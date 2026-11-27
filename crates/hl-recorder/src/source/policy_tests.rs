use super::*;
use crate::{Clock, Config, Item, RecorderSink};
use std::io::Read;
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, sync_channel};
use std::thread::{self, JoinHandle};

const REJECTED_TEXT: &str = "synthetic-rejected-text-marker";
const TIMEOUT: Duration = Duration::from_secs(8);

fn loopback_config(listener: &TcpListener) -> Config {
    let mut config = Config::from_toml(
        r#"
        out_dir = "unused"
        [[source]]
        name = "hl-public"
        send_policy = "hl-public-v1"
        url = "wss://api.hyperliquid.xyz/ws"
        ping_text = '{"method":"ping"}'
        subscribe_once = ['{"method":"subscribe","subscription":{"type":"fastAssetCtxs"}}']
        "#,
    )
    .expect("valid synthetic public source");
    let address = listener.local_addr().expect("bound address");
    assert!(address.ip().is_loopback());
    config.sources[0].url = format!("ws://{address}/ws");
    config
}

// Exercise the send and reconnect paths below public-entry-point validation.
fn spawn_local_loop(
    source: SourceConfig,
    shutdown: &Arc<AtomicBool>,
    window: Duration,
) -> (JoinHandle<io::Result<()>>, Receiver<Item>) {
    assert!(source.url.starts_with("ws://127.0.0.1:"));
    let (queue, records) = sync_channel(64);
    let stop = Arc::clone(shutdown);
    let worker = thread::spawn(move || {
        let mut sink = RecorderSink {
            clock: Clock::start(),
            queue,
            dropped: Default::default(),
        };
        let context = SourceContext {
            index: 0,
            config: &source,
            shutdown: &stop,
            sink: &mut sink,
        };
        run_loop(context, Some(&mut AttemptWindow::new(window)))
    });
    (worker, records)
}

fn accept(listener: &TcpListener) -> WebSocket<TcpStream> {
    listener.set_nonblocking(true).expect("bounded accept");
    let deadline = Instant::now() + TIMEOUT;
    let stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("expected a loopback connection: {error}"),
        }
    };
    stream.set_nonblocking(false).expect("blocking handshake");
    stream
        .set_read_timeout(Some(TIMEOUT))
        .expect("bounded read");
    tungstenite::accept(stream).expect("loopback handshake")
}

#[test]
fn public_run_source_refuses_a_changed_url_before_connecting() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("free port");
    let source = loopback_config(&listener).sources.remove(0);
    // Any source-loop event would mean validation let the altered URL through.
    struct NoRecords;
    impl SourceSink for NoRecords {
        fn begin(&mut self, _: usize, _: RecordKind) -> impl FnOnce(Vec<u8>) + '_ {
            |_| panic!("invalid policy source entered the connection loop")
        }
    }
    let stop = AtomicBool::new(false);
    run_source(0, &source, &stop, &mut NoRecords);
    assert!(stop.load(Ordering::Relaxed));
    listener.set_nonblocking(true).expect("check for a dial");
    assert_eq!(
        listener.accept().expect_err("no TCP connection").kind(),
        io::ErrorKind::WouldBlock
    );
}

#[test]
fn rejected_send_stops_the_loop_without_sending_or_recording_the_text() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("free port");
    let mut config = loopback_config(&listener);
    let out = std::env::temp_dir().join(format!(
        "hl-recorder-rejected-{}-{}",
        std::process::id(),
        Clock::start().start_wall_ns(),
    ));
    config.out_dir = out.clone();
    config.sources[0].subscribe_once = vec![REJECTED_TEXT.to_owned()];
    let stop = Arc::new(AtomicBool::new(false));
    let shutdown = Arc::clone(&stop);
    let worker = thread::spawn(move || {
        crate::record_sources(&config, &shutdown, |index, source, shutdown, sink| {
            assert!(source.url.starts_with("ws://127.0.0.1:"));
            let context = SourceContext {
                index,
                config: source,
                shutdown,
                sink,
            };
            run_loop(context, Some(&mut AttemptWindow::new(public_v1::WINDOW)))
        })
    });
    let mut peer = accept(&listener);
    // After the handshake even the first byte of a frame would be observable.
    let wire = peer.get_mut().read(&mut [0_u8; 1]);
    let stopped_itself = stop.swap(true, Ordering::Relaxed);
    let error = worker
        .join()
        .expect("recorder thread")
        .expect_err("recorder propagates the fatal source error");
    assert!(stopped_itself);
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert!(!error.to_string().contains(REJECTED_TEXT));
    assert_eq!(wire.expect("peer reads EOF"), 0);
    let mut files: Vec<_> = std::fs::read_dir(out.join("hl-public"))
        .unwrap()
        .flat_map(|day| std::fs::read_dir(day.unwrap().path()).unwrap())
        .map(|file| file.unwrap().path())
        .collect();
    files.sort();
    let mut events = Vec::new();
    for file in files {
        assert!(file.to_str().unwrap().ends_with(crate::FINISHED_SUFFIX));
        for record in crate::open_recording(&file).expect("sealed recording") {
            let record = record.expect("complete RFC 0002 record");
            assert_eq!(record.kind, RecordKind::Meta);
            let payload = String::from_utf8(record.payload).unwrap();
            assert!(!payload.contains(REJECTED_TEXT));
            let meta: Value = serde_json::from_str(&payload).expect("lifecycle record");
            if meta["event"] != "file_open" {
                events.push(meta);
            }
        }
    }
    assert_eq!(
        events,
        [
            json!({ "event": "connecting", "attempt": 1 }),
            json!({ "event": "connected" }),
            json!({ "event": "send_policy_rejected" }),
        ]
    );
    std::fs::remove_dir_all(out).unwrap();
}

#[test]
fn rejection_log_omits_the_rejected_text() {
    let output = std::process::Command::new(std::env::current_exe().expect("unit test binary"))
        .args([
            "--exact",
            "source::policy_tests::rejected_send_stops_the_loop_without_sending_or_recording_the_text",
            "--nocapture",
        ])
        .output()
        .expect("run the loopback rejection with captured stderr");
    assert!(output.status.success());
    let log = String::from_utf8(output.stderr).expect("UTF-8 operational log");
    assert!(log.contains(r#""event":"send_policy_rejected""#));
    assert!(!log.contains(REJECTED_TEXT));
}

#[test]
fn immediately_closed_connections_obey_the_rolling_attempt_limit() {
    const WINDOW: Duration = Duration::from_millis(2_200);
    let listener = TcpListener::bind("127.0.0.1:0").expect("free port");
    let stop = Arc::new(AtomicBool::new(false));
    let source = loopback_config(&listener).sources.remove(0);
    let (worker, records) = spawn_local_loop(source, &stop, WINDOW);
    for _ in 0..4 {
        drop(accept(&listener));
    }
    stop.store(true, Ordering::Relaxed);
    worker.join().expect("source thread").expect("normal stop");

    // The client stamps connecting after the wait, before the TCP dial.
    // Accept/handshake latency on the server would skew the measured starts.
    let starts: Vec<_> = records
        .into_iter()
        .filter(|item| item.kind == RecordKind::Meta)
        .filter(|item| {
            let meta: Value = serde_json::from_slice(&item.payload).expect("lifecycle record");
            meta["event"] == "connecting"
        })
        .map(|item| Duration::from_nanos(item.stamp.mono_ns))
        .collect();
    assert_eq!(starts.len(), 4);
    for triple in starts.windows(3) {
        assert!(triple[2] - triple[0] >= WINDOW, "{starts:?}");
    }
}

#[test]
fn a_guarded_attempt_dials_one_resolved_address_and_rotates_on_retry() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("free port");
    listener.set_nonblocking(true).expect("bounded accept");
    let available = listener.local_addr().expect("bound address");
    let unavailable = SocketAddr::new(available.ip(), 0);
    let addresses = [unavailable, available];
    let mut attempts = AttemptWindow::new(public_v1::WINDOW);

    connect_tcp("loopback", Ok(addresses), Some(&mut attempts))
        .expect_err("first attempt stops at the unavailable address");
    assert_eq!(
        listener.accept().expect_err("no fallback dial").kind(),
        io::ErrorKind::WouldBlock
    );
    connect_tcp("loopback", Ok(addresses), Some(&mut attempts))
        .expect("second attempt selects the other address");
    listener.accept().expect("one guarded TCP connection");

    connect_tcp("loopback", Ok(addresses), None).expect("unrestricted source tries both addresses");
    listener.accept().expect("one unrestricted TCP connection");
}

#[test]
fn failed_address_lookups_consume_attempts() {
    let mut attempts = AttemptWindow::new(public_v1::WINDOW);
    for _ in 0..public_v1::MAX_ATTEMPTS {
        assert!(attempts.delay().is_zero());
        let unresolved: io::Result<[SocketAddr; 0]> =
            Err(io::Error::other("synthetic lookup failure"));
        connect_tcp("loopback", unresolved, Some(&mut attempts)).expect_err("failed lookup");
    }
    assert!(!attempts.delay().is_zero());
}
