//! One source's WebSocket connection and reconnect loop.

use std::io;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use hl_codec::rawlog::RecordKind;
use serde_json::{Value, json};
use tungstenite::client::IntoClientRequest;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use crate::config::SourceConfig;
use crate::{install_tls_provider, log, public_v1};

/// How long a TCP connect, the TLS handshake or a send may take.
const SETUP_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a read waits before the loop checks its timers. Frames are
/// handed over as soon as they arrive; this only bounds how late a ping, a
/// subscription or a shutdown can be.
const READ_TICK: Duration = Duration::from_millis(50);

/// A connection that lasted this long resets the reconnect backoff.
const BACKOFF_RESET_AFTER: Duration = Duration::from_secs(30);

const BACKOFF_FIRST: Duration = Duration::from_millis(500);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

// RFC 0010 D2/D3 assigns this process one public source. Keep its attempt
// starts if a library caller stops and starts that source again.
static PUBLIC_ATTEMPTS: Mutex<AttemptWindow> = Mutex::new(AttemptWindow::new(public_v1::WINDOW));

struct AttemptWindow {
    window: Duration,
    starts: [Option<Instant>; public_v1::MAX_ATTEMPTS],
    next_address: usize,
}

impl AttemptWindow {
    const fn new(window: Duration) -> Self {
        Self {
            window,
            starts: [None; public_v1::MAX_ATTEMPTS],
            next_address: 0,
        }
    }

    fn delay(&self) -> Duration {
        self.starts[0].map_or(Duration::ZERO, |oldest| {
            self.window.saturating_sub(oldest.elapsed())
        })
    }

    fn start(&mut self) {
        self.starts.rotate_left(1);
        self.starts[public_v1::MAX_ATTEMPTS - 1] = Some(Instant::now());
    }

    fn select_address(&mut self, addresses: &[SocketAddr]) -> Option<SocketAddr> {
        if addresses.is_empty() {
            return None;
        }
        let index = self.next_address % addresses.len();
        self.next_address = index + 1;
        Some(addresses[index])
    }
}

/// Admits a single source's records in the order produced by [`run_source`].
///
/// The sink owns the timestamp. A sink that shares an ordered queue with a
/// scheduler can acquire its admission lock and stamp in [`Self::begin`],
/// then move the guard into the returned closure and append under that lock.
/// See `docs/rfc/0008-paper-trading.md` D4 and D5.
pub trait SourceSink {
    /// Begins one record before its payload is copied or serialized. The loop
    /// calls the returned closure exactly once with the complete RFC 0002
    /// payload, on the same thread, before reading or producing another item.
    fn begin(&mut self, source: usize, kind: RecordKind) -> impl FnOnce(Vec<u8>) + '_;

    /// Admits the final lifecycle record when the send policy stops a source.
    /// A bounded recorder queue can wait for space for this record.
    fn terminal_meta(&mut self, source: usize, payload: Vec<u8>) {
        self.begin(source, RecordKind::Meta)(payload);
    }
}

struct SourceContext<'a, S> {
    index: usize,
    config: &'a SourceConfig,
    shutdown: &'a AtomicBool,
    sink: &'a mut S,
}

/// Delays between reconnect attempts: doubling from [`BACKOFF_FIRST`] up to
/// [`BACKOFF_MAX`].
#[derive(Debug)]
struct Backoff {
    next: Duration,
}

impl Backoff {
    fn new() -> Self {
        Self {
            next: BACKOFF_FIRST,
        }
    }

    fn next_delay(&mut self) -> Duration {
        let delay = self.next;
        self.next = (self.next * 2).min(BACKOFF_MAX);
        delay
    }

    fn reset(&mut self) {
        self.next = BACKOFF_FIRST;
    }
}

type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

enum SessionEnd {
    Shutdown,
    Lost(String),
    SendPolicyRejected,
}

fn connect_tcp(
    host: &str,
    resolved: io::Result<impl AsRef<[SocketAddr]>>,
    attempts: Option<&mut AttemptWindow>,
) -> Result<TcpStream, String> {
    let addresses = resolved.map_err(|error| error.to_string());
    let no_address = || format!("{host} did not resolve to an address");
    let dial = |address: &SocketAddr| {
        TcpStream::connect_timeout(address, SETUP_TIMEOUT)
            .map_err(|error| format!("{address}: {error}"))
    };
    if let Some(window) = attempts {
        let address = addresses.and_then(|addresses| {
            window
                .select_address(addresses.as_ref())
                .ok_or_else(no_address)
        });
        // DNS and the connecting Meta callback may take time. Charge a lookup
        // failure here too, and start at most one TCP dial for this attempt.
        window.start();
        return dial(&address?);
    }

    let mut failure = no_address();
    for address in addresses?.as_ref() {
        match dial(address) {
            Ok(stream) => return Ok(stream),
            Err(error) => failure = error,
        }
    }
    Err(failure)
}

fn connect(url: &str, attempts: Option<&mut AttemptWindow>) -> Result<Socket, String> {
    let request = url
        .into_client_request()
        .map_err(|error| error.to_string())?;
    let uri = request.uri();
    let host = uri.host().ok_or("url has no host")?.to_owned();
    let default_port = if uri.scheme_str() == Some("wss") {
        443
    } else {
        80
    };
    let port = uri.port_u16().unwrap_or(default_port);

    let stream = connect_tcp(&host, (host.as_str(), port).to_socket_addrs(), attempts)?;

    let configure = |stream: &TcpStream, read_timeout: Duration| -> io::Result<()> {
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(read_timeout))?;
        stream.set_write_timeout(Some(SETUP_TIMEOUT))
    };
    configure(&stream, SETUP_TIMEOUT).map_err(|error| error.to_string())?;
    // A second handle to the same socket, to shorten the read timeout once
    // the handshake is done.
    let handle = stream.try_clone().map_err(|error| error.to_string())?;

    let (socket, _response) =
        tungstenite::client_tls(request, stream).map_err(|error| error.to_string())?;
    configure(&handle, READ_TICK).map_err(|error| error.to_string())?;
    Ok(socket)
}

impl<S: SourceSink> SourceContext<'_, S> {
    fn is_shutting_down(&self) -> bool {
        self.shutdown.load(Ordering::Relaxed)
    }

    /// Records a lifecycle event in the data and in the operational log.
    fn meta(&mut self, level: &str, event: &str, mut fields: Value) {
        let mut record = json!({ "event": event });
        if let (Value::Object(record), Value::Object(fields)) = (&mut record, &fields) {
            record.extend(fields.clone());
        }
        let admit = self.sink.begin(self.index, RecordKind::Meta);
        admit(record.to_string().into_bytes());

        if let Value::Object(fields) = &mut fields {
            fields.insert("source".to_owned(), json!(self.config.name));
        }
        log::emit(level, event, fields);
    }

    fn send_text(&mut self, socket: &mut Socket, text: &str) -> Result<(), SessionEnd> {
        if self.config.send_policy().is_some() && !public_v1::allows(text) {
            return Err(SessionEnd::SendPolicyRejected);
        }
        socket
            .send(Message::text(text))
            .map_err(|error| SessionEnd::Lost(format!("send failed: {error}")))?;
        let admit = self.sink.begin(self.index, RecordKind::SentText);
        admit(text.as_bytes().to_vec());
        Ok(())
    }

    fn received(&mut self, kind: RecordKind, payload: &[u8], last_received: &mut Instant) {
        let admit = self.sink.begin(self.index, kind);
        // Measure idle time from receipt, before copying or queueing the frame.
        *last_received = Instant::now();
        admit(payload.to_vec());
    }

    /// Runs one connection until it is lost or the recorder shuts down.
    fn run_session(&mut self, socket: &mut Socket) -> SessionEnd {
        let subscriptions = self.config.subscriptions();
        let pace = Duration::from_millis(self.config.subscribe_pace_ms);
        let stale_after = Duration::from_millis(self.config.stale_after_ms);
        let ping_every = Duration::from_secs(self.config.ping_interval_secs);

        let mut subscriptions_sent = 0;
        let mut next_subscription_at = Instant::now();
        let mut last_ping = Instant::now();
        let mut last_received = Instant::now();

        loop {
            if self.is_shutting_down() {
                let _ = socket.close(None);
                let _ = socket.flush();
                return SessionEnd::Shutdown;
            }

            let now = Instant::now();
            if subscriptions_sent < subscriptions.len() && now >= next_subscription_at {
                if let Err(end) = self.send_text(socket, &subscriptions[subscriptions_sent]) {
                    return end;
                }
                subscriptions_sent += 1;
                next_subscription_at = now + pace;
            }
            if let Some(ping) = &self.config.ping_text
                && now.duration_since(last_ping) >= ping_every
            {
                if let Err(end) = self.send_text(socket, ping) {
                    return end;
                }
                last_ping = now;
            }
            if now.duration_since(last_received) >= stale_after {
                return SessionEnd::Lost(format!("no frame for {} ms", self.config.stale_after_ms));
            }

            match socket.read() {
                Ok(Message::Text(text)) => {
                    self.received(RecordKind::RecvText, text.as_bytes(), &mut last_received);
                }
                Ok(Message::Binary(bytes)) => {
                    self.received(RecordKind::RecvBinary, &bytes, &mut last_received);
                }
                Ok(Message::Close(frame)) => {
                    let reason = frame.map_or_else(
                        || "closed by the server".to_owned(),
                        |frame| format!("closed by the server: {} {}", frame.code, frame.reason),
                    );
                    return SessionEnd::Lost(reason);
                }
                // Protocol-level pings are answered by the library on the
                // next read. They still prove the link is alive.
                Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {
                    last_received = Instant::now();
                }
                Err(tungstenite::Error::Io(error))
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) => {}
                Err(error) => return SessionEnd::Lost(error.to_string()),
            }
        }
    }

    /// Sleeps for `delay` in short steps. Returns false when the recorder
    /// started shutting down.
    fn sleep(&self, delay: Duration) -> bool {
        let until = Instant::now() + delay;
        while Instant::now() < until {
            if self.is_shutting_down() {
                return false;
            }
            std::thread::sleep(READ_TICK.min(until.saturating_duration_since(Instant::now())));
        }
        !self.is_shutting_down()
    }
}

/// Runs one source's connection, subscription, ping and reconnect loop on the
/// calling thread until `shutdown` is set. Each admission carries `index`.
///
/// The loop calls [`SourceSink::begin`] as soon as a text or binary read
/// succeeds, before checking timers or copying frame bytes. Received frames,
/// successful text sends and lifecycle events reach the sink in the order they
/// occur across reconnects. A normal stop ends with a `shutdown` record.
/// A send policy violation emits a terminal rejection and sets `shutdown`.
/// A policy config is validated again before the first connection. Lifecycle
/// payloads are the recorder's raw `Meta` JSON bytes. The sink takes its own
/// stamps during admission as specified in `docs/rfc/0008-paper-trading.md`
/// D4 and D5.
pub fn run_source(
    index: usize,
    config: &SourceConfig,
    shutdown: &AtomicBool,
    sink: &mut impl SourceSink,
) {
    // The caller observes a fatal stop through the flag and operational log.
    let _ = run_source_checked(index, config, shutdown, sink);
}

pub(crate) fn run_source_checked(
    index: usize,
    config: &SourceConfig,
    shutdown: &AtomicBool,
    sink: &mut impl SourceSink,
) -> io::Result<()> {
    let mut attempts = if config.send_policy().is_some() {
        let refuse = |reason: &'static str| {
            shutdown.store(true, Ordering::Relaxed);
            log::error("send_policy_source_stopped", json!({ "reason": reason }));
            io::Error::new(io::ErrorKind::InvalidInput, reason)
        };
        config
            .validate()
            .map_err(|_| refuse("invalid hl-public-v1 source config"))?;
        Some(PUBLIC_ATTEMPTS.try_lock().map_err(|_| {
            refuse("hl-public-v1 source is already running or its attempt history is unavailable")
        })?)
    } else {
        None
    };
    let context = SourceContext {
        index,
        config,
        shutdown,
        sink,
    };
    run_loop(context, attempts.as_deref_mut())
}

fn run_loop(
    mut context: SourceContext<'_, impl SourceSink>,
    mut attempts: Option<&mut AttemptWindow>,
) -> io::Result<()> {
    install_tls_provider();
    let mut backoff = Backoff::new();
    let mut attempt: u64 = 0;
    while !context.is_shutting_down() {
        if let Some(window) = &attempts
            && !context.sleep(window.delay())
        {
            break;
        }
        attempt += 1;
        context.meta("info", "connecting", json!({ "attempt": attempt }));
        match connect(&context.config.url, attempts.as_deref_mut()) {
            Ok(mut socket) => {
                context.meta("info", "connected", json!({}));
                let connected_at = Instant::now();
                match context.run_session(&mut socket) {
                    SessionEnd::Shutdown => break,
                    SessionEnd::SendPolicyRejected => {
                        const EVENT: &str = "send_policy_rejected";
                        context.shutdown.store(true, Ordering::Relaxed);
                        log::error(EVENT, json!({ "source": context.config.name }));
                        context.sink.terminal_meta(
                            context.index,
                            json!({ "event": EVENT }).to_string().into_bytes(),
                        );
                        return Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            "hl-public-v1 rejected an outgoing text",
                        ));
                    }
                    SessionEnd::Lost(reason) => {
                        context.meta("warn", "disconnected", json!({ "reason": reason }));
                    }
                }
                if connected_at.elapsed() >= BACKOFF_RESET_AFTER {
                    backoff.reset();
                }
            }
            Err(error) => context.meta("warn", "connect_failed", json!({ "error": error })),
        }
        if !context.sleep(backoff.next_delay()) {
            break;
        }
    }
    context.meta("info", "shutdown", json!({}));
    Ok(())
}

#[cfg(test)]
mod policy_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Clock, Config, RecorderSink};
    use std::sync::mpsc::sync_channel;

    #[test]
    fn lifecycle_events_are_queued_as_meta_records() {
        let config = Config::from_toml(
            r#"
            out_dir = "/tmp/unused"
            [[source]]
            name = "test"
            url = "ws://127.0.0.1:1/ws"
            subscribe_once = ["hello"]
            "#,
        )
        .unwrap();
        let (queue, received) = sync_channel(2);
        let mut sink = RecorderSink {
            clock: Clock::start(),
            queue,
            dropped: Default::default(),
        };
        let mut context = SourceContext {
            index: 3,
            config: &config.sources[0],
            shutdown: &AtomicBool::new(false),
            sink: &mut sink,
        };
        context.meta("warn", "disconnected", json!({ "reason": "test" }));

        let item = received.try_recv().unwrap();
        assert_eq!(item.kind, RecordKind::Meta);
        let record: Value = serde_json::from_slice(&item.payload).unwrap();
        assert_eq!(record, json!({ "event": "disconnected", "reason": "test" }));
    }

    #[test]
    fn backoff_doubles_up_to_the_cap_and_resets() {
        let mut backoff = Backoff::new();
        let delays: Vec<u64> = (0..9)
            .map(|_| backoff.next_delay().as_millis() as u64)
            .collect();
        assert_eq!(
            delays,
            [
                500, 1_000, 2_000, 4_000, 8_000, 16_000, 30_000, 30_000, 30_000
            ]
        );

        backoff.reset();
        assert_eq!(backoff.next_delay(), Duration::from_millis(500));
    }
}
