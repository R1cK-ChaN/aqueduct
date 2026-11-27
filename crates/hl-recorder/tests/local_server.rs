//! Runs the recorder and a caller-supplied sink against a WebSocket server
//! on localhost and checks subscriptions, received frames, a stale
//! connection, the reconnect, pings and a clean shutdown.

use std::fs;
use std::io::ErrorKind;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use hl_codec::rawlog::Record;
use hl_recorder::{Config, RecordKind, SourceSink, inspect, open_recording};
use tungstenite::{Message, WebSocket};

const SUBSCRIPTIONS: [&str; 3] = [r#"{"sub":"all"}"#, r#"{"sub":"AAA"}"#, r#"{"sub":"BBB"}"#];

fn accept(listener: &TcpListener) -> WebSocket<TcpStream> {
    let (stream, _) = listener.accept().expect("recorder connects");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set timeout");
    tungstenite::accept(stream).expect("websocket handshake")
}

/// Reads the three subscriptions and acknowledges each.
fn expect_subscriptions(socket: &mut WebSocket<TcpStream>) {
    for expected in SUBSCRIPTIONS {
        let message = socket.read().expect("subscription arrives");
        assert_eq!(message.to_text().expect("text frame"), expected);
        socket
            .send(Message::text(format!("ack {expected}")))
            .expect("send ack");
    }
}

/// The venue. First connection: acknowledges, sends three frames, then goes
/// silent so the recorder must give up on it. Second connection:
/// acknowledges, sends one frame, then keeps the link busy and reports the
/// first ping it sees.
fn serve(listener: &TcpListener, ping_seen: &mpsc::Sender<()>) {
    let mut first = accept(listener);
    expect_subscriptions(&mut first);
    first.send(Message::text("first-1")).expect("send");
    first
        .send(Message::binary(vec![0_u8, 1, 2, 255]))
        .expect("send");
    first.send(Message::text("first-2")).expect("send");
    // Silent from here on. The recorder hangs up once the link is stale,
    // which ends this read with an error.
    while first.read().is_ok() {}

    let mut second = accept(listener);
    expect_subscriptions(&mut second);
    second.send(Message::text("second-1")).expect("send");
    second
        .get_ref()
        .set_read_timeout(Some(Duration::from_millis(20)))
        .expect("set timeout");
    let mut reported = false;
    loop {
        match second.read() {
            Ok(Message::Text(text)) if text.as_str() == "PING" && !reported => {
                reported = true;
                ping_seen.send(()).expect("test is waiting");
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(tungstenite::Error::Io(error))
                if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => break,
        }
        if second.send(Message::text("tick")).is_err() {
            break;
        }
    }
}

fn recordings(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_owned()];
    while let Some(next) = pending.pop() {
        for entry in fs::read_dir(next).expect("readable directory") {
            let path = entry.expect("readable entry").path();
            if path.is_dir() {
                pending.push(path)
            } else {
                files.push(path)
            }
        }
    }
    files.sort();
    files
}

fn texts(records: &[Record], kind: RecordKind) -> Vec<String> {
    records
        .iter()
        .filter(|record| record.kind == kind)
        .map(|record| String::from_utf8(record.payload.clone()).expect("utf-8"))
        .collect()
}

fn run_local_server<T: Send + 'static>(
    out_dir: &Path,
    run: impl FnOnce(Config, Arc<AtomicBool>) -> T + Send + 'static,
) -> T {
    let listener = TcpListener::bind("127.0.0.1:0").expect("free port");
    let port = listener.local_addr().expect("bound").port();
    let (ping_seen_tx, ping_seen) = mpsc::channel();
    let server = thread::spawn(move || serve(&listener, &ping_seen_tx));

    let config = Config::from_toml(&format!(
        r#"
        out_dir = "{}"

        [[source]]
        name = "local"
        url = "ws://127.0.0.1:{port}/ws"
        stale_after_ms = 300
        subscribe_pace_ms = 5
        ping_text = "PING"
        ping_interval_secs = 1
        symbols = ["AAA", "BBB"]
        subscribe_each = ['{{"sub":"{{symbol}}"}}']
        subscribe_once = ['{{"sub":"all"}}']
        "#,
        out_dir.display()
    ))
    .expect("valid config");

    let shutdown = Arc::new(AtomicBool::new(false));
    let client = {
        let shutdown = Arc::clone(&shutdown);
        thread::spawn(move || run(config, shutdown))
    };

    // The second connection has been up for a second when its ping arrives.
    ping_seen
        .recv_timeout(Duration::from_secs(20))
        .expect("recorder reconnects and pings");
    shutdown.store(true, Ordering::Relaxed);
    let result = client.join().expect("client thread");
    server.join().expect("server thread");
    result
}

#[test]
fn records_frames_across_a_reconnect_and_shuts_down_cleanly() {
    let out_dir = std::env::temp_dir().join(format!("hl-recorder-it-{}", std::process::id()));
    let _ = fs::remove_dir_all(&out_dir);
    run_local_server(&out_dir, |config, shutdown| {
        hl_recorder::run(&config, &shutdown)
    })
    .expect("recorder stops cleanly");

    let files = recordings(&out_dir);
    assert!(!files.is_empty());
    for file in &files {
        let name = file
            .file_name()
            .and_then(|name| name.to_str())
            .expect("utf-8 name");
        assert!(
            name.starts_with("local-") && name.ends_with(".hlrec.zst"),
            "{name}"
        );
        assert!(!name.contains("truncated"), "{name}");
        assert!(
            file.parent()
                .expect("day directory")
                .parent()
                .expect("source")
                .ends_with("local")
        );
    }

    // Normally one file; two if the test straddled the top of an hour.
    let mut records = Vec::new();
    let mut reported = 0;
    for file in &files {
        let report = inspect(file).expect("readable file");
        assert_eq!(report.error, None, "a sealed file ends cleanly");
        reported += report.records;
        records.extend(
            open_recording(file)
                .expect("open")
                .map(|record| record.expect("record")),
        );
    }
    assert_eq!(reported, records.len() as u64);

    assert_scenario_records(&records);
    fs::remove_dir_all(&out_dir).expect("clean up");
}

#[derive(Default)]
struct CapturingSink {
    records: Vec<Record>,
}

impl SourceSink for CapturingSink {
    fn begin(&mut self, source: usize, kind: RecordKind) -> impl FnOnce(Vec<u8>) + '_ {
        assert_eq!(
            source, 7,
            "the caller's source index accompanies every item"
        );
        let sequence = self.records.len() as u64;
        move |payload| {
            self.records.push(Record {
                kind,
                recv_wall_ns: 123_456,
                recv_mono_ns: sequence,
                payload,
            });
        }
    }
}

#[test]
fn caller_supplied_sink_receives_the_same_scenario_in_order() {
    let records = run_local_server(Path::new("unused-by-capturing-sink"), |config, shutdown| {
        let mut sink = CapturingSink::default();
        hl_recorder::run_source(7, &config.sources[0], &shutdown, &mut sink);
        sink.records
    });
    assert_scenario_records(&records);
}

fn assert_scenario_records(records: &[Record]) {
    let events: Vec<String> = texts(records, RecordKind::Meta)
        .iter()
        .map(|text| {
            let meta: serde_json::Value = serde_json::from_str(text).expect("meta is JSON");
            meta["event"].as_str().expect("event field").to_owned()
        })
        .filter(|event| event != "file_open")
        .collect();
    assert_eq!(
        events,
        [
            "connecting",
            "connected",
            "disconnected",
            "connecting",
            "connected",
            "shutdown"
        ]
    );
    let disconnect = texts(records, RecordKind::Meta)
        .into_iter()
        .find(|text| text.contains("disconnected"))
        .expect("disconnect recorded");
    assert!(disconnect.contains("no frame for 300 ms"), "{disconnect}");

    let sent = texts(records, RecordKind::SentText);
    assert_eq!(
        sent[..3],
        SUBSCRIPTIONS,
        "subscribed on the first connection"
    );
    assert_eq!(sent[3..6], SUBSCRIPTIONS, "and again after reconnecting");
    assert!(
        sent.len() > 6 && sent[6..].iter().all(|text| text == "PING"),
        "{sent:?}"
    );

    let received: Vec<String> = texts(records, RecordKind::RecvText)
        .into_iter()
        .filter(|text| text != "tick")
        .collect();
    let acks: Vec<String> = SUBSCRIPTIONS
        .iter()
        .map(|text| format!("ack {text}"))
        .collect();
    let mut expected = acks.clone();
    expected.extend(["first-1".to_owned(), "first-2".to_owned()]);
    expected.extend(acks);
    expected.push("second-1".to_owned());
    assert_eq!(
        received, expected,
        "every frame is present, in order, exactly once"
    );

    let binary: Vec<&Record> = records
        .iter()
        .filter(|record| record.kind == RecordKind::RecvBinary)
        .collect();
    assert_eq!(binary.len(), 1);
    assert_eq!(binary[0].payload, [0, 1, 2, 255]);

    assert!(
        records
            .windows(2)
            .all(|pair| pair[0].recv_mono_ns <= pair[1].recv_mono_ns),
        "records are in the order they were stamped"
    );
    assert!(
        records
            .iter()
            .filter(|record| record.kind == RecordKind::RecvText)
            .count()
            > 20
    );

    assert_payload_order(records);
}

fn assert_payload_order(records: &[Record]) {
    // The timers can vary the number of ticks and the interleaving of
    // outbound subscriptions with acknowledgements. Check the exact
    // lifecycle/receive sequence and each subscription before its ack.
    let mut sent = Vec::new();
    let mut actual = Vec::new();
    let mut connected = false;
    let mut connection = 0;
    for record in records {
        match record.kind {
            RecordKind::Meta => {
                let meta: serde_json::Value =
                    serde_json::from_slice(&record.payload).expect("meta is JSON");
                if meta["event"] == "file_open" {
                    continue;
                }
                if meta["event"] == "connected" {
                    sent.clear();
                    connected = true;
                    connection += 1;
                } else if meta["event"] == "disconnected" || meta["event"] == "shutdown" {
                    connected = false;
                }
            }
            RecordKind::SentText => {
                assert!(connected, "sent text belongs to an active connection");
                if record.payload == b"PING" {
                    assert_eq!(connection, 2, "the active second connection sends pings");
                }
                sent.push(record.payload.as_slice());
                continue;
            }
            RecordKind::RecvText => {
                if record.payload == b"tick" {
                    assert_eq!(
                        actual.last(),
                        Some(&(RecordKind::RecvText, b"second-1".as_slice())),
                        "ticks follow the second connection's data frame"
                    );
                    continue;
                }
                if let Some(subscription) = record.payload.strip_prefix(b"ack ") {
                    assert!(
                        sent.contains(&subscription),
                        "an acknowledgement follows its subscription on this connection"
                    );
                }
            }
            RecordKind::RecvBinary => {}
        }
        actual.push((record.kind, record.payload.as_slice()));
    }

    let expected: Vec<(RecordKind, &[u8])> = vec![
        (RecordKind::Meta, br#"{"attempt":1,"event":"connecting"}"#),
        (RecordKind::Meta, br#"{"event":"connected"}"#),
        (RecordKind::RecvText, br#"ack {"sub":"all"}"#),
        (RecordKind::RecvText, br#"ack {"sub":"AAA"}"#),
        (RecordKind::RecvText, br#"ack {"sub":"BBB"}"#),
        (RecordKind::RecvText, b"first-1"),
        (RecordKind::RecvBinary, &[0, 1, 2, 255]),
        (RecordKind::RecvText, b"first-2"),
        (
            RecordKind::Meta,
            br#"{"event":"disconnected","reason":"no frame for 300 ms"}"#,
        ),
        (RecordKind::Meta, br#"{"attempt":2,"event":"connecting"}"#),
        (RecordKind::Meta, br#"{"event":"connected"}"#),
        (RecordKind::RecvText, br#"ack {"sub":"all"}"#),
        (RecordKind::RecvText, br#"ack {"sub":"AAA"}"#),
        (RecordKind::RecvText, br#"ack {"sub":"BBB"}"#),
        (RecordKind::RecvText, b"second-1"),
        (RecordKind::Meta, br#"{"event":"shutdown"}"#),
    ];
    assert_eq!(actual, expected, "the same ordered kinds and payload bytes");
}
