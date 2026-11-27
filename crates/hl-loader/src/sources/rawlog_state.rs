//! Lossless storage of the state carried between recording files.

use std::collections::BTreeMap;

use hl_book::L2Book;
use hl_codec::hyperliquid::{Network, TradeCutoff};
use hl_types::{L2Snapshot, Level, Px, Qty, Timestamp};
use serde::{Deserialize, Serialize};

use super::SourceError;

/// Decoder state at a boundary between records in one recording chain.
#[derive(Debug, Clone)]
pub struct RawlogState {
    pub(super) network: Network,
    pub(super) cutoff: TradeCutoff,
    pub(super) books: BTreeMap<u32, L2Book>,
}

/// Integer fixed-point price and quantity, followed by the order count.
type StoredLevel = (i64, i64, u32);

#[derive(Serialize, Deserialize)]
struct StoredBook {
    synced: bool,
    /// Absent in version 1. A book carried from that format keeps an unknown
    /// snapshot time until the next subscription snapshot.
    snapshot_time_ns: Option<i64>,
    bids: Vec<StoredLevel>,
    asks: Vec<StoredLevel>,
}

#[derive(Serialize, Deserialize)]
struct StoredState {
    version: u32,
    network: String,
    connected_at: Option<i64>,
    books: BTreeMap<u32, StoredBook>,
}

fn store_levels(levels: &[Level]) -> Vec<StoredLevel> {
    levels
        .iter()
        .map(|level| (level.px.as_e8(), level.qty.as_e8(), level.orders))
        .collect()
}

fn restore_levels(levels: Vec<StoredLevel>) -> Vec<Level> {
    levels
        .into_iter()
        .map(|(px, qty, orders)| Level {
            px: Px::from_e8(px),
            qty: Qty::from_e8(qty),
            orders,
        })
        .collect()
}

impl RawlogState {
    pub(super) fn new(network: Network) -> Self {
        Self {
            network,
            cutoff: TradeCutoff::UnknownStart,
            books: BTreeMap::new(),
        }
    }

    /// Encodes state deterministically with integer prices, sizes and times.
    pub fn to_json(&self) -> Result<String, SourceError> {
        let stored = StoredState {
            version: 2,
            network: self.network.venue().to_owned(),
            // Version 1 and 2 readers admit times at or above this bound.
            // The minimum admits every timestamp across an unknown-start rotation.
            connected_at: match self.cutoff {
                TradeCutoff::UnknownStart => Some(i64::MIN),
                TradeCutoff::ConnectedSince(time) => Some(time.as_nanos()),
                TradeCutoff::Disconnected => None,
            },
            books: self
                .books
                .iter()
                .map(|(&instrument_id, book)| {
                    (
                        instrument_id,
                        StoredBook {
                            synced: book.is_synced(),
                            snapshot_time_ns: book.snapshot_time().map(Timestamp::as_nanos),
                            bids: store_levels(book.bids()),
                            asks: store_levels(book.asks()),
                        },
                    )
                })
                .collect(),
        };
        serde_json::to_string(&stored)
            .map_err(|error| SourceError::Invalid(format!("cannot encode rawlog state: {error}")))
    }

    /// Restores a stored state, validating the format and every synced book.
    pub fn from_json(input: &str) -> Result<Self, SourceError> {
        let stored: StoredState = serde_json::from_str(input)
            .map_err(|error| SourceError::Invalid(format!("invalid rawlog state JSON: {error}")))?;
        if !matches!(stored.version, 1 | 2) {
            return Err(SourceError::Invalid(format!(
                "unsupported rawlog state version {}",
                stored.version
            )));
        }
        let network = Network::from_venue(&stored.network).ok_or_else(|| {
            SourceError::Invalid(format!("invalid rawlog state network {:?}", stored.network))
        })?;
        let mut books = BTreeMap::new();
        for (instrument_id, stored_book) in stored.books {
            let snapshot_time = match stored.version {
                1 => None,
                _ => stored_book.snapshot_time_ns.map(Timestamp::from_nanos),
            };
            let book = if stored_book.synced {
                L2Book::from_snapshot(
                    &L2Snapshot {
                        bids: restore_levels(stored_book.bids),
                        asks: restore_levels(stored_book.asks),
                    },
                    snapshot_time,
                )
                .map_err(|error| {
                    SourceError::Invalid(format!(
                        "invalid stored book for instrument {instrument_id}: {error}"
                    ))
                })?
            } else if stored_book.bids.is_empty()
                && stored_book.asks.is_empty()
                && snapshot_time.is_none()
            {
                L2Book::new()
            } else {
                return Err(SourceError::Invalid(format!(
                    "unsynced stored book for instrument {instrument_id} has levels or a snapshot time"
                )));
            };
            books.insert(instrument_id, book);
        }
        Ok(Self {
            network,
            // Older writers used null for both an unseen and an ended connection.
            // Carried null retains its original behavior of suppressing trades.
            cutoff: match stored.connected_at {
                Some(time) => TradeCutoff::ConnectedSince(Timestamp::from_nanos(time)),
                None => TradeCutoff::Disconnected,
            },
            books,
        })
    }
}
