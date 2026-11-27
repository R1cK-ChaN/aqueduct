use std::fmt;

use serde_json::Value;

use super::{FORMAT_VERSION, Record, RecordKind};

/// All files written for one source by one recorder process (RFC 0002 D2).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ChainKey {
    /// Recorder source name.
    pub source: String,
    /// Wall-clock nanoseconds at recorder process start.
    pub process_start_wall_ns: i64,
}

/// A file's position in the recording chain. Legacy files can omit any
/// part of the position, including the process anchor used by replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainPosition {
    /// Recorder source name, when supplied.
    pub source: Option<String>,
    /// Wall-clock nanoseconds at recorder process start, when supplied.
    pub process_start_wall_ns: Option<i64>,
    /// Sequence within the chain, absent for legacy files.
    pub file_seq: Option<u64>,
}

impl ChainPosition {
    /// Identifies the chain whenever both fields are present, including in
    /// an unnumbered header.
    pub fn key(&self) -> Option<ChainKey> {
        Some(ChainKey {
            source: self.source.clone()?,
            process_start_wall_ns: self.process_start_wall_ns?,
        })
    }
}

/// The recorder's opening metadata and the receipt stamps on that record.
/// Readers can identify the feed by URL before interpreting its chain.
#[derive(Debug)]
pub struct FileOpen {
    meta: Value,
    /// Wall-clock nanoseconds when the file opened.
    pub opened_wall_ns: i64,
    /// Process-relative nanoseconds when the file opened.
    pub opened_mono_ns: u64,
}

impl FileOpen {
    /// Reads a `file_open` meta record (RFC 0002 D2).
    pub fn parse(record: &Record) -> Result<Self, FileOpenError> {
        let meta: Value = serde_json::from_slice(&record.payload).map_err(FileOpenError::Json)?;
        if record.kind != RecordKind::Meta
            || meta.get("event").and_then(Value::as_str) != Some("file_open")
        {
            return Err(FileOpenError::NotFileOpen);
        }
        Ok(Self {
            meta,
            opened_wall_ns: record.recv_wall_ns,
            opened_mono_ns: record.recv_mono_ns,
        })
    }

    /// The recorded URL, when the header supplies a string.
    pub fn url(&self) -> Option<&str> {
        self.meta.get("url").and_then(Value::as_str)
    }

    /// Returns the declared layout version, rejecting unsupported versions.
    /// An absent field is accepted for compatibility with older headers.
    pub fn format_version(&self) -> Result<Option<u32>, FileOpenError> {
        self.field("format", |value| {
            (value.as_u64() == Some(u64::from(FORMAT_VERSION))).then_some(FORMAT_VERSION)
        })
    }

    /// Reads the chain fields. A numbered file requires both a source
    /// name and a process start. Legacy files keep either field they supply.
    pub fn chain_position(&self) -> Result<ChainPosition, FileOpenError> {
        let source = self.field("source", |value| {
            value.as_str().filter(|s| !s.is_empty()).map(str::to_owned)
        })?;
        let process_start_wall_ns = self.field("process_start_wall_ns", Value::as_i64)?;
        let file_seq = self.field("file_seq", Value::as_u64)?;
        if file_seq.is_some() && (source.is_none() || process_start_wall_ns.is_none()) {
            return Err(FileOpenError::MissingChainKey);
        }
        Ok(ChainPosition {
            source,
            process_start_wall_ns,
            file_seq,
        })
    }

    fn field<T>(
        &self,
        name: &'static str,
        parse: impl FnOnce(&Value) -> Option<T>,
    ) -> Result<Option<T>, FileOpenError> {
        self.meta
            .get(name)
            .map(|value| parse(value).ok_or(FileOpenError::InvalidField(name)))
            .transpose()
    }
}

/// Why a `file_open` record or one of its fields could not be read.
#[derive(Debug)]
pub enum FileOpenError {
    /// The record's payload could not be decoded as JSON.
    Json(serde_json::Error),
    /// The record is not the opening metadata event.
    NotFileOpen,
    /// The named field has an invalid value.
    InvalidField(&'static str),
    /// A numbered file omits its source or process start.
    MissingChainKey,
}

impl fmt::Display for FileOpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => write!(f, "invalid file_open JSON: {error}"),
            Self::NotFileOpen => f.write_str("first record is not file_open"),
            Self::InvalidField(name) => write!(f, "invalid file_open {name}"),
            Self::MissingChainKey => {
                f.write_str("numbered file_open needs source and process_start_wall_ns")
            }
        }
    }
}

impl std::error::Error for FileOpenError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(meta: Value) -> FileOpen {
        FileOpen::parse(&Record {
            kind: RecordKind::Meta,
            payload: meta.to_string().into_bytes(),
            recv_wall_ns: -77,
            recv_mono_ns: 350,
        })
        .expect("file_open")
    }

    #[test]
    fn numbered_header_preserves_chain_sequence_url_and_open_stamps() {
        let open = parse(json!({
            "event": "file_open", "format": 1, "source": "test-feed",
            "process_start_wall_ns": -99, "file_seq": u64::MAX,
            "url": "wss://example.invalid/ws", "extra": []
        }));
        assert_eq!(open.format_version().expect("known format"), Some(1));
        assert_eq!(open.url(), Some("wss://example.invalid/ws"));
        assert_eq!((open.opened_wall_ns, open.opened_mono_ns), (-77, 350));
        let position = open.chain_position().expect("numbered file");
        assert_eq!(position.file_seq, Some(u64::MAX));
        assert_eq!(
            position.key(),
            Some(ChainKey {
                source: "test-feed".to_owned(),
                process_start_wall_ns: -99
            })
        );
    }

    #[test]
    fn legacy_header_keeps_partial_identity_but_numbered_files_require_both_fields() {
        for (source, start) in [
            (None, None),
            (Some("feed"), None),
            (None, Some(-6)),
            (Some("feed"), Some(-6)),
        ] {
            let mut meta = json!({"event": "file_open"});
            if let Some(source) = source {
                meta["source"] = json!(source);
            }
            if let Some(start) = start {
                meta["process_start_wall_ns"] = json!(start);
            }
            let open = parse(meta.clone());
            assert_eq!(open.format_version().expect("absent version"), None);
            assert_eq!(open.url(), None);
            let legacy = open.chain_position().expect("legacy file");
            assert_eq!(legacy.source.as_deref(), source);
            assert_eq!(legacy.process_start_wall_ns, start);
            assert_eq!(legacy.file_seq, None);
            let key = source
                .zip(start)
                .map(|(source, process_start_wall_ns)| ChainKey {
                    source: source.to_owned(),
                    process_start_wall_ns,
                });
            assert_eq!(legacy.key(), key);

            meta["file_seq"] = json!(0);
            let numbered = parse(meta).chain_position();
            if let Some(key) = key {
                assert_eq!(numbered.expect("numbered file").key(), Some(key));
            } else {
                assert!(matches!(numbered, Err(FileOpenError::MissingChainKey)));
            }
        }
    }

    #[test]
    fn format_check_rejects_unknown_or_malformed_versions() {
        for version in [json!(2), json!("1"), Value::Null] {
            let open = parse(json!({"event": "file_open", "format": version}));
            assert!(matches!(
                open.format_version(),
                Err(FileOpenError::InvalidField("format"))
            ));
        }
    }
}
