//! Rust mirrors of the table rows in `schema/`.
//!
//! Field names and types must match the columns; the ClickHouse client checks
//! them against the server's schema before the first insert into each table.
//! Fixed-point values are written as their raw `i64` (`Decimal64(8)`),
//! `DateTime64(3)` as milliseconds and `DateTime64(9)` as nanoseconds.

#![allow(
    missing_docs,
    reason = "each field is the column of the same name in schema/"
)]

use clickhouse::Row;
use hl_types::{Level, Side, Timestamp};
use serde::{Deserialize, Serialize};

/// Source name of the official L2 archive.
pub const SOURCE_ARCHIVE: &str = "s3_archive";
/// Source name of the official `node_fills_by_block` archive.
pub const SOURCE_NODE_FILLS: &str = "node_fills";
/// Source name of data recorded from the public WebSocket feed.
pub const SOURCE_WS: &str = "ws_public";

/// `Enum8('B' = 1, 'A' = 2)`.
pub(crate) fn side_code(side: Side) -> i8 {
    match side {
        Side::Buy => 1,
        Side::Sell => 2,
    }
}

/// `hl.l2_snapshot`.
#[derive(Debug, Clone, PartialEq, Eq, Row, Serialize)]
pub struct L2Row {
    pub instrument_id: u32,
    pub source: String,
    pub ts_exch: i64,
    pub ts_node: i64,
    pub ts_recv: i64,
    pub bid_px: Vec<i64>,
    pub bid_sz: Vec<i64>,
    pub bid_n: Vec<u32>,
    pub ask_px: Vec<i64>,
    pub ask_sz: Vec<i64>,
    pub ask_n: Vec<u32>,
    pub ingest_id: u64,
}

impl L2Row {
    /// A book row. `ts_node` and `ts_recv` are `Timestamp::EPOCH` when not known.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        instrument_id: u32,
        source: &str,
        ts_exch: Timestamp,
        ts_node: Timestamp,
        ts_recv: Timestamp,
        bids: &[Level],
        asks: &[Level],
        ingest_id: u64,
    ) -> Self {
        Self {
            instrument_id,
            source: source.to_owned(),
            ts_exch: ts_exch.as_millis(),
            ts_node: ts_node.as_nanos(),
            ts_recv: ts_recv.as_nanos(),
            bid_px: bids.iter().map(|level| level.px.as_e8()).collect(),
            bid_sz: bids.iter().map(|level| level.qty.as_e8()).collect(),
            bid_n: bids.iter().map(|level| level.orders).collect(),
            ask_px: asks.iter().map(|level| level.px.as_e8()).collect(),
            ask_sz: asks.iter().map(|level| level.qty.as_e8()).collect(),
            ask_n: asks.iter().map(|level| level.orders).collect(),
            ingest_id,
        }
    }
}

/// `hl.fills`.
#[derive(Debug, Clone, PartialEq, Eq, Row, Serialize)]
pub struct FillRow {
    pub instrument_id: u32,
    pub ts_exch: i64,
    pub block_number: u64,
    pub ts_node: i64,
    pub tid: u64,
    pub side: i8,
    pub user: [u8; 20],
    pub crossed: bool,
    pub px: i64,
    pub sz: i64,
    pub start_pos: i64,
    pub dir: String,
    pub closed_pnl: i64,
    pub fee: i64,
    pub fee_token: String,
    pub builder_fee: i64,
    pub builder: [u8; 20],
    pub deployer_fee: i64,
    pub priority_gas: i64,
    pub oid: u64,
    pub cloid: [u8; 16],
    pub twap_id: u64,
    pub hash: [u8; 32],
    pub liquidation: bool,
    pub liquidated_user: [u8; 20],
    pub liquidation_mark_px: i64,
    pub liquidation_method: String,
    pub source: String,
    pub ingest_id: u64,
}

/// `hl.bbo`.
#[derive(Debug, Clone, PartialEq, Eq, Row, Serialize)]
pub struct BboRow {
    pub instrument_id: u32,
    pub source: String,
    pub ts_exch: i64,
    pub ts_recv: i64,
    pub bid_px: i64,
    pub bid_sz: i64,
    pub bid_n: u32,
    pub ask_px: i64,
    pub ask_sz: i64,
    pub ask_n: u32,
    pub ingest_id: u64,
}

/// `hl.ws_trades`.
#[derive(Debug, Clone, PartialEq, Eq, Row, Serialize)]
pub struct WsTradeRow {
    pub instrument_id: u32,
    pub source: String,
    pub ts_exch: i64,
    pub ts_recv: i64,
    pub tid: u64,
    pub aggressor: i8,
    pub px: i64,
    pub sz: i64,
    pub buyer: [u8; 20],
    pub seller: [u8; 20],
    pub hash: [u8; 32],
    pub live: u8,
    pub ingest_id: u64,
}

/// `hl.instruments`.
#[derive(Debug, Clone, PartialEq, Eq, Row, Serialize, Deserialize)]
pub struct InstrumentRow {
    pub venue: String,
    pub coin: String,
    pub instrument_id: u32,
    pub dex: String,
    pub ingest_id: u64,
}

/// `hl.rawlog_state`, the decoder state after a sequenced recording file.
#[derive(Debug, Clone, PartialEq, Eq, Row, Serialize, Deserialize)]
pub struct RawlogStateRow {
    pub source: String,
    pub process_start_wall_ns: i64,
    pub file_seq: u64,
    pub sha256: String,
    pub state_json: String,
    pub ingest_id: u64,
}

/// `hl.ingest_log`.
#[derive(Debug, Clone, PartialEq, Eq, Row, Serialize)]
pub struct IngestLogRow {
    pub ingest_id: u64,
    pub source: String,
    pub file_name: String,
    pub sha256: String,
    pub bytes: u64,
    pub rows: u64,
    pub ts_min: i64,
    pub ts_max: i64,
    pub loader_version: String,
    pub started_at: i64,
    pub finished_at: i64,
    /// `Enum8('done' = 1, 'failed' = 2)`.
    pub status: i8,
    pub details: String,
    pub error: String,
}

/// `ingest_log.status` for a completed load.
pub const STATUS_DONE: i8 = 1;
/// `ingest_log.status` for a failed load.
pub const STATUS_FAILED: i8 = 2;
