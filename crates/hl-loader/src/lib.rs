//! Loads raw files into ClickHouse.
//!
//! Raw files stay the source of truth; every table this crate writes can be
//! rebuilt from them. Three inputs are understood: the official L2 archive,
//! the official `node_fills_by_block` archive, and recordings made by
//! `hl-recorder` of a Hyperliquid feed. Table layout and loading rules are in
//! `docs/rfc/0004-clickhouse-schema-and-loading.md`.
//!
//! A load is idempotent in three layers: a file whose SHA-256 is already in
//! `hl.ingest_log` with status `done` is skipped (numbered recordings also
//! need their matching end state); each insert carries a
//! deduplication token derived from that hash; and the tables are
//! `ReplacingMergeTree`s keyed so that the same row loaded twice collapses.

pub mod db;
pub mod input;
mod load;
pub mod quality;
pub mod rows;
pub mod sources;

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

pub use db::{ConflictTable, Db, DbError};
pub use load::{LoadOptions, load_file, load_files};
pub use sources::{Batch, InputKind, SourceError};

/// What happened to one file.
#[derive(Debug)]
pub enum Outcome {
    /// The file was loaded.
    Loaded {
        /// Id of this load.
        ingest_id: u64,
        /// Rows written per table.
        rows: Vec<(&'static str, usize)>,
        /// Counters from reading the file.
        counters: std::collections::BTreeMap<String, u64>,
    },
    /// A file with the same content was loaded before.
    AlreadyLoaded,
    /// This recording needs an earlier file before it can be loaded.
    WaitingForPredecessor {
        /// Source and process from the file header.
        chain: sources::RawlogChain,
        /// The immediately preceding file number in this chain.
        missing_file_seq: u64,
    },
    /// The file is not something this kind of load handles.
    Unsupported(String),
}

/// Why a file could not be loaded.
#[derive(Debug)]
pub enum LoadError {
    /// The file could not be read or turned into rows.
    Source(SourceError),
    /// The database refused something.
    Db(DbError),
    /// A successful ingest has no matching decoder state.
    IncompleteState(String),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source(error) => error.fmt(f),
            Self::Db(error) => error.fmt(f),
            Self::IncompleteState(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for LoadError {}

impl From<SourceError> for LoadError {
    fn from(error: SourceError) -> Self {
        Self::Source(error)
    }
}

impl From<DbError> for LoadError {
    fn from(error: DbError) -> Self {
        Self::Db(error)
    }
}

impl From<std::io::Error> for LoadError {
    fn from(error: std::io::Error) -> Self {
        Self::Source(SourceError::Io(error))
    }
}

fn now_ns() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_nanos()).unwrap_or(i64::MAX)
        })
}

/// Milliseconds since the epoch, now.
pub fn now_ms() -> i64 {
    now_ns() / 1_000_000
}

/// The instruments and the `ts_exch` range (milliseconds) of some rows.
fn extent<T>(rows: &[T], key: impl Fn(&T) -> (u32, i64)) -> (Vec<u32>, (i64, i64)) {
    let mut instruments = std::collections::BTreeSet::new();
    let (mut low, mut high) = (i64::MAX, i64::MIN);
    for row in rows {
        let (instrument, ts_exch) = key(row);
        instruments.insert(instrument);
        low = low.min(ts_exch);
        high = high.max(ts_exch);
    }
    (instruments.into_iter().collect(), (low, high))
}

/// Writes a batch and returns how many keys now hold conflicting rows.
async fn store(db: &Db, batch: &Batch, token: &str, ingest_id: u64) -> Result<u64, DbError> {
    db.add_instruments(&batch.instruments, ingest_id).await?;
    // A retry after an interrupted load may already have L2 chunks written by
    // the older decoder, which applied stale diffs and produced different rows.
    let l2_token = batch
        .counters
        .contains_key(sources::STALE_DIFFS_SKIPPED)
        .then(|| format!("{token}:skip-stale"));
    db.insert(
        "hl.l2_snapshot",
        &batch.l2,
        l2_token.as_deref().unwrap_or(token),
    )
    .await?;
    db.insert("hl.fills", &batch.fills, token).await?;
    db.insert("hl.bbo", &batch.bbo, token).await?;
    // An interrupted load may have written unknown-start trades with live = 0.
    let trade_token = batch
        .has_unknown_start_trades
        .then(|| format!("{token}:unknown-start-live"));
    db.insert(
        "hl.ws_trades",
        &batch.ws_trades,
        trade_token.as_deref().unwrap_or(token),
    )
    .await?;

    let now = now_ms();
    let checks = [
        (
            ConflictTable::L2Snapshot,
            extent(&batch.l2, |row| (row.instrument_id, row.ts_exch)),
        ),
        (
            ConflictTable::Fills,
            extent(&batch.fills, |row| (row.instrument_id, row.ts_exch)),
        ),
        (
            ConflictTable::Bbo,
            extent(&batch.bbo, |row| (row.instrument_id, row.ts_exch)),
        ),
        (
            ConflictTable::WsTrades,
            extent(&batch.ws_trades, |row| (row.instrument_id, row.ts_exch)),
        ),
    ];
    let mut conflicts = 0;
    for (table, (instruments, range)) in checks {
        conflicts += db
            .record_conflicts(table, &instruments, range, ingest_id, now)
            .await?;
    }
    Ok(conflicts)
}
