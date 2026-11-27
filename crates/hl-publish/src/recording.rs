//! Check the compressed bytes and retain only observations from a complete recording.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::{self, BufRead, BufReader, Cursor, Read};

use hl_codec::hyperliquid::{WsFrame, decode_ws_frame};
use hl_codec::rawlog::{
    ChainKey, FORMAT_VERSION, FileOpen, ReadError, Record, RecordKind, RecordReader,
};
use hl_recorder::public_v1;
use serde::de::{Error as _, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

const ZSTD_ERROR: &str = "invalid_zstd";
const RECORD_ERROR: &str = "invalid_record";
const OPEN_ERROR: &str = "invalid_file_open";
const META_ERROR: &str = "invalid_meta";
const FRAME_ERROR: &str = "invalid_received_frame";

#[derive(Debug)]
pub(crate) struct Recording {
    pub chain: ChainKey,
    pub file_seq: u64,
    pub first_wall_ns: i64,
    pub last_wall_ns: i64,
    pub size_bytes: u64,
    pub sha256: String,
    pub coins_covered: BTreeMap<String, BTreeSet<&'static str>>,
    pub lifecycle: Vec<Lifecycle>,
}

#[derive(Debug)]
pub(crate) struct Lifecycle {
    pub at_ns: i64,
    pub event: &'static str,
    pub reason: Option<DisconnectReason>,
    pub dropped: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DisconnectReason {
    ServerClosed,
    Stale,
    SendFailed,
    ConnectionFailed,
    Other,
}

/// Hash the exact bytes fed to the decoder, including the frame header and checksum.
struct CompressedBytes<R> {
    input: R,
    size: u64,
    hash: Sha256,
}

impl<R: Read> Read for CompressedBytes<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // A read failure must reach the publisher even if a framing helper
        // would normally retry that error kind.
        let n = self
            .input
            .read(buf)
            .map_err(|_| io::Error::other(ZSTD_ERROR))?;
        self.hash.update(&buf[..n]);
        self.size += n as u64;
        Ok(n)
    }
}

pub(crate) fn inspect(
    input: impl Read,
    deny: &[String],
    rejected: &mut BTreeSet<String>,
) -> Result<Recording, &'static str> {
    // RFC 0002 D3 requires a standard zstd frame with Content_Checksum_Flag
    // set in its frame header descriptor. The decoder verifies the checksum.
    let mut compressed = CompressedBytes {
        input,
        size: 0,
        hash: Sha256::new(),
    };
    let mut header = [0; 5];
    compressed.read_exact(&mut header).map_err(|_| ZSTD_ERROR)?;
    if header[..4] != zstd::zstd_safe::MAGICNUMBER.to_le_bytes() || header[4] & 0x04 == 0 {
        return Err(ZSTD_ERROR);
    }
    let mut decoder = zstd::Decoder::new(Cursor::new(header).chain(&mut compressed))
        .map_err(|_| ZSTD_ERROR)?
        .single_frame();
    let mut records = RecordReader::new(BufReader::new(&mut decoder));
    let first = records
        .read_record()
        .map_err(record_error)?
        .ok_or(OPEN_ERROR)?;
    let mut result = open_recording(&first, deny);
    if first.kind == RecordKind::SentText {
        // The opening check already withheld this file for its missing header.
        let _ = screen_send(&first.payload, rejected);
    }
    for record in records {
        let record = record.map_err(record_error)?;
        // Even a withheld file can contain another send that must be kept out
        // of the day's report and manifest. Only eligible files add observations.
        let check = match record.kind {
            RecordKind::SentText => screen_send(&record.payload, rejected),
            _ => Ok(()),
        };
        if let Ok(file) = &mut result
            && let Err(reason) = check.and_then(|()| file.observe(&record, deny))
        {
            result = Err(reason);
        }
    }
    decoder.finish_frame().map_err(|_| ZSTD_ERROR)?;
    let mut input = decoder.into_inner();
    // A second frame (even an empty/skippable one) or arbitrary trailing data
    // would be additional bytes published under the verified frame's name.
    if !input.fill_buf().map_err(|_| ZSTD_ERROR)?.is_empty() {
        return Err(ZSTD_ERROR);
    }
    let mut result = result?;
    result.size_bytes = compressed.size;
    result.sha256 = data_encoding::HEXLOWER.encode(&compressed.hash.finalize());
    Ok(result)
}

fn record_error(error: ReadError) -> &'static str {
    match error {
        ReadError::Io(_) => ZSTD_ERROR,
        _ => RECORD_ERROR,
    }
}

fn open_recording(first: &Record, deny: &[String]) -> Result<Recording, &'static str> {
    if first.kind != RecordKind::Meta {
        return Err(OPEN_ERROR);
    }
    meta_fields(first, deny)?;
    let open = FileOpen::parse(first).map_err(|_| OPEN_ERROR)?;
    if open.format_version().map_err(|_| OPEN_ERROR)? != Some(FORMAT_VERSION) {
        return Err(OPEN_ERROR);
    }
    let position = open.chain_position().map_err(|_| OPEN_ERROR)?;
    let key = position.key().ok_or(OPEN_ERROR)?;
    if key.source != public_v1::SOURCE_NAME {
        return Err("wrong_source");
    }
    if open.url() != Some(public_v1::URL) {
        return Err("wrong_url");
    }
    Ok(Recording {
        chain: key,
        file_seq: position.file_seq.ok_or(OPEN_ERROR)?,
        first_wall_ns: first.recv_wall_ns,
        last_wall_ns: first.recv_wall_ns,
        size_bytes: 0,
        sha256: String::new(),
        coins_covered: BTreeMap::new(),
        lifecycle: Vec::new(),
    })
}

impl Recording {
    fn observe(&mut self, record: &Record, deny: &[String]) -> Result<(), &'static str> {
        self.last_wall_ns = record.recv_wall_ns;
        match record.kind {
            RecordKind::Meta => {
                let fields = meta_fields(record, deny)?;
                if let Some(event) = lifecycle(&fields, record.recv_wall_ns)? {
                    self.lifecycle.push(event);
                }
            }
            RecordKind::RecvText => observe_frame(&record.payload, &mut self.coins_covered)?,
            RecordKind::RecvBinary | RecordKind::SentText => {}
        }
        Ok(())
    }
}

fn screen_send(payload: &[u8], rejected: &mut BTreeSet<String>) -> Result<(), &'static str> {
    let text = str::from_utf8(payload).map_err(|_| "rejected_send")?;
    if public_v1::allows(text) {
        return Ok(());
    }
    if !text.is_empty() && !rejected.contains(text) {
        rejected.insert(text.to_owned());
    }
    Err("rejected_send")
}

/// A second top-level field could shadow the event, URL or chain used for
/// disclosure checks. Serde's ordinary map deserializer keeps the last value.
struct MetaFields(BTreeMap<String, Value>);

impl<'de> Deserialize<'de> for MetaFields {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        struct FieldsVisitor;
        impl<'de> Visitor<'de> for FieldsVisitor {
            type Value = MetaFields;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a metadata object with unique fields")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut fields = BTreeMap::new();
                while let Some((key, value)) = map.next_entry()? {
                    if fields.insert(key, value).is_some() {
                        return Err(A::Error::custom("duplicate metadata field"));
                    }
                }
                Ok(MetaFields(fields))
            }
        }
        de.deserialize_map(FieldsVisitor)
    }
}

/// Check literal bytes and decoded string tokens in JSON, including object
/// keys and earlier values of repeated keys. Callers validate JSON separately.
pub(crate) fn json_contains_denied(text: &str, deny: &[String]) -> bool {
    if deny.is_empty() {
        return false;
    }
    if deny.iter().any(|needle| text.contains(needle)) {
        return true;
    }
    if !text.contains('\\') {
        return false;
    }
    let mut tail = text;
    while let Some(start) = tail.find('"') {
        let mut token = serde_json::Deserializer::from_str(&tail[start..]).into_iter::<String>();
        let Some(Ok(decoded)) = token.next() else {
            return true;
        };
        if deny.iter().any(|needle| decoded.contains(needle)) {
            return true;
        }
        tail = &tail[start + token.byte_offset()..];
    }
    false
}

fn meta_fields(record: &Record, deny: &[String]) -> Result<BTreeMap<String, Value>, &'static str> {
    let text = str::from_utf8(&record.payload).map_err(|_| META_ERROR)?;
    if json_contains_denied(text, deny) {
        return Err("denied_meta");
    }
    let fields: MetaFields = serde_json::from_str(text).map_err(|_| META_ERROR)?;
    Ok(fields.0)
}

fn lifecycle(
    fields: &BTreeMap<String, Value>,
    at_ns: i64,
) -> Result<Option<Lifecycle>, &'static str> {
    let text = |key| {
        fields
            .get(key)
            .map(|value| value.as_str().ok_or(META_ERROR))
            .transpose()
    };
    let mut reason = None;
    let mut dropped = None;
    let event = match text("event")?.ok_or(META_ERROR)? {
        "file_open" => return Err(OPEN_ERROR),
        "connecting" => return Ok(None),
        "connected" => "connected",
        "disconnected" => {
            reason = text("reason")?.map(disconnect_reason);
            "disconnected"
        }
        "shutdown" => "shutdown",
        "connect_failed" => {
            reason = text("error")?.map(|_| DisconnectReason::ConnectionFailed);
            "connect_failed"
        }
        "send_policy_rejected" => "send_policy_rejected",
        "dropped" => {
            dropped = Some(
                fields
                    .get("count")
                    .and_then(Value::as_u64)
                    .ok_or(META_ERROR)?,
            );
            "dropped"
        }
        _ => return Err("unknown_meta"),
    };
    Ok(Some(Lifecycle {
        at_ns,
        event,
        reason,
        dropped,
    }))
}

fn disconnect_reason(reason: &str) -> DisconnectReason {
    if reason == "closed by the server" || reason.starts_with("closed by the server: ") {
        DisconnectReason::ServerClosed
    } else if reason.starts_with("no frame for ") && reason.ends_with(" ms") {
        DisconnectReason::Stale
    } else if reason.starts_with("send failed: ") {
        DisconnectReason::SendFailed
    } else {
        DisconnectReason::Other
    }
}

fn observe_frame(
    payload: &[u8],
    coverage: &mut BTreeMap<String, BTreeSet<&'static str>>,
) -> Result<(), &'static str> {
    let mut observed = |coin: String, channel| {
        if !public_v1::COINS.contains(&coin.as_str()) {
            return Err(FRAME_ERROR);
        }
        coverage.entry(coin).or_default().insert(channel);
        Ok(())
    };
    match decode_ws_frame(payload).map_err(|_| FRAME_ERROR)? {
        WsFrame::Book(book) => {
            // The codec returns Book for both l2 snapshots and l2Book.
            #[derive(Deserialize)]
            struct Envelope {
                channel: String,
            }
            let frame: Envelope = serde_json::from_slice(payload).map_err(|_| FRAME_ERROR)?;
            if frame.channel != "l2" {
                return Err(FRAME_ERROR);
            }
            observed(book.coin, "l2")?;
        }
        WsFrame::BookDiff(diff) => observed(diff.coin, "l2")?,
        WsFrame::Bbo(bbo) => observed(bbo.coin, "bbo")?,
        WsFrame::Trades(trades) => {
            for trade in trades {
                observed(trade.coin, "trades")?;
            }
        }
        WsFrame::Other { channel } if channel == "activeAssetCtx" => {
            // The codec identifies this channel but leaves its data opaque.
            // HL-91 in docs/venues/hyperliquid.md describes the envelope.
            #[derive(Deserialize)]
            struct Envelope {
                data: Context,
            }
            #[derive(Deserialize)]
            struct Context {
                coin: String,
                #[serde(rename = "ctx")]
                _context: serde_json::Map<String, Value>,
            }
            let frame: Envelope = serde_json::from_slice(payload).map_err(|_| FRAME_ERROR)?;
            observed(frame.data.coin, "activeAssetCtx")?;
        }
        // D2 keeps the shared context intact. Other unknown data channels
        // have no reviewed place in this public source.
        WsFrame::Other { channel } if channel == "fastAssetCtxs" => {}
        WsFrame::Other { .. } => return Err(FRAME_ERROR),
        WsFrame::SubscriptionResponse | WsFrame::Pong | WsFrame::Error(_) => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hl_codec::rawlog::write_record;
    use serde_json::json;
    use std::io::Write;

    fn compressed_records(records: &[(RecordKind, &[u8])]) -> Vec<u8> {
        let mut encoder = zstd::Encoder::new(Vec::new(), 3).expect("create encoder");
        encoder.include_checksum(true).expect("set checksum flag");
        for (index, &(kind, payload)) in records.iter().enumerate() {
            write_record(&mut encoder, kind, index as i64 + 1, index as u64, payload)
                .expect("write synthetic record");
        }
        encoder.flush().expect("flush records");
        encoder.finish().expect("finish frame")
    }

    fn sample(kind: RecordKind, payload: &[u8]) -> Vec<u8> {
        let header = json!({
            "event": "file_open", "format": FORMAT_VERSION, "source": public_v1::SOURCE_NAME,
            "url": public_v1::URL, "file_seq": 0, "process_start_wall_ns": 1,
            "recorder_version": "synthetic"
        });
        compressed_records(&[
            (RecordKind::Meta, header.to_string().as_bytes()),
            (kind, payload),
        ])
    }

    #[test]
    fn physical_eof_read_failure_withholds_a_complete_checksummed_frame() {
        struct FailOnce(bool);
        impl Read for FailOnce {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                if std::mem::take(&mut self.0) {
                    Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "synthetic private message",
                    ))
                } else {
                    Ok(0)
                }
            }
        }
        let data = sample(RecordKind::SentText, public_v1::PING.as_bytes());
        let good = inspect(data.as_slice(), &[], &mut BTreeSet::new()).expect("clean EOF");
        assert_eq!(good.size_bytes, data.len() as u64);
        assert_eq!(
            good.sha256,
            data_encoding::HEXLOWER.encode(&Sha256::digest(&data))
        );
        assert_eq!(
            inspect(
                data.as_slice().chain(FailOnce(true)),
                &[],
                &mut BTreeSet::new()
            )
            .expect_err("failed EOF"),
            ZSTD_ERROR
        );
    }

    #[test]
    fn escaped_denied_keys_and_shadowed_values_are_screened() {
        let deny = vec!["private-id".to_owned()];
        for payload in [
            br#"{"event":"connected","tags":{"priv\u0061te-id":1}}"#.as_slice(),
            br#"{"event":"connected","tags":{"label":"priv\u0061te-id","label":"public"}}"#,
        ] {
            let data = sample(RecordKind::Meta, payload);
            assert_eq!(
                inspect(data.as_slice(), &deny, &mut BTreeSet::new()).expect_err("denied"),
                "denied_meta"
            );
        }
        let data = sample(
            RecordKind::Meta,
            br#"{"event":"future","e\u0076ent":"connected"}"#,
        );
        assert_eq!(
            inspect(data.as_slice(), &[], &mut BTreeSet::new()).expect_err("ambiguous event"),
            META_ERROR
        );
    }

    #[test]
    fn screened_sends_include_the_first_record_and_later_records_of_a_withheld_file() {
        use RecordKind::{Meta, SentText};
        let data = compressed_records(&[
            (SentText, b"synthetic-first"),
            (Meta, br#"{"event":"future"}"#),
            (SentText, b"synthetic-second"),
            (SentText, b"synthetic-first"),
            (SentText, public_v1::PING.as_bytes()),
        ]);
        let mut rejected = BTreeSet::new();
        assert_eq!(
            inspect(data.as_slice(), &[], &mut rejected).expect_err("missing header"),
            OPEN_ERROR
        );
        assert_eq!(
            rejected,
            BTreeSet::from(["synthetic-first".to_owned(), "synthetic-second".to_owned()])
        );
    }

    #[test]
    fn malformed_received_frames_and_unexpected_individual_coins_withhold() {
        for payload in [
            b"\xff".as_slice(),
            br#"{"channel":"activeAssetCtx","data":{"coin":"BTC"}}"#,
            br#"{"channel":"bbo","data":{"coin":"OTHER","time":1,"bbo":[null,null]}}"#,
            br#"{"channel":"l2Book","data":{"coin":"BTC","time":1,"levels":[[],[]]}}"#,
            br#"{"channel":"unreviewed-channel","data":{"coin":"BTC"}}"#,
        ] {
            let data = sample(RecordKind::RecvText, payload);
            assert_eq!(
                inspect(data.as_slice(), &[], &mut BTreeSet::new()).expect_err("invalid coverage"),
                FRAME_ERROR
            );
        }
    }
}
