//! Turning input files into table rows, without touching the database.

use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, BufRead, Read};

use hl_book::{BookError, DiffOutcome, L2Book};
use hl_codec::hyperliquid::{
    DecodeError, FillBlock, Network, NodeFill, TradeCutoff, WsFrame, decode_archive_l2_line,
    decode_fill_block, decode_ws_frame, instrument_id,
};
use hl_codec::rawlog::{FileOpen, FileOpenError, ReadError, Record, RecordKind, RecordReader};
use hl_types::{InstrumentId, L2Snapshot, Timestamp};
use serde_json::Value;

use crate::rows::{
    BboRow, FillRow, L2Row, SOURCE_ARCHIVE, SOURCE_NODE_FILLS, SOURCE_WS, WsTradeRow, side_code,
};

mod rawlog_state;

pub use hl_codec::rawlog::ChainKey as RawlogChain;
pub use rawlog_state::RawlogState;

/// The kinds of input the loader understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    /// One coin-hour of the official L2 archive (HL-97).
    ArchiveL2,
    /// One hour of the official `node_fills_by_block` archive (HL-105).
    NodeFills,
    /// One file written by `hl-recorder` for a Hyperliquid source.
    Rawlog,
}

impl InputKind {
    /// The `source` recorded in the ingest log for this kind.
    pub fn source(self) -> &'static str {
        match self {
            Self::ArchiveL2 => SOURCE_ARCHIVE,
            Self::NodeFills => SOURCE_NODE_FILLS,
            Self::Rawlog => SOURCE_WS,
        }
    }

    /// Parses the command-line name of a kind.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "archive-l2" => Some(Self::ArchiveL2),
            "node-fills" => Some(Self::NodeFills),
            "rawlog" => Some(Self::Rawlog),
            _ => None,
        }
    }
}

/// Why a file could not be turned into rows.
#[derive(Debug)]
pub enum SourceError {
    /// Reading or decompressing the file failed.
    Io(io::Error),
    /// A line of an official archive did not decode. Archives are expected
    /// to be well-formed, so one bad line rejects the file.
    Line {
        /// 1-based line number.
        line: usize,
        /// What was wrong.
        error: DecodeError,
    },
    /// The file is not something this kind of load handles, for example a
    /// recording of a venue other than Hyperliquid.
    Unsupported(String),
    /// The file's structure is wrong in a way that is not a decoding error
    /// of one line.
    Invalid(String),
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "read failed: {error}"),
            Self::Line { line, error } => write!(f, "line {line}: {error}"),
            Self::Unsupported(reason) => write!(f, "not supported: {reason}"),
            Self::Invalid(reason) => write!(f, "invalid file: {reason}"),
        }
    }
}

impl std::error::Error for SourceError {}

impl From<io::Error> for SourceError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<FileOpenError> for SourceError {
    fn from(error: FileOpenError) -> Self {
        match error {
            FileOpenError::Json(error) => {
                Self::Invalid(format!("meta record is not JSON: {error}"))
            }
            error => Self::Invalid(error.to_string()),
        }
    }
}

/// Everything one file contributes to the database.
#[derive(Debug, Default)]
pub struct Batch {
    /// Rows for `hl.l2_snapshot`.
    pub l2: Vec<L2Row>,
    /// Rows for `hl.fills`.
    pub fills: Vec<FillRow>,
    /// Rows for `hl.bbo`.
    pub bbo: Vec<BboRow>,
    /// Rows for `hl.ws_trades`.
    pub ws_trades: Vec<WsTradeRow>,
    /// Instruments seen: `(venue, coin)` to id.
    pub instruments: BTreeMap<(String, String), InstrumentId>,
    /// Earliest and latest `ts_exch` of any row, in milliseconds.
    pub ts_range: Option<(i64, i64)>,
    /// Counters worth keeping in the ingest log: skipped rows, gaps,
    /// decoding problems.
    pub counters: BTreeMap<String, u64>,
    /// The first few problem messages, for the ingest log.
    pub problems: Vec<String>,
    /// Whether any trade arrived before the first known connection boundary.
    pub has_unknown_start_trades: bool,
}

/// How many problem messages a batch keeps.
const MAX_PROBLEMS: usize = 5;
pub(crate) const STALE_DIFFS_SKIPPED: &str = "stale_diffs_skipped";

impl Batch {
    /// Total rows across the data tables.
    pub fn rows(&self) -> u64 {
        (self.l2.len() + self.fills.len() + self.bbo.len() + self.ws_trades.len()) as u64
    }

    fn instrument(&mut self, network: Network, coin: &str) -> u32 {
        let id = instrument_id(network, coin);
        self.instruments
            .entry((network.venue().to_owned(), coin.to_owned()))
            .or_insert(id);
        id.get()
    }

    fn saw_time(&mut self, ts_exch: Timestamp) {
        let millis = ts_exch.as_millis();
        self.ts_range = Some(match self.ts_range {
            Some((low, high)) => (low.min(millis), high.max(millis)),
            None => (millis, millis),
        });
    }

    fn count(&mut self, counter: &str, by: u64) {
        *self.counters.entry(counter.to_owned()).or_default() += by;
    }

    fn problem(&mut self, counter: &str, message: String) {
        self.count(counter, 1);
        if self.problems.len() < MAX_PROBLEMS {
            self.problems.push(message);
        }
    }
}

fn lines(input: impl BufRead) -> impl Iterator<Item = (usize, io::Result<Vec<u8>>)> {
    input
        .split(b'\n')
        .enumerate()
        .map(|(index, line)| (index + 1, line))
}

/// Reads one file of the official L2 archive.
pub fn read_archive_l2(input: impl BufRead, ingest_id: u64) -> Result<Batch, SourceError> {
    let mut batch = Batch::default();
    for (line_number, line) in lines(input) {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        let archived = decode_archive_l2_line(&line).map_err(|error| SourceError::Line {
            line: line_number,
            error,
        })?;
        let book = &archived.book;
        let instrument_id = batch.instrument(Network::Mainnet, &book.coin);
        batch.saw_time(book.time);
        batch.l2.push(L2Row::new(
            instrument_id,
            SOURCE_ARCHIVE,
            book.time,
            archived.node_time,
            Timestamp::EPOCH,
            &book.bids,
            &book.asks,
            ingest_id,
        ));
    }
    Ok(batch)
}

fn fill_row(block: &FillBlock, fill: &NodeFill, instrument_id: u32, ingest_id: u64) -> FillRow {
    let (liquidated_user, liquidation_mark_px, liquidation_method) = match &fill.liquidation {
        Some(liquidation) => (
            liquidation.liquidated_user.0,
            liquidation.mark_px.as_e8(),
            liquidation.method.clone(),
        ),
        None => ([0; 20], 0, String::new()),
    };
    FillRow {
        instrument_id,
        ts_exch: fill.time.as_millis(),
        block_number: block.block_number,
        ts_node: block.local_time.as_nanos(),
        tid: fill.tid.0,
        side: side_code(fill.side),
        user: fill.user.0,
        crossed: fill.crossed,
        px: fill.px.as_e8(),
        sz: fill.qty.as_e8(),
        start_pos: fill.start_position.as_e8(),
        dir: fill.dir.clone(),
        closed_pnl: fill.closed_pnl.as_e8(),
        fee: fill.fee.as_e8(),
        fee_token: fill.fee_token.clone(),
        builder_fee: fill.builder_fee.map_or(0, |fee| fee.as_e8()),
        builder: fill.builder.map_or([0; 20], |address| address.0),
        deployer_fee: fill.deployer_fee.map_or(0, |fee| fee.as_e8()),
        priority_gas: fill.priority_gas.map_or(0, |gas| gas.as_e8()),
        oid: fill.oid.0,
        cloid: fill.cloid.map_or([0; 16], |cloid| cloid.to_bytes()),
        twap_id: fill.twap_id.unwrap_or(0),
        hash: fill.hash,
        liquidation: fill.liquidation.is_some(),
        liquidated_user,
        liquidation_mark_px,
        liquidation_method,
        source: SOURCE_NODE_FILLS.to_owned(),
        ingest_id,
    }
}

/// Reads one hour of `node_fills_by_block`. Only fills on perpetuals become
/// rows; spot and outcome rows are counted.
pub fn read_node_fills(input: impl BufRead, ingest_id: u64) -> Result<Batch, SourceError> {
    let mut batch = Batch::default();
    let mut previous_block = None;
    for (line_number, line) in lines(input) {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        let block = decode_fill_block(&line).map_err(|error| SourceError::Line {
            line: line_number,
            error,
        })?;
        if let Some(previous) = previous_block
            && block.block_number != previous + 1
        {
            batch.problem(
                "block_gaps",
                format!("block {} follows block {previous}", block.block_number),
            );
        }
        previous_block = Some(block.block_number);
        batch.count("blocks", 1);
        batch.count("spot_rows_skipped", u64::from(block.spot_rows));
        batch.count("outcome_rows_skipped", u64::from(block.outcome_rows));
        for fill in &block.fills {
            let instrument_id = batch.instrument(Network::Mainnet, &fill.coin);
            batch.saw_time(fill.time);
            batch
                .fills
                .push(fill_row(&block, fill, instrument_id, ingest_id));
        }
    }
    Ok(batch)
}

/// Header fields needed to order recordings and choose their starting state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawlogMetadata {
    /// Chain identity, absent for old headers without its identifying fields.
    pub chain: Option<RawlogChain>,
    /// File number in the chain, absent for legacy recordings.
    pub file_seq: Option<u64>,
    /// Wall-clock nanoseconds of the `file_open` record.
    pub opened_wall_ns: i64,
    /// Process-relative time of the header, ordered even across wall-clock steps.
    pub opened_mono_ns: u64,
    /// Hyperliquid network identified by the recorded URL.
    pub network: Network,
}

fn rawlog_metadata(record: &Record) -> Result<RawlogMetadata, SourceError> {
    if record.kind != RecordKind::Meta {
        return Err(SourceError::Invalid(
            "first record is not file_open".to_owned(),
        ));
    }
    let open = FileOpen::parse(record)?;
    let url = open.url().unwrap_or_default();
    let network = Network::from_url(url).ok_or_else(|| {
        SourceError::Unsupported(format!("recording of {url:?} is not a Hyperliquid feed"))
    })?;
    let position = open.chain_position()?;
    Ok(RawlogMetadata {
        chain: position.key(),
        file_seq: position.file_seq,
        opened_wall_ns: open.opened_wall_ns,
        opened_mono_ns: open.opened_mono_ns,
        network,
    })
}

fn rawlog_header(input: &mut RecordReader<impl Read>) -> Result<RawlogMetadata, SourceError> {
    let record = input
        .read_record()
        .map_err(|error| SourceError::Invalid(error.to_string()))?
        .ok_or_else(|| SourceError::Invalid("no file_open record".to_owned()))?;
    rawlog_metadata(&record)
}

/// Reads the first record of a decompressed recording to identify its chain.
pub fn read_rawlog_metadata(input: impl Read) -> Result<RawlogMetadata, SourceError> {
    rawlog_header(&mut RecordReader::new(input))
}

fn book_row(
    instrument_id: u32,
    time: Timestamp,
    ts_recv: Timestamp,
    book: &L2Book,
    ingest_id: u64,
) -> L2Row {
    L2Row::new(
        instrument_id,
        SOURCE_WS,
        time,
        Timestamp::EPOCH,
        ts_recv,
        book.bids(),
        book.asks(),
        ingest_id,
    )
}

fn handle_frame(
    batch: &mut Batch,
    state: &mut RawlogState,
    frame: WsFrame,
    ts_recv: Timestamp,
    ingest_id: u64,
) {
    let network = state.network;
    match frame {
        WsFrame::Book(book) => {
            let id = batch.instrument(network, &book.coin);
            let snapshot = L2Snapshot {
                bids: book.bids,
                asks: book.asks,
            };
            let state_book = state.books.entry(id).or_default();
            match state_book.apply_snapshot(&snapshot, book.time) {
                Ok(()) => {
                    batch.saw_time(book.time);
                    batch
                        .l2
                        .push(book_row(id, book.time, ts_recv, state_book, ingest_id));
                }
                Err(error) => {
                    batch.problem("book_errors", format!("{} snapshot: {error}", book.coin))
                }
            }
        }
        WsFrame::BookDiff(change) => {
            let id = batch.instrument(network, &change.coin);
            let state_book = state.books.entry(id).or_default();
            match state_book.apply_diff(&change.diff, change.time) {
                Ok(DiffOutcome::Applied) => {
                    batch.saw_time(change.time);
                    batch
                        .l2
                        .push(book_row(id, change.time, ts_recv, state_book, ingest_id));
                }
                Ok(DiffOutcome::SkippedStale) => batch.count(STALE_DIFFS_SKIPPED, 1),
                Err(BookError::NotSynced) => batch.count("diffs_without_snapshot", 1),
                Err(error) => {
                    batch.problem("book_errors", format!("{} diff: {error}", change.coin))
                }
            }
        }
        WsFrame::Bbo(bbo) => {
            let id = batch.instrument(network, &bbo.coin);
            batch.saw_time(bbo.time);
            let side = |level: Option<hl_types::Level>| {
                level.map_or((0, 0, 0), |level| {
                    (level.px.as_e8(), level.qty.as_e8(), level.orders)
                })
            };
            let ((bid_px, bid_sz, bid_n), (ask_px, ask_sz, ask_n)) = (side(bbo.bid), side(bbo.ask));
            batch.bbo.push(BboRow {
                instrument_id: id,
                source: SOURCE_WS.to_owned(),
                ts_exch: bbo.time.as_millis(),
                ts_recv: ts_recv.as_nanos(),
                bid_px,
                bid_sz,
                bid_n,
                ask_px,
                ask_sz,
                ask_n,
                ingest_id,
            });
        }
        WsFrame::Trades(trades) => {
            batch.has_unknown_start_trades |=
                !trades.is_empty() && state.cutoff == TradeCutoff::UnknownStart;
            for trade in trades {
                let id = batch.instrument(network, &trade.coin);
                batch.saw_time(trade.time);
                let live = state.cutoff.is_live(trade.time);
                batch.count(
                    if live {
                        "trades_live"
                    } else {
                        "trades_replayed"
                    },
                    1,
                );
                batch.ws_trades.push(WsTradeRow {
                    instrument_id: id,
                    source: SOURCE_WS.to_owned(),
                    ts_exch: trade.time.as_millis(),
                    ts_recv: ts_recv.as_nanos(),
                    tid: trade.tid.0,
                    aggressor: side_code(trade.aggressor),
                    px: trade.px.as_e8(),
                    sz: trade.qty.as_e8(),
                    buyer: trade.buyer.0,
                    seller: trade.seller.0,
                    hash: trade.hash,
                    live: u8::from(live),
                    ingest_id,
                });
            }
        }
        WsFrame::Other { channel } => batch.count(&format!("frames_not_loaded:{channel}"), 1),
        WsFrame::Error(message) => batch.problem("venue_errors", format!("venue error: {message}")),
        WsFrame::SubscriptionResponse | WsFrame::Pong => {}
    }
}

fn meta_event(payload: &[u8]) -> Result<Value, SourceError> {
    serde_json::from_slice(payload)
        .map_err(|error| SourceError::Invalid(format!("meta record is not JSON: {error}")))
}

/// Reads one recording made by `hl-recorder` of a Hyperliquid source.
///
/// Each valid snapshot or applied diff yields a book row. Diffs already
/// included in the subscription snapshot only increment `stale_diffs_skipped` (HL-75).
/// A disconnect drops every book until the next snapshot. A recording cut
/// off by a crash is read up to the cut, which is counted as `truncated`.
/// Frames that fail to decode are counted and reported, not fatal: one odd
/// frame must not cost an hour of data.
pub fn read_rawlog(input: impl Read, ingest_id: u64) -> Result<Batch, SourceError> {
    read_rawlog_with_state(input, ingest_id, None).map(|(batch, _)| batch)
}

/// Reads a recording from the preceding file's state in the same source
/// and process. A rotation carries the state forward. At a truncated tail,
/// the returned state reflects the last complete record.
pub fn read_rawlog_with_state(
    input: impl Read,
    ingest_id: u64,
    start: Option<RawlogState>,
) -> Result<(Batch, RawlogState), SourceError> {
    let mut input = RecordReader::new(input);
    let metadata = rawlog_header(&mut input)?;
    let mut state = start.unwrap_or_else(|| RawlogState::new(metadata.network));
    if state.network != metadata.network {
        return Err(SourceError::Invalid(
            "carried state belongs to a different network".to_owned(),
        ));
    }
    let mut batch = Batch::default();
    for record in input {
        let record = match record {
            Ok(record) => record,
            // A recording cut off by a crash ends with a decompression error.
            Err(ReadError::Io(error)) => {
                batch.problem("truncated", format!("recording ends early: {error}"));
                break;
            }
            Err(ReadError::Truncated) => {
                batch.problem("truncated", "recording ends inside a record".to_owned());
                break;
            }
            Err(error) => return Err(SourceError::Invalid(error.to_string())),
        };
        let ts_recv = Timestamp::from_nanos(record.recv_wall_ns);
        match record.kind {
            RecordKind::Meta => {
                let meta = meta_event(&record.payload)?;
                let event = meta
                    .get("event")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                state.cutoff.observe_meta(event, ts_recv);
                match event {
                    "file_open" => {
                        let next = rawlog_metadata(&record)?;
                        if next.network != metadata.network || next.chain != metadata.chain {
                            return Err(SourceError::Invalid(
                                "file_open changes the recording chain".to_owned(),
                            ));
                        }
                    }
                    "connected" | "disconnected" | "shutdown" => {
                        if event == "disconnected" {
                            batch.count("disconnects", 1);
                        }
                        state.books.clear();
                    }
                    "dropped" => {
                        let count = meta.get("count").and_then(Value::as_u64).unwrap_or(0);
                        batch.count("frames_dropped_by_recorder", count);
                    }
                    _ => {}
                }
            }
            RecordKind::RecvText => match decode_ws_frame(&record.payload) {
                Ok(frame) => handle_frame(&mut batch, &mut state, frame, ts_recv, ingest_id),
                Err(error) => batch.problem("decode_errors", error.to_string()),
            },
            RecordKind::RecvBinary => batch.count("binary_frames", 1),
            RecordKind::SentText => {}
        }
    }
    Ok((batch, state))
}

#[cfg(test)]
mod tests;
