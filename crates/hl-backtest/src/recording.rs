//! Read recorder files into the receive-ordered input accepted by [`hl_run::replay`].
//!
//! File boundaries follow RFC 0002 D2. The trade cutoff follows HL-81 in
//! `docs/venues/hyperliquid.md`.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::File;
use std::io::{self, BufReader};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use hl_codec::hyperliquid::Network;
use hl_codec::rawlog::{ChainKey, ChainPosition, FileOpen, ReadError, RecordReader};
use hl_run::recording::{EventOrigin, GapCause, RawPosition, ReaderStats, RecordConverter};
use hl_types::{MarketEvent, Timestamp};

/// Receive-ordered market events and the counts that describe their conversion.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Recording {
    /// Market events for the requested coin, in file and record order.
    pub events: Vec<MarketEvent>,
    /// Raw record location (and gap cause, when present) of each market event.
    pub origins: Vec<EventOrigin>,
    /// Files in the order the converter actually consumed them and their header sequences.
    pub files: Vec<(PathBuf, Option<u64>)>,
    /// Descriptor identities verified during decoding, in the same order as `files`.
    pub file_stamps: Vec<RawFileStamp>,
    /// Final complete record, including a header or an ignored frame.
    pub last_position: Option<RawPosition>,
    /// Reader counters for the complete input set.
    pub stats: ReaderStats,
    /// Inclusive local cutoff of the final complete record, including a file
    /// header or a record that produced no market event. None for no files.
    pub stop_time: Option<Timestamp>,
}

/// Identity and change metadata of one sealed raw file on Unix.
///
/// Captured from the descriptor that supplied the bytes. The change timestamp
/// detects in-place writes even when size and modification time are restored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawFileStamp {
    device: u64,
    inode: u64,
    length: u64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

impl RawFileStamp {
    fn capture(file: &File) -> io::Result<Self> {
        let metadata = file.metadata()?;
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            length: metadata.len(),
            changed_seconds: metadata.ctime(),
            changed_nanoseconds: metadata.ctime_nsec(),
        })
    }
}

struct VerifiedRawFile {
    file: File,
    stamp: RawFileStamp,
}

impl VerifiedRawFile {
    fn open(path: &Path, expected: Option<&RawFileStamp>) -> io::Result<Self> {
        let file = File::open(path)?;
        let stamp = RawFileStamp::capture(&file)?;
        if expected.is_some_and(|expected| *expected != stamp) {
            return Err(io::Error::other("raw file changed between reads"));
        }
        Ok(Self { file, stamp })
    }

    fn verify(&self) -> io::Result<()> {
        if RawFileStamp::capture(&self.file)? != self.stamp {
            return Err(io::Error::other("raw file changed while being read"));
        }
        Ok(())
    }
}

/// Hash the same sealed file that supplied an earlier decoded recording.
pub fn sha256_file_checked(path: &Path, expected: &RawFileStamp) -> io::Result<String> {
    sha256_file_stamped(path, Some(expected)).map(|(digest, _)| digest)
}

pub(crate) fn sha256_file_stamped(
    path: &Path,
    expected: Option<&RawFileStamp>,
) -> io::Result<(String, RawFileStamp)> {
    let raw = VerifiedRawFile::open(path, expected)?;
    // Give sha256sum an open file as stdin. No path is interpreted as an option
    // or emitted with escaping in the checksum output.
    let output = Command::new("sha256sum")
        .stdin(raw.file.try_clone()?)
        .output()?;
    raw.verify()?;
    if !output.status.success() {
        return Err(io::Error::other(String::from_utf8_lossy(&output.stderr)));
    }
    let text = std::str::from_utf8(&output.stdout).map_err(io::Error::other)?;
    let digest = text.split_ascii_whitespace().next().unwrap_or("");
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(io::Error::other("invalid digest from sha256sum"));
    }
    Ok((digest.to_owned(), raw.stamp))
}

/// An unreadable recording or an invalid recording chain.
#[derive(Debug)]
pub struct RecordingError {
    path: PathBuf,
    message: String,
}

impl RecordingError {
    fn new(path: &Path, message: impl fmt::Display) -> Self {
        Self {
            path: path.to_owned(),
            message: message.to_string(),
        }
    }
}

impl fmt::Display for RecordingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.message)
    }
}

impl std::error::Error for RecordingError {}

#[derive(Debug, PartialEq, Eq)]
struct Header {
    path: PathBuf,
    stamp: RawFileStamp,
    position: ChainPosition,
    opened_wall_ns: i64,
    opened_mono_ns: u64,
    network: Network,
}

impl Header {
    // Each unnumbered file is a separate replay chain, even with a known key.
    fn numbered_chain(&self) -> Option<ChainKey> {
        self.position.file_seq?;
        self.position.key()
    }

    fn same_process(&self, other: &Self) -> bool {
        self.numbered_chain()
            .is_some_and(|key| other.numbered_chain() == Some(key))
    }
}

type FileReader<'a> = RecordReader<zstd::stream::read::Decoder<'static, BufReader<&'a File>>>;

fn open_reader<'a>(path: &Path, file: &'a File) -> Result<FileReader<'a>, RecordingError> {
    let decoder =
        zstd::stream::read::Decoder::new(file).map_err(|error| RecordingError::new(path, error))?;
    Ok(RecordReader::new(decoder))
}

fn read_header(
    path: &Path,
    input: &mut FileReader<'_>,
    stamp: RawFileStamp,
    paper_network: Option<Network>,
) -> Result<Header, RecordingError> {
    let record = input
        .read_record()
        .map_err(|error| RecordingError::new(path, error))?
        .ok_or_else(|| RecordingError::new(path, "no file_open record"))?;
    let invalid = |error| RecordingError::new(path, error);
    let open = FileOpen::parse(&record).map_err(invalid)?;
    open.format_version().map_err(invalid)?;
    let position = open.chain_position().map_err(invalid)?;
    let url = open
        .url()
        .ok_or_else(|| RecordingError::new(path, "invalid file_open url"))?;
    // Paper supplies the configured network independently of the transport
    // address (including loopback and proxied feeds). Standalone raw files
    // still need a recognized URL to identify the recorded network.
    let network = paper_network
        .or_else(|| Network::from_url(url))
        .ok_or_else(|| RecordingError::new(path, "unsupported recording URL"))?;
    Ok(Header {
        path: path.to_owned(),
        stamp,
        position,
        opened_wall_ns: open.opened_wall_ns,
        opened_mono_ns: open.opened_mono_ns,
        network,
    })
}

fn order_headers(headers: Vec<Header>) -> Vec<Header> {
    let file_count = headers.len();
    let mut numbered: BTreeMap<_, Vec<Header>> = BTreeMap::new();
    let mut chains = Vec::new();
    for header in headers {
        if let Some(key) = header.numbered_chain() {
            numbered.entry(key).or_default().push(header);
        } else {
            chains.push(vec![header].into_iter());
        }
    }
    for mut chain in numbered.into_values() {
        chain.sort_by_key(|header| header.position.file_seq);
        chains.push(chain.into_iter());
    }

    // Only each chain's next file competes by wall time. A backward wall-clock
    // step can never move a numbered file ahead of its sequence predecessor.
    let mut heads = BTreeMap::new();
    for (chain_index, chain) in chains.iter_mut().enumerate() {
        if let Some(header) = chain.next() {
            heads.insert(
                (header.opened_wall_ns, header.path.clone(), chain_index),
                header,
            );
        }
    }
    let mut ordered = Vec::with_capacity(file_count);
    while let Some(((_, _, chain_index), header)) = heads.pop_first() {
        ordered.push(header);
        if let Some(header) = chains[chain_index].next() {
            heads.insert(
                (header.opened_wall_ns, header.path.clone(), chain_index),
                header,
            );
        }
    }
    ordered
}

fn validate_headers(headers: &[Header], network: Network) -> Result<(), RecordingError> {
    let mut source = None;
    let mut sequences: BTreeMap<i64, u64> = BTreeMap::new();
    for header in headers {
        if header.network != network {
            return Err(RecordingError::new(
                &header.path,
                "recording network does not match --network",
            ));
        }
        if let Some(next_source) = &header.position.source {
            if source.is_some_and(|source| source != next_source) {
                return Err(RecordingError::new(
                    &header.path,
                    "recordings contain more than one source",
                ));
            }
            source = Some(next_source);
        }
        if let (Some(process), Some(sequence)) = (
            header.position.process_start_wall_ns,
            header.position.file_seq,
        ) && let Some(previous) = sequences.insert(process, sequence)
        {
            let expected = previous.checked_add(1).ok_or_else(|| {
                RecordingError::new(&header.path, "file_seq overflow in recording chain")
            })?;
            if sequence > expected {
                return Err(RecordingError::new(
                    &header.path,
                    format!("missing file_seq {expected} for process {process}"),
                ));
            }
            if sequence != expected {
                return Err(RecordingError::new(
                    &header.path,
                    format!("expected file_seq {expected}, found {sequence} for process {process}"),
                ));
            }
        }
    }
    Ok(())
}

fn read_file(
    header: &Header,
    process_changed: bool,
    converter: &mut RecordConverter<'_>,
    events: &mut Vec<MarketEvent>,
    mut paper: Option<&mut PaperIdentity<'_>>,
) -> Result<(), RecordingError> {
    let raw = VerifiedRawFile::open(&header.path, Some(&header.stamp))
        .map_err(|error| RecordingError::new(&header.path, error))?;
    let mut input = open_reader(&header.path, &raw.file)?;
    if read_header(
        &header.path,
        &mut input,
        raw.stamp,
        paper.as_ref().map(|_| header.network),
    )? != *header
    {
        return Err(RecordingError::new(
            &header.path,
            "file_open changed while reading recordings",
        ));
    }
    let first_event = events.len();
    converter
        .file_start(
            header.opened_wall_ns,
            header.opened_mono_ns,
            header.position.process_start_wall_ns,
            process_changed,
            events,
        )
        .map_err(|error| RecordingError::new(&header.path, error))?;
    if let Some(paper) = paper.as_deref_mut() {
        paper.record_position(converter, &events[first_event..]);
    }
    for record in input {
        let record = match record {
            Ok(record) => record,
            Err(ReadError::Truncated) => {
                converter
                    .truncated(events)
                    .map_err(|error| RecordingError::new(&header.path, error))?;
                break;
            }
            Err(ReadError::Io(error)) if error.kind() == io::ErrorKind::UnexpectedEof => {
                converter
                    .truncated(events)
                    .map_err(|error| RecordingError::new(&header.path, error))?;
                break;
            }
            Err(error) => return Err(RecordingError::new(&header.path, error)),
        };
        let first_event = events.len();
        converter
            .record(&record, events)
            .map_err(|error| RecordingError::new(&header.path, error))?;
        if let Some(paper) = paper.as_deref_mut() {
            paper.record_position(converter, &events[first_event..]);
        }
    }
    raw.verify()
        .map_err(|error| RecordingError::new(&header.path, error))?;
    Ok(())
}

/// Reads BBO and live-trade events for one coin from a set of recorder files.
///
/// Numbered files of one `(source, process_start_wall_ns)` form a chain ordered
/// by consecutive `file_seq` values. Each legacy file without a sequence number
/// forms its own chain. Repeatedly reading the chain whose next header has the
/// smallest `(receive wall time, path)` merges the chains as RFC 0008 D5
/// specifies. See RFC 0002 D2 for the file header contract. [`RecordConverter`]
/// applies the connection rules and the trade cutoff in HL-81
/// (`docs/venues/hyperliquid.md`).
///
/// A process-boundary gap uses the new header's wall and monotonic stamps. A
/// truncated-file gap uses the last complete record's stamps, or the header's
/// stamps if no later record was complete. RFC 0008 D4 derives the process clock
/// from the header's process anchor, falling back to header wall time minus
/// header monotonic time. Local time is the maximum of the raw wall time, the
/// process clock and the preceding event's local time. Market events also use
/// their exchange time as a floor. The inclusive stop applies the same rule to
/// the final complete record.
///
/// # Errors
///
/// Returns an error for unreadable files, invalid headers, mixed sources or
/// networks, nonconsecutive file sequences, process-clock overflow and corrupt
/// records. A file cut off after its complete header is counted and read through
/// its last complete record.
pub fn read_recordings(
    paths: &[PathBuf],
    network: Network,
    coin: &str,
) -> Result<Recording, RecordingError> {
    read_recordings_from(Path::new(""), paths, network, coin, None, None)
}

/// Required raw-chain identity and the midnights the checker will replay.
pub(crate) struct PaperIdentity<'a> {
    pub source: &'a str,
    pub process_start_wall_ns: i64,
    pub sealed: bool,
    pub extents: BTreeMap<Timestamp, Option<RawPosition>>,
}

impl PaperIdentity<'_> {
    fn record_position(&mut self, converter: &RecordConverter<'_>, events: &[MarketEvent]) {
        use std::ops::Bound::{Excluded, Unbounded};
        let time = converter
            .checkpoint_record_time(events)
            .expect("file header installed");
        if let Some((_, last)) = self.extents.range_mut((Excluded(time), Unbounded)).next() {
            // Update one interval per record. A forward wall stamp on a
            // control does not prevent a later raw position falling before it.
            *last = converter.last_position();
        }
    }
}

fn validate_paper_headers(
    headers: &[Header],
    identity: &PaperIdentity<'_>,
) -> Result<(), RecordingError> {
    for (index, header) in headers.iter().enumerate() {
        if header.position.source.as_deref() != Some(identity.source) {
            return Err(RecordingError::new(
                &header.path,
                "paper source differs from start.source",
            ));
        }
        if header.position.process_start_wall_ns != Some(identity.process_start_wall_ns) {
            return Err(RecordingError::new(
                &header.path,
                "paper process anchor differs from start.process_start_wall_ns",
            ));
        }
        if header.position.file_seq != Some(index as u64) {
            return Err(RecordingError::new(
                &header.path,
                format!(
                    "paper chain expected file_seq {index}, found {:?}",
                    header.position.file_seq
                ),
            ));
        }
    }
    Ok(())
}

/// Resolve file IO against `directory` while breaking header-time ties by the
/// supplied path spellings, just as the original recording command did.
pub(crate) fn read_recordings_from(
    directory: &Path,
    paths: &[PathBuf],
    network: Network,
    coin: &str,
    expected_stamps: Option<&[RawFileStamp]>,
    mut paper: Option<&mut PaperIdentity<'_>>,
) -> Result<Recording, RecordingError> {
    if expected_stamps.is_some_and(|stamps| stamps.len() != paths.len()) {
        return Err(RecordingError::new(
            directory,
            "raw file identity count differs from path count",
        ));
    }
    let headers = paths
        .iter()
        .enumerate()
        .map(|(index, path)| {
            let resolved = directory.join(path);
            let expected = expected_stamps.map(|stamps| &stamps[index]);
            let raw = VerifiedRawFile::open(&resolved, expected)
                .map_err(|error| RecordingError::new(&resolved, error))?;
            let mut header = read_header(
                &resolved,
                &mut open_reader(&resolved, &raw.file)?,
                raw.stamp,
                paper.as_ref().map(|_| network),
            )?;
            raw.verify()
                .map_err(|error| RecordingError::new(&resolved, error))?;
            header.path.clone_from(path);
            Ok(header)
        })
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(identity) = paper.as_deref() {
        validate_paper_headers(&headers, identity)?;
    }
    let mut headers = order_headers(headers);
    for header in &mut headers {
        header.path = directory.join(&header.path);
    }
    validate_headers(&headers, network)?;
    let mut converter = RecordConverter::new(network, coin);
    let mut recording = Recording::default();
    let mut previous: Option<&Header> = None;
    for header in &headers {
        let process_changed = previous.is_some_and(|previous| !header.same_process(previous));
        read_file(
            header,
            process_changed,
            &mut converter,
            &mut recording.events,
            paper.as_deref_mut(),
        )?;
        previous = Some(header);
    }
    if let Some(paper) = paper {
        if converter.stats().truncated_files != 0 {
            return Err(RecordingError::new(directory, "truncated paper raw file"));
        }
        // A final control frame or file header after shutdown has no event of
        // its own. Match its raw position as well as the last emitted gap.
        if paper.sealed
            && !converter.origins().last().is_some_and(|origin| {
                origin.gap_cause == Some(GapCause::Shutdown)
                    && Some(origin.position) == converter.last_position()
            })
        {
            return Err(RecordingError::new(
                directory,
                "paper final raw record must be shutdown",
            ));
        }
        // The requested cutoff includes every preceding interval, even a
        // quiet day. Use raw order, including control-only wall-clock steps.
        let mut last = None;
        for extent in paper.extents.values_mut() {
            last = last.max(*extent);
            *extent = last;
        }
    }
    recording.stats = converter.stats().clone();
    recording.stop_time = converter.stop_time();
    recording.last_position = converter.last_position();
    recording.origins = converter.take_origins();
    recording.file_stamps = headers.iter().map(|header| header.stamp).collect();
    recording.files = headers
        .into_iter()
        .map(|h| (h.path, h.position.file_seq))
        .collect();
    Ok(recording)
}

#[cfg(test)]
mod tests {
    use std::fs::{self, FileTimes};
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use hl_codec::rawlog::RecordKind;

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    struct SealedRaw {
        directory: PathBuf,
        path: PathBuf,
        bytes: Vec<u8>,
    }

    impl SealedRaw {
        fn new() -> Self {
            let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let directory = std::env::temp_dir().join(format!(
                "hl-backtest-file-identity-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&directory).expect("create synthetic raw directory");
            let mut raw = Vec::new();
            hl_codec::rawlog::write_record(
                &mut raw,
                RecordKind::Meta,
                0,
                0,
                br#"{"event":"file_open","format":1,"source":"hl","url":"wss://api.hyperliquid.xyz/ws"}"#,
            )
            .expect("write synthetic header");
            let bytes =
                zstd::stream::encode_all(raw.as_slice(), 1).expect("compress synthetic raw");
            let path = directory.join("input.hlrec.zst");
            fs::write(&path, &bytes).expect("write synthetic raw");
            Self {
                directory,
                path,
                bytes,
            }
        }

        fn replace_with_same_bytes(&self) {
            let replacement = self.directory.join("replacement.hlrec.zst");
            fs::write(&replacement, &self.bytes).expect("write replacement inode");
            fs::rename(replacement, &self.path).expect("replace synthetic raw path");
        }
    }

    impl Drop for SealedRaw {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.directory).expect("remove synthetic raw directory");
        }
    }

    #[test]
    fn checksum_identity_rejects_a_replacement_before_decoding() {
        let raw = SealedRaw::new();
        let (_, stamp) = sha256_file_stamped(&raw.path, None).expect("hash sealed input");
        raw.replace_with_same_bytes();
        let error = read_recordings_from(
            Path::new(""),
            std::slice::from_ref(&raw.path),
            Network::Mainnet,
            "BTC",
            Some(&[stamp]),
            None,
        )
        .expect_err("decoding must use the same inode that was hashed");
        assert!(error.to_string().contains("raw file changed"), "{error}");
    }

    #[test]
    fn decoded_identity_rejects_a_replacement_before_hashing() {
        let raw = SealedRaw::new();
        let recording = read_recordings(std::slice::from_ref(&raw.path), Network::Mainnet, "BTC")
            .expect("decode sealed input");
        raw.replace_with_same_bytes();
        let error = sha256_file_checked(&raw.path, &recording.file_stamps[0])
            .expect_err("manifest hash must describe the decoded inode");
        assert!(error.to_string().contains("raw file changed"), "{error}");
    }

    #[test]
    fn descriptor_identity_detects_same_length_writes_with_restored_mtime() {
        let raw = SealedRaw::new();
        let opened = VerifiedRawFile::open(&raw.path, None).expect("open sealed input");
        let modified = opened
            .file
            .metadata()
            .expect("metadata")
            .modified()
            .expect("mtime");
        let mut changed = raw.bytes.clone();
        *changed.last_mut().expect("compressed bytes") ^= 1;
        fs::write(&raw.path, &changed).expect("rewrite the same inode with the same length");
        File::options()
            .write(true)
            .open(&raw.path)
            .expect("open rewritten input")
            .set_times(FileTimes::new().set_modified(modified))
            .expect("restore modification time");
        let error = opened
            .verify()
            .expect_err("ctime detects the intervening write");
        assert!(error.to_string().contains("raw file changed"), "{error}");
    }

    fn header(process: i64, sequence: u64) -> Header {
        Header {
            path: PathBuf::from(format!("{process}-{sequence}.hlrec.zst")),
            stamp: RawFileStamp {
                device: 0,
                inode: 0,
                length: 0,
                changed_seconds: 0,
                changed_nanoseconds: 0,
            },
            position: ChainPosition {
                source: Some("hl".to_owned()),
                process_start_wall_ns: Some(process),
                file_seq: Some(sequence),
            },
            opened_wall_ns: 0,
            opened_mono_ns: 0,
            network: Network::Mainnet,
        }
    }

    #[test]
    fn sequence_validation_tracks_each_process_across_intervening_files() {
        let headers = [header(100, 3), header(200, 0), header(100, 5)];
        let error = validate_headers(&headers, Network::Mainnet)
            .expect_err("the sequence hole survives an intervening process");
        assert!(error.to_string().contains("missing file_seq 4"));

        let consecutive = [header(100, 3), header(200, 0), header(100, 4)];
        validate_headers(&consecutive, Network::Mainnet)
            .expect("a run may start at a nonzero file sequence");
    }

    #[test]
    fn duplicate_and_reversed_sequences_are_errors() {
        for second in [2, 3] {
            let error = validate_headers(&[header(100, 3), header(100, second)], Network::Mainnet)
                .expect_err("a process sequence must advance");
            assert!(error.to_string().contains("expected file_seq 4"));
        }
    }

    #[test]
    fn mixed_sources_and_networks_are_errors() {
        let first = header(100, 0);
        let mut second = header(200, 0);
        second.position.source = Some("another-source".to_owned());
        let error = validate_headers(&[first, second], Network::Mainnet)
            .expect_err("only one source is accepted");
        assert!(error.to_string().contains("more than one source"));

        let error = validate_headers(&[header(100, 0)], Network::Testnet)
            .expect_err("the requested network must match");
        assert!(error.to_string().contains("network"));
    }
}
