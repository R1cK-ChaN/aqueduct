//! A synthetic recording shared by the loader and replay conformance tests.

use hl_codec::rawlog::RecordKind::{Meta, RecvText};
use hl_codec::rawlog::write_record;
use serde_json::{Value, json};

/// Expected `(tid, live)` rows in recording order.
pub const EXPECTED: [(u64, bool); 8] = [
    (11, true),
    (12, false),
    (13, true),
    (14, false),
    (15, true),
    (16, false),
    (17, false),
    (18, true),
];

pub fn recording() -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut record = |kind, wall_ms: u32, payload: Value| {
        write_record(
            &mut bytes,
            kind,
            i64::from(wall_ms) * 1_000_000,
            u64::from(wall_ms) * 1_000_000,
            &serde_json::to_vec(&payload).expect("encode synthetic payload"),
        )
        .expect("write synthetic record");
    };
    let trade = |tid: u64, time: u64| {
        json!({
            "coin": "BTC", "tid": tid, "time": time, "px": "101", "sz": "1", "side": "B",
            "hash": format!("0x{}", "00".repeat(32)),
            "users": [format!("0x{}", "01".repeat(20)), format!("0x{}", "02".repeat(20))],
        })
    };
    let trades = |rows: &[Value]| json!({"channel": "trades", "data": rows});
    record(
        Meta,
        0,
        json!({
            "event": "file_open", "format": 1, "url": "wss://api.hyperliquid.xyz/ws",
            "source": "synthetic", "process_start_wall_ns": 0, "file_seq": 2,
        }),
    );
    record(RecvText, 1, trades(&[trade(11, 0)]));
    record(Meta, 2, json!({"event": "connected"}));
    record(RecvText, 3, trades(&[trade(12, 1), trade(13, 2)]));
    record(Meta, 4, json!({"event": "dropped", "count": 1}));
    record(RecvText, 5, trades(&[trade(14, 1), trade(15, 4)]));
    record(Meta, 6, json!({"event": "disconnected"}));
    record(RecvText, 7, trades(&[trade(16, 7)]));
    record(Meta, 8, json!({"event": "connected"}));
    record(RecvText, 9, trades(&[trade(17, 7), trade(18, 8)]));
    bytes
}
