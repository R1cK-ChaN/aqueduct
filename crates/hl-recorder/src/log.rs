//! Operational log: one JSON object per line on stderr, for journald.

use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};

/// Writes one log line. `fields` must be a JSON object; its entries are
/// merged into the line.
pub(crate) fn emit(level: &str, event: &str, fields: Value) {
    let unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        });
    let mut line = Map::new();
    line.insert("ts_ms".to_owned(), json!(unix_ms));
    line.insert("level".to_owned(), json!(level));
    line.insert("event".to_owned(), json!(event));
    if let Value::Object(fields) = fields {
        line.extend(fields);
    }
    // A failed write to stderr has nowhere to be reported.
    let _ = writeln!(std::io::stderr().lock(), "{}", Value::Object(line));
}

pub(crate) fn info(event: &str, fields: Value) {
    emit("info", event, fields);
}

pub(crate) fn warn(event: &str, fields: Value) {
    emit("warn", event, fields);
}

pub(crate) fn error(event: &str, fields: Value) {
    emit("error", event, fields);
}
