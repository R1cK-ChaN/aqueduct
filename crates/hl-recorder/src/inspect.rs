//! Reading recordings back, for checks and for people.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::File;
use std::io::{self, BufReader};
use std::path::Path;

use hl_codec::rawlog::{RecordKind, RecordReader};
use serde_json::Value;

/// Opens a recording file for reading record by record.
pub fn open_recording(
    path: &Path,
) -> io::Result<RecordReader<zstd::Decoder<'static, BufReader<File>>>> {
    Ok(RecordReader::new(zstd::Decoder::new(File::open(path)?)?))
}

/// A summary of one recording file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InspectReport {
    /// Number of records read.
    pub records: u64,
    /// Total payload bytes before compression.
    pub payload_bytes: u64,
    /// Wall clock of the first record.
    pub first_wall_ns: Option<i64>,
    /// Wall clock of the last record.
    pub last_wall_ns: Option<i64>,
    /// Record counts by kind.
    pub by_kind: BTreeMap<&'static str, u64>,
    /// Meta record counts by their `event` field.
    pub meta_events: BTreeMap<String, u64>,
    /// Received text frame counts by a best-effort label: the `channel`
    /// field (Hyperliquid) or the `e` field (Binance, BN-13) of the JSON payload.
    pub recv_text_labels: BTreeMap<String, u64>,
    /// Why reading stopped early, if it did. Expected for truncated files.
    pub error: Option<String>,
}

fn kind_name(kind: RecordKind) -> &'static str {
    match kind {
        RecordKind::Meta => "meta",
        RecordKind::RecvText => "recv_text",
        RecordKind::RecvBinary => "recv_binary",
        RecordKind::SentText => "sent_text",
    }
}

fn string_field(payload: &[u8], fields: &[&str]) -> Option<String> {
    let value: Value = serde_json::from_slice(payload).ok()?;
    fields
        .iter()
        .find_map(|field| value.get(field)?.as_str().map(str::to_owned))
}

/// Reads a whole recording and summarises it. A file that ends abruptly is
/// not an error: the report covers what could be read and says why reading
/// stopped.
pub fn inspect(path: &Path) -> io::Result<InspectReport> {
    let mut report = InspectReport::default();
    for record in open_recording(path)? {
        let record = match record {
            Ok(record) => record,
            Err(error) => {
                report.error = Some(error.to_string());
                break;
            }
        };
        report.records += 1;
        report.payload_bytes += record.payload.len() as u64;
        report.first_wall_ns.get_or_insert(record.recv_wall_ns);
        report.last_wall_ns = Some(record.recv_wall_ns);
        *report.by_kind.entry(kind_name(record.kind)).or_default() += 1;
        match record.kind {
            RecordKind::Meta => {
                let event = string_field(&record.payload, &["event"]);
                *report
                    .meta_events
                    .entry(event.unwrap_or_else(|| "?".to_owned()))
                    .or_default() += 1;
            }
            RecordKind::RecvText => {
                let label = string_field(&record.payload, &["channel", "e"]);
                *report
                    .recv_text_labels
                    .entry(label.unwrap_or_else(|| "?".to_owned()))
                    .or_default() += 1;
            }
            RecordKind::RecvBinary | RecordKind::SentText => {}
        }
    }
    Ok(report)
}

impl fmt::Display for InspectReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "records: {}", self.records)?;
        writeln!(f, "payload bytes: {}", self.payload_bytes)?;
        if let (Some(first), Some(last)) = (self.first_wall_ns, self.last_wall_ns) {
            let seconds = (last - first) / 1_000_000_000;
            writeln!(f, "wall clock: {first} to {last} ns ({seconds} s)")?;
        }
        writeln!(f, "by kind:")?;
        for (name, count) in &self.by_kind {
            writeln!(f, "  {name}: {count}")?;
        }
        writeln!(f, "meta events:")?;
        for (name, count) in &self.meta_events {
            writeln!(f, "  {name}: {count}")?;
        }
        writeln!(f, "received text frames:")?;
        for (name, count) in &self.recv_text_labels {
            writeln!(f, "  {name}: {count}")?;
        }
        match &self.error {
            Some(error) => writeln!(f, "stopped early: {error}"),
            None => writeln!(f, "ended cleanly"),
        }
    }
}
