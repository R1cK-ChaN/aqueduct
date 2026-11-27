//! The record format of raw recordings.
//!
//! A recording, once decompressed, is a sequence of records. Each record is
//! a fixed header followed by a payload. All integers are little-endian.
//!
//! | Offset | Size | Field |
//! | --- | --- | --- |
//! | 0 | 4 | payload length, `u32` |
//! | 4 | 1 | kind, see [`RecordKind`] |
//! | 5 | 8 | wall clock at receipt, `i64` nanoseconds since the Unix epoch |
//! | 13 | 8 | monotonic clock, `u64` nanoseconds since the recorder started |
//! | 21 | n | payload |
//!
//! This module frames records and reads the recorder's `file_open` header.
//! Compression and file naming belong to the recorder. Venue parsers decode
//! frame payloads.
//! The reasons for the layout are in `docs/rfc/0002-raw-recording-format.md`.

use std::fmt;
use std::io::{self, Read, Write};

mod file_open;
pub use file_open::{ChainKey, ChainPosition, FileOpen, FileOpenError};

/// Version of the layout described in this module. Written into the
/// `file_open` record of every file.
pub const FORMAT_VERSION: u32 = 1;

/// Size of the fixed record header in bytes.
pub const HEADER_LEN: usize = 21;

/// Largest payload a writer will accept and a reader will allocate for.
/// Protects readers from a corrupt length field.
pub const MAX_PAYLOAD_LEN: usize = 64 * 1024 * 1024;

/// What a record holds. The numeric codes are part of the file format and
/// never change meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RecordKind {
    /// A JSON object written by the recorder itself. Always has an `event`
    /// field.
    Meta,
    /// A text frame received from the venue, UTF-8.
    RecvText,
    /// A binary frame received from the venue.
    RecvBinary,
    /// A text frame the recorder sent to the venue.
    SentText,
}

impl RecordKind {
    /// The code stored in the file.
    pub const fn code(self) -> u8 {
        match self {
            Self::Meta => 1,
            Self::RecvText => 2,
            Self::RecvBinary => 3,
            Self::SentText => 4,
        }
    }

    /// The kind for a stored code, or `None` for a code this version does
    /// not know.
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Meta),
            2 => Some(Self::RecvText),
            3 => Some(Self::RecvBinary),
            4 => Some(Self::SentText),
            _ => None,
        }
    }
}

/// One record read back from a recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// What the payload is.
    pub kind: RecordKind,
    /// Wall clock at receipt, nanoseconds since the Unix epoch.
    pub recv_wall_ns: i64,
    /// Monotonic clock at receipt, nanoseconds since the recorder started.
    pub recv_mono_ns: u64,
    /// The bytes exactly as received or sent.
    pub payload: Vec<u8>,
}

/// Writes one record.
///
/// # Errors
///
/// Returns `InvalidInput` when the payload is longer than
/// [`MAX_PAYLOAD_LEN`], and any error from `out`.
pub fn write_record<W: Write>(
    out: &mut W,
    kind: RecordKind,
    recv_wall_ns: i64,
    recv_mono_ns: u64,
    payload: &[u8],
) -> io::Result<()> {
    if payload.len() > MAX_PAYLOAD_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "record payload too large",
        ));
    }
    let payload_len = u32::try_from(payload.len()).expect("bounded by MAX_PAYLOAD_LEN");
    let mut header = [0_u8; HEADER_LEN];
    header[0..4].copy_from_slice(&payload_len.to_le_bytes());
    header[4] = kind.code();
    header[5..13].copy_from_slice(&recv_wall_ns.to_le_bytes());
    header[13..21].copy_from_slice(&recv_mono_ns.to_le_bytes());
    out.write_all(&header)?;
    out.write_all(payload)
}

/// Why reading a recording stopped early.
#[derive(Debug)]
pub enum ReadError {
    /// The underlying reader failed. A recording cut off inside a compressed
    /// block shows up here, as an error from the decompressor.
    Io(io::Error),
    /// The input ended in the middle of a record.
    Truncated,
    /// A record has a kind code this version does not know.
    UnknownKind(u8),
    /// A record claims a payload longer than [`MAX_PAYLOAD_LEN`].
    PayloadTooLarge(u32),
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "read failed: {error}"),
            Self::Truncated => f.write_str("input ends in the middle of a record"),
            Self::UnknownKind(code) => write!(f, "unknown record kind {code}"),
            Self::PayloadTooLarge(len) => write!(f, "record payload of {len} bytes is too large"),
        }
    }
}

impl std::error::Error for ReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

/// Reads records from decompressed recording bytes.
///
/// As an iterator it yields records until the input ends or an error occurs;
/// after an error it yields nothing more.
#[derive(Debug)]
pub struct RecordReader<R> {
    input: R,
    failed: bool,
}

/// Fills `buf` as far as the input allows and returns how many bytes were
/// read. Fewer than `buf.len()` means the input ended.
fn read_up_to<R: Read>(input: &mut R, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match input.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(filled)
}

impl<R: Read> RecordReader<R> {
    /// Wraps a reader positioned at the start of a record.
    pub fn new(input: R) -> Self {
        Self {
            input,
            failed: false,
        }
    }

    /// Reads the next record. `Ok(None)` means the input ended cleanly on a
    /// record boundary.
    pub fn read_record(&mut self) -> Result<Option<Record>, ReadError> {
        let mut header = [0_u8; HEADER_LEN];
        match read_up_to(&mut self.input, &mut header).map_err(ReadError::Io)? {
            0 => return Ok(None),
            HEADER_LEN => {}
            _ => return Err(ReadError::Truncated),
        }
        let payload_len = u32::from_le_bytes(header[0..4].try_into().expect("four bytes"));
        let kind = RecordKind::from_code(header[4]).ok_or(ReadError::UnknownKind(header[4]))?;
        let recv_wall_ns = i64::from_le_bytes(header[5..13].try_into().expect("eight bytes"));
        let recv_mono_ns = u64::from_le_bytes(header[13..21].try_into().expect("eight bytes"));

        let len = usize::try_from(payload_len).unwrap_or(usize::MAX);
        if len > MAX_PAYLOAD_LEN {
            return Err(ReadError::PayloadTooLarge(payload_len));
        }
        let mut payload = vec![0_u8; len];
        if read_up_to(&mut self.input, &mut payload).map_err(ReadError::Io)? != len {
            return Err(ReadError::Truncated);
        }
        Ok(Some(Record {
            kind,
            recv_wall_ns,
            recv_mono_ns,
            payload,
        }))
    }
}

impl<R: Read> Iterator for RecordReader<R> {
    type Item = Result<Record, ReadError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        let next = self.read_record().transpose();
        self.failed = matches!(next, Some(Err(_)));
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        let mut bytes = Vec::new();
        write_record(
            &mut bytes,
            RecordKind::Meta,
            1_000,
            5,
            br#"{"event":"file_open"}"#,
        )
        .unwrap();
        write_record(&mut bytes, RecordKind::RecvText, -7, u64::MAX, b"").unwrap();
        write_record(
            &mut bytes,
            RecordKind::RecvBinary,
            i64::MAX,
            0,
            &[0, 255, 10, 13],
        )
        .unwrap();
        write_record(
            &mut bytes,
            RecordKind::SentText,
            2_000,
            9,
            "ping \u{1F600}".as_bytes(),
        )
        .unwrap();
        bytes
    }

    fn read_all(bytes: &[u8]) -> Vec<Result<Record, ReadError>> {
        RecordReader::new(bytes).collect()
    }

    #[test]
    fn header_layout_is_fixed() {
        let mut bytes = Vec::new();
        write_record(
            &mut bytes,
            RecordKind::RecvText,
            0x0102_0304_0506_0708,
            0x1112,
            b"ab",
        )
        .unwrap();
        assert_eq!(
            bytes,
            [
                2, 0, 0, 0, // payload length
                2, // kind
                8, 7, 6, 5, 4, 3, 2, 1, // wall clock
                0x12, 0x11, 0, 0, 0, 0, 0, 0, // monotonic clock
                b'a', b'b',
            ]
        );
        assert_eq!(bytes.len(), HEADER_LEN + 2);
    }

    #[test]
    fn kind_codes_are_stable() {
        let kinds = [
            (RecordKind::Meta, 1),
            (RecordKind::RecvText, 2),
            (RecordKind::RecvBinary, 3),
            (RecordKind::SentText, 4),
        ];
        for (kind, code) in kinds {
            assert_eq!(kind.code(), code);
            assert_eq!(RecordKind::from_code(code), Some(kind));
        }
        assert_eq!(RecordKind::from_code(0), None);
        assert_eq!(RecordKind::from_code(5), None);
    }

    #[test]
    fn records_round_trip_exactly() {
        let records: Vec<Record> = read_all(&sample())
            .into_iter()
            .map(Result::unwrap)
            .collect();
        assert_eq!(records.len(), 4);
        assert_eq!(
            records[0],
            Record {
                kind: RecordKind::Meta,
                recv_wall_ns: 1_000,
                recv_mono_ns: 5,
                payload: br#"{"event":"file_open"}"#.to_vec(),
            }
        );
        assert_eq!(
            (records[1].recv_wall_ns, records[1].recv_mono_ns),
            (-7, u64::MAX)
        );
        assert!(records[1].payload.is_empty());
        assert_eq!(records[2].payload, [0, 255, 10, 13]);
        assert_eq!(records[3].payload, "ping \u{1F600}".as_bytes());
    }

    #[test]
    fn empty_input_has_no_records() {
        assert!(read_all(&[]).is_empty());
    }

    #[test]
    fn input_cut_inside_a_header_or_payload_is_truncated() {
        let bytes = sample();
        let first_len = HEADER_LEN + br#"{"event":"file_open"}"#.len();

        for cut in [first_len - 1, first_len + 3] {
            let results = read_all(&bytes[..cut]);
            assert!(
                matches!(results.last(), Some(Err(ReadError::Truncated))),
                "cut at {cut}"
            );
        }
        // Everything before the cut is still returned.
        let results = read_all(&bytes[..first_len + 3]);
        assert_eq!(results.len(), 2);
        assert!(results[0].is_ok());
    }

    #[test]
    fn unknown_kind_stops_the_reader() {
        let mut bytes = sample();
        let second_record = HEADER_LEN + br#"{"event":"file_open"}"#.len();
        bytes[second_record + 4] = 99;

        let results = read_all(&bytes);
        assert_eq!(results.len(), 2, "nothing is yielded after the error");
        assert!(matches!(results[1], Err(ReadError::UnknownKind(99))));
    }

    #[test]
    fn oversized_length_is_rejected_without_allocating() {
        let mut bytes = vec![0_u8; HEADER_LEN];
        bytes[0..4].copy_from_slice(&u32::MAX.to_le_bytes());
        bytes[4] = RecordKind::RecvText.code();
        assert!(matches!(
            read_all(&bytes)[0],
            Err(ReadError::PayloadTooLarge(u32::MAX))
        ));
    }

    #[test]
    fn writer_rejects_an_oversized_payload() {
        let payload = vec![0_u8; MAX_PAYLOAD_LEN + 1];
        let error = write_record(&mut Vec::new(), RecordKind::RecvBinary, 0, 0, &payload);
        assert_eq!(error.unwrap_err().kind(), io::ErrorKind::InvalidInput);
    }
}
