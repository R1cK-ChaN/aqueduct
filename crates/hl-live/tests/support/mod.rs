//! Loopback source, paper settings and offline checker shared by paper tests.

use std::fs;
use std::io::ErrorKind;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tungstenite::{Message, WebSocket};

static CHECKER: OnceLock<PathBuf> = OnceLock::new();
pub const TIMEOUT: Duration = Duration::from_secs(15);
pub const SYNTH_RULES: &str = include_str!("../../../hl-backtest/tests/fixtures/synth-rules.json");

pub fn config_text(out_dir: &Path, port: u16) -> String {
    config_text_for_coin(out_dir, port, "BTC")
}

pub fn config_text_for_coin(out_dir: &Path, port: u16, coin: &str) -> String {
    format!(
        r#"out_dir = {:?}
queue_capacity = 1024
zstd_level = 1
network = "testnet"
coin = "{coin}"
strategy_name = "fixed_spread"

[source]
name = "loopback"
url = "ws://127.0.0.1:{port}/ws"
stale_after_ms = 60000
subscribe_pace_ms = 0
subscribe_once = ['{{"method":"subscribe","subscription":{{"type":"bbo","coin":"{coin}"}}}}', '{{"method":"subscribe","subscription":{{"type":"trades","coin":"{coin}"}}}}']

[strategy]
half_spread_quote_per_base = "1"
qty_base = "1"

[sim]
order_latency_ns = "0"
report_latency_ns = "0"
maker_fee_bps = "1.5"
min_order_notional_usd = "10"
"#,
        out_dir.display().to_string()
    )
}

pub fn listener() -> TcpListener {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    listener
}

pub fn connect(listener: &TcpListener) -> WebSocket<TcpStream> {
    connect_for_coin(listener, "BTC")
}

pub fn connect_for_coin(listener: &TcpListener, coin: &str) -> WebSocket<TcpStream> {
    let deadline = Instant::now() + TIMEOUT;
    let stream = loop {
        match listener.accept() {
            Ok((stream, peer)) => {
                assert!(peer.ip().is_loopback());
                break stream;
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "paper did not connect");
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("loopback accept: {error}"),
        }
    };
    stream
        .set_read_timeout(Some(TIMEOUT))
        .expect("bound subscription wait");
    stream
        .set_write_timeout(Some(TIMEOUT))
        .expect("bound server writes");
    let mut socket = tungstenite::accept(stream).expect("WebSocket handshake");
    for channel in ["bbo", "trades"] {
        let message = socket.read().expect("subscription");
        let subscription: Value =
            serde_json::from_str(message.to_text().expect("text subscription"))
                .expect("JSON subscription");
        assert_eq!(subscription["subscription"]["type"], channel);
        assert_eq!(subscription["subscription"]["coin"], coin);
    }
    socket
}

pub fn bbo(socket: &mut WebSocket<TcpStream>, time: u64, bid: &str, ask: &str) {
    bbo_for_coin(socket, "BTC", time, bid, ask);
}

pub fn bbo_for_coin(
    socket: &mut WebSocket<TcpStream>,
    coin: &str,
    time: u64,
    bid: &str,
    ask: &str,
) {
    socket
        .send(Message::text(
            json!({"channel":"bbo","data":{
                "coin":coin,"time":time,
                "bbo":[{"px":bid,"sz":"10","n":1},{"px":ask,"sz":"10","n":1}]
            }})
            .to_string(),
        ))
        .expect("send loopback BBO");
}

pub fn trade(socket: &mut WebSocket<TcpStream>, time: u64, id: u64, quantity: &str) {
    trade_for_coin(socket, "BTC", time, id, quantity);
}

pub fn trade_for_coin(
    socket: &mut WebSocket<TcpStream>,
    coin: &str,
    time: u64,
    id: u64,
    quantity: &str,
) {
    socket
        .send(Message::text(
            json!({"channel":"trades","data":[{
                "coin":coin,"side":"A","px":"98","sz":quantity,
                "time":time,"tid":id,
                "hash":"0x0000000000000000000000000000000000000000000000000000000000000000",
                "users":["0x1111111111111111111111111111111111111111","0x2222222222222222222222222222222222222222"]
            }]})
            .to_string(),
        ))
        .expect("send loopback trade");
}

pub fn read_rows(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .expect("read session rows")
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

pub fn count(rows: &[Value], kind: &str, variant: &str) -> usize {
    rows.iter()
        .filter(|row| row["kind"] == kind && row["variant"] == variant)
        .count()
}

fn checker() -> &'static Path {
    CHECKER.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let output = Command::new(env!("CARGO"))
            .current_dir(root)
            .args([
                "build",
                "--offline",
                "--message-format=json",
                "-p",
                "hl-backtest",
                "--bin",
                "hl-backtest",
            ])
            .output()
            .expect("build offline checker without a crate dependency");
        assert!(
            output.status.success(),
            "checker build failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("Cargo artifact messages are UTF-8")
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("Cargo JSON message"))
            .find_map(|message| {
                (message["target"]["name"] == "hl-backtest")
                    .then(|| message["executable"].as_str().map(PathBuf::from))
                    .flatten()
            })
            .expect("Cargo reported the checker executable")
    })
}

pub fn check_session(path: &Path) -> String {
    let output = Command::new(checker())
        .args(["run", "--check-session"])
        .arg(path)
        .output()
        .expect("check captured session offline");
    assert!(
        output.status.success(),
        "session check failed for {}: {}{}",
        path.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("checker output UTF-8")
}
