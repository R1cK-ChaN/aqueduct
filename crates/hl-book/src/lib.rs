//! Order book reconstruction.
//!
//! [`L2Book`] keeps the visible price levels of one instrument, built from
//! snapshots and diffs. It is shared by the data pipeline, the backtester and
//! live trading, so all three see the same book for the same events.
//!
//! An L4 book keyed by order id is planned and not implemented yet.
//!
//! The diff semantics and how they were confirmed are in
//! `docs/rfc/0003-hyperliquid-market-data-decoding.md`.

use core::fmt;

use hl_types::{L2Diff, L2Snapshot, Level, MarketEvent, MarketEventKind, Side, Timestamp};

/// Whether a diff was applied or was already included in the snapshot (HL-75).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffOutcome {
    /// The diff was applied to the current levels.
    Applied,
    /// The diff's exchange time is at or before the source snapshot's time (HL-75).
    SkippedStale,
}

/// Why a snapshot or diff could not be applied. After any error the book is
/// out of sync until the next valid snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookError {
    /// A diff arrived while the book had no valid snapshot to build on.
    NotSynced,
    /// A diff removes a position the book does not have.
    PositionOutOfRange {
        /// Side of the book.
        side: Side,
        /// The position the diff named.
        position: u32,
        /// How many levels that side had.
        levels: usize,
    },
    /// A level has a price or size that is not positive.
    InvalidLevel {
        /// Side of the book.
        side: Side,
    },
    /// A snapshot's levels are not strictly ordered best price first.
    Unsorted {
        /// Side of the book.
        side: Side,
    },
    /// The best bid is not below the best ask.
    Crossed,
}

impl fmt::Display for BookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotSynced => f.write_str("diff received before a snapshot"),
            Self::PositionOutOfRange {
                side,
                position,
                levels,
            } => {
                write!(
                    f,
                    "diff removes {side:?} position {position} of {levels} levels"
                )
            }
            Self::InvalidLevel { side } => write!(f, "{side:?} level is not positive"),
            Self::Unsorted { side } => write!(f, "{side:?} levels are not ordered best first"),
            Self::Crossed => f.write_str("best bid is not below best ask"),
        }
    }
}

impl std::error::Error for BookError {}

/// The visible price levels of one instrument.
///
/// Bids are held highest price first and asks lowest price first, so
/// position 0 is the best level on each side (HL-77). Prices beyond the last level
/// are unknown, not empty.
///
/// The book is **in sync** between a valid snapshot and the next problem.
/// Diffs remove levels by position, so they only make sense on top of the
/// exact book the venue had (HL-71): the book refuses diffs when it is not in sync,
/// and falls out of sync on a feed gap or any inconsistent input. While out
/// of sync it is empty.
#[derive(Debug, Clone, Default)]
pub struct L2Book {
    bids: Vec<Level>,
    asks: Vec<Level>,
    synced: bool,
    snapshot_time: Option<Timestamp>,
    /// Scratch space for marking removed positions.
    removed: Vec<bool>,
}

fn is_positive(level: &Level) -> bool {
    level.px.is_positive() && level.qty.is_positive()
}

/// True when `better` belongs nearer the top of `side` than `other`.
fn is_better(side: Side, better: &Level, other: &Level) -> bool {
    match side {
        Side::Buy => better.px > other.px,
        Side::Sell => better.px < other.px,
    }
}

fn check_snapshot_side(side: Side, levels: &[Level]) -> Result<(), BookError> {
    if !levels.iter().all(is_positive) {
        return Err(BookError::InvalidLevel { side });
    }
    if !levels
        .windows(2)
        .all(|pair| is_better(side, &pair[0], &pair[1]))
    {
        return Err(BookError::Unsorted { side });
    }
    Ok(())
}

fn check_not_crossed(bids: &[Level], asks: &[Level]) -> Result<(), BookError> {
    match (bids.first(), asks.first()) {
        (Some(bid), Some(ask)) if bid.px >= ask.px => Err(BookError::Crossed),
        _ => Ok(()),
    }
}

fn apply_diff_side(
    side: Side,
    levels: &mut Vec<Level>,
    removed: &mut Vec<bool>,
    remove: &[u32],
    set: &[Level],
) -> Result<(), BookError> {
    // Positions refer to the book before this diff, so mark them all before
    // removing anything.
    removed.clear();
    removed.resize(levels.len(), false);
    for &position in remove {
        let mark = usize::try_from(position)
            .ok()
            .and_then(|index| removed.get_mut(index));
        match mark {
            Some(mark) => *mark = true,
            None => {
                return Err(BookError::PositionOutOfRange {
                    side,
                    position,
                    levels: levels.len(),
                });
            }
        }
    }
    let mut marks = removed.iter();
    levels.retain(|_| !marks.next().copied().unwrap_or(false));

    for level in set {
        if !is_positive(level) {
            return Err(BookError::InvalidLevel { side });
        }
        let at = levels.partition_point(|existing| is_better(side, existing, level));
        match levels.get_mut(at) {
            Some(existing) if existing.px == level.px => *existing = *level,
            _ => levels.insert(at, *level),
        }
    }
    Ok(())
}

impl L2Book {
    /// An empty book that is not in sync.
    pub fn new() -> Self {
        Self::default()
    }

    /// Restores stored levels and the time of their source snapshot.
    ///
    /// Older stored state may lack that time. With `None`, diffs are applied
    /// without a stale-time check until the next snapshot supplies a time.
    pub fn from_snapshot(
        snapshot: &L2Snapshot,
        snapshot_time: Option<Timestamp>,
    ) -> Result<Self, BookError> {
        let mut book = Self::new();
        book.replace_snapshot(snapshot, snapshot_time)?;
        Ok(book)
    }

    /// True when the book reflects the venue's book as of the last applied
    /// snapshot or diff.
    pub fn is_synced(&self) -> bool {
        self.synced
    }

    /// Exchange time of the snapshot the book was built from. Applied diffs
    /// do not advance it. Unknown for older restored state and while out of sync.
    pub fn snapshot_time(&self) -> Option<Timestamp> {
        self.snapshot_time
    }

    /// Bid levels, best first. Empty while out of sync.
    pub fn bids(&self) -> &[Level] {
        &self.bids
    }

    /// Ask levels, best first. Empty while out of sync.
    pub fn asks(&self) -> &[Level] {
        &self.asks
    }

    /// The highest bid.
    pub fn best_bid(&self) -> Option<&Level> {
        self.bids.first()
    }

    /// The lowest ask.
    pub fn best_ask(&self) -> Option<&Level> {
        self.asks.first()
    }

    /// The current book as a snapshot, or `None` while out of sync.
    pub fn to_snapshot(&self) -> Option<L2Snapshot> {
        self.synced.then(|| L2Snapshot {
            bids: self.bids.clone(),
            asks: self.asks.clone(),
        })
    }

    /// Drops the book out of sync and empties it. The next snapshot brings
    /// it back.
    pub fn desync(&mut self) {
        self.bids.clear();
        self.asks.clear();
        self.synced = false;
        self.snapshot_time = None;
    }

    /// Applies a snapshot or diff at its exchange time. A feed gap drops the
    /// book out of sync. Only diffs return an outcome. Other events return
    /// `None`.
    pub fn apply(&mut self, event: &MarketEvent) -> Result<Option<DiffOutcome>, BookError> {
        match &event.kind {
            MarketEventKind::L2Snapshot(snapshot) => {
                self.apply_snapshot(snapshot, event.ts_exch)?;
                Ok(None)
            }
            MarketEventKind::L2Diff(diff) => self.apply_diff(diff, event.ts_exch).map(Some),
            MarketEventKind::FeedGap => {
                self.desync();
                Ok(None)
            }
            MarketEventKind::Bbo(_) | MarketEventKind::Trade(_) => Ok(None),
        }
    }

    /// Replaces the book with `snapshot` and brings it into sync.
    ///
    /// The snapshot must have positive prices and sizes, be strictly ordered
    /// best price first on both sides (HL-77), and not be crossed.
    pub fn apply_snapshot(
        &mut self,
        snapshot: &L2Snapshot,
        ts_exch: Timestamp,
    ) -> Result<(), BookError> {
        self.replace_snapshot(snapshot, Some(ts_exch))
    }

    fn replace_snapshot(
        &mut self,
        snapshot: &L2Snapshot,
        snapshot_time: Option<Timestamp>,
    ) -> Result<(), BookError> {
        self.desync();
        check_snapshot_side(Side::Buy, &snapshot.bids)?;
        check_snapshot_side(Side::Sell, &snapshot.asks)?;
        check_not_crossed(&snapshot.bids, &snapshot.asks)?;
        self.bids.extend_from_slice(&snapshot.bids);
        self.asks.extend_from_slice(&snapshot.asks);
        self.synced = true;
        self.snapshot_time = snapshot_time;
        Ok(())
    }

    /// Applies `diff` on top of the current book, or skips it when its exchange
    /// time is at or before the source snapshot's time (HL-75). A skipped diff leaves
    /// the book unchanged and in sync. Later diffs use the same reference time.
    pub fn apply_diff(
        &mut self,
        diff: &L2Diff,
        ts_exch: Timestamp,
    ) -> Result<DiffOutcome, BookError> {
        if !self.synced {
            return Err(BookError::NotSynced);
        }
        if self.snapshot_time.is_some_and(|time| ts_exch <= time) {
            return Ok(DiffOutcome::SkippedStale);
        }
        let result = self.try_apply_diff(diff);
        if result.is_err() {
            self.desync();
        }
        result.map(|()| DiffOutcome::Applied)
    }

    fn try_apply_diff(&mut self, diff: &L2Diff) -> Result<(), BookError> {
        let removed = &mut self.removed;
        apply_diff_side(
            Side::Buy,
            &mut self.bids,
            removed,
            &diff.remove_bids,
            &diff.set_bids,
        )?;
        apply_diff_side(
            Side::Sell,
            &mut self.asks,
            removed,
            &diff.remove_asks,
            &diff.set_asks,
        )?;
        check_not_crossed(&self.bids, &self.asks)
    }
}

#[cfg(test)]
mod tests {
    use hl_types::{Bbo, InstrumentId, Px, Qty, Trade, TradeId};

    use super::*;

    const SNAPSHOT_TIME: Timestamp = Timestamp::from_millis(1_000);
    const DIFF_TIME: Timestamp = Timestamp::from_millis(1_001);

    fn level(price: &str, size: &str, orders: u32) -> Level {
        Level {
            px: price.parse().unwrap(),
            qty: size.parse().unwrap(),
            orders,
        }
    }

    fn levels(side: &[(&str, &str)]) -> Vec<Level> {
        side.iter()
            .map(|(price, size)| level(price, size, 2))
            .collect()
    }

    fn snapshot() -> L2Snapshot {
        L2Snapshot {
            bids: levels(&[("100", "1"), ("99", "2"), ("98", "3")]),
            asks: levels(&[("101", "4"), ("102", "5"), ("103", "6")]),
        }
    }

    fn synced() -> L2Book {
        let mut book = L2Book::new();
        book.apply_snapshot(&snapshot(), SNAPSHOT_TIME).unwrap();
        book
    }

    fn prices(side: &[Level]) -> Vec<String> {
        side.iter().map(|level| level.px.to_string()).collect()
    }

    fn event(kind: MarketEventKind, ts_exch: Timestamp) -> MarketEvent {
        MarketEvent {
            instrument: InstrumentId::new(1),
            ts_exch,
            ts_local: Timestamp::from_millis(100_000),
            kind,
        }
    }

    #[test]
    fn a_new_book_is_empty_and_refuses_diffs() {
        let mut book = L2Book::new();
        assert!(!book.is_synced());
        assert_eq!(book.snapshot_time(), None);
        assert_eq!(
            (book.best_bid(), book.best_ask(), book.to_snapshot()),
            (None, None, None)
        );
        assert_eq!(
            book.apply_diff(&L2Diff::default(), DIFF_TIME),
            Err(BookError::NotSynced)
        );
    }

    #[test]
    fn a_snapshot_replaces_the_book() {
        let mut book = synced();
        assert!(book.is_synced());
        assert_eq!(book.best_bid(), Some(&level("100", "1", 2)));
        assert_eq!(book.best_ask(), Some(&level("101", "4", 2)));
        assert_eq!(book.to_snapshot(), Some(snapshot()));

        let narrower = L2Snapshot {
            bids: levels(&[("50", "1")]),
            asks: vec![],
        };
        book.apply_snapshot(&narrower, DIFF_TIME).unwrap();
        assert_eq!(book.to_snapshot(), Some(narrower));
        assert_eq!(book.snapshot_time(), Some(DIFF_TIME));
    }

    #[test]
    fn a_diff_removes_by_old_position_then_sets_levels() {
        let mut book = synced();
        book.apply_diff(
            &L2Diff {
                // Positions are those of the book before the diff, in any order.
                remove_bids: vec![2, 0],
                remove_asks: vec![],
                set_bids: vec![level("99.5", "7", 0), level("97", "8", 0)],
                set_asks: vec![level("102", "9", 0), level("100.5", "1", 0)],
            },
            DIFF_TIME,
        )
        .unwrap();

        assert_eq!(prices(book.bids()), ["99.5", "99", "97"]);
        assert_eq!(prices(book.asks()), ["100.5", "101", "102", "103"]);
        assert_eq!(
            book.asks()[2],
            level("102", "9", 0),
            "an existing price gets the new size"
        );
        assert_eq!(
            book.asks()[1],
            level("101", "4", 2),
            "untouched levels keep their order count"
        );
    }

    #[test]
    fn a_recorded_diff_applies_as_the_venue_describes() {
        // Shape of a real `l2` diff: one level leaves each side by position
        // and one new price enters each side (HL-71, HL-72).
        let mut book = synced();
        book.apply_diff(
            &L2Diff {
                remove_bids: vec![1],
                remove_asks: vec![2],
                set_bids: vec![level("99.9", "3000", 0)],
                set_asks: vec![level("101.5", "3000", 0)],
            },
            DIFF_TIME,
        )
        .unwrap();
        assert_eq!(prices(book.bids()), ["100", "99.9", "98"]);
        assert_eq!(prices(book.asks()), ["101", "101.5", "102"]);
    }

    #[test]
    fn an_empty_diff_changes_nothing() {
        let mut book = synced();
        book.apply_diff(&L2Diff::default(), DIFF_TIME).unwrap();
        assert_eq!(book.to_snapshot(), Some(snapshot()));
    }

    #[test]
    fn same_time_and_older_diffs_are_skipped_without_changing_the_book() {
        let diff = L2Diff {
            remove_bids: vec![0],
            remove_asks: vec![2],
            ..L2Diff::default()
        };
        for time in [SNAPSHOT_TIME, Timestamp::from_millis(999)] {
            let mut book = synced();
            assert_eq!(book.apply_diff(&diff, time), Ok(DiffOutcome::SkippedStale));
            assert_eq!(book.to_snapshot(), Some(snapshot()));
            assert_eq!(book.snapshot_time(), Some(SNAPSHOT_TIME));
            assert!(book.is_synced());
        }
    }

    #[test]
    fn a_stale_diff_is_skipped_before_validating_its_positions() {
        let mut book = synced();
        let diff = L2Diff {
            remove_asks: vec![3],
            ..L2Diff::default()
        };
        assert_eq!(
            book.apply_diff(&diff, SNAPSHOT_TIME),
            Ok(DiffOutcome::SkippedStale)
        );
        assert_eq!(book.to_snapshot(), Some(snapshot()));
        assert_eq!(book.snapshot_time(), Some(SNAPSHOT_TIME));
        assert!(book.is_synced());
    }

    #[test]
    fn later_diffs_apply_without_advancing_the_snapshot_reference() {
        let mut book = synced();
        let diff = L2Diff {
            remove_bids: vec![0],
            ..L2Diff::default()
        };
        for time in [
            Timestamp::from_millis(1_002),
            Timestamp::from_millis(1_002),
            DIFF_TIME,
        ] {
            assert_eq!(book.apply_diff(&diff, time), Ok(DiffOutcome::Applied));
            assert_eq!(book.snapshot_time(), Some(SNAPSHOT_TIME));
            assert!(book.is_synced());
        }
        assert!(book.bids().is_empty());
        assert_eq!(book.asks(), snapshot().asks);
    }

    #[test]
    fn a_new_snapshot_resets_the_time_used_to_skip_diffs() {
        let mut book = synced();
        let diff = L2Diff {
            remove_bids: vec![0],
            ..L2Diff::default()
        };
        assert_eq!(book.apply_diff(&diff, DIFF_TIME), Ok(DiffOutcome::Applied));

        book.apply_snapshot(&snapshot(), DIFF_TIME).unwrap();
        assert_eq!(book.snapshot_time(), Some(DIFF_TIME));
        assert_eq!(
            book.apply_diff(&diff, DIFF_TIME),
            Ok(DiffOutcome::SkippedStale)
        );
        assert_eq!(book.to_snapshot(), Some(snapshot()));
        assert_eq!(
            book.apply_diff(&diff, Timestamp::from_millis(1_002)),
            Ok(DiffOutcome::Applied)
        );
        assert_eq!(prices(book.bids()), ["99", "98"]);

        book.apply_snapshot(&snapshot(), SNAPSHOT_TIME).unwrap();
        assert_eq!(book.apply_diff(&diff, DIFF_TIME), Ok(DiffOutcome::Applied));
    }

    #[test]
    fn restored_books_use_their_snapshot_time_when_known() {
        let diff = L2Diff {
            remove_bids: vec![0],
            ..L2Diff::default()
        };
        let mut known = L2Book::from_snapshot(&snapshot(), Some(SNAPSHOT_TIME)).unwrap();
        assert_eq!(
            known.apply_diff(&diff, SNAPSHOT_TIME),
            Ok(DiffOutcome::SkippedStale)
        );
        assert_eq!(known.to_snapshot(), Some(snapshot()));

        let mut unknown = L2Book::from_snapshot(&snapshot(), None).unwrap();
        assert!(unknown.is_synced());
        assert_eq!(unknown.snapshot_time(), None);
        assert_eq!(
            unknown.apply_diff(&diff, SNAPSHOT_TIME),
            Ok(DiffOutcome::Applied)
        );
        assert_eq!(prices(unknown.bids()), ["99", "98"]);
        unknown.apply_snapshot(&snapshot(), DIFF_TIME).unwrap();
        assert_eq!(
            unknown.apply_diff(&diff, DIFF_TIME),
            Ok(DiffOutcome::SkippedStale)
        );
        assert_eq!(unknown.snapshot_time(), Some(DIFF_TIME));
        assert_eq!(unknown.to_snapshot(), Some(snapshot()));
    }

    #[test]
    fn a_bad_position_drops_the_book_out_of_sync() {
        let mut book = synced();
        let diff = L2Diff {
            remove_asks: vec![3],
            ..L2Diff::default()
        };
        assert_eq!(
            book.apply_diff(&diff, DIFF_TIME),
            Err(BookError::PositionOutOfRange {
                side: Side::Sell,
                position: 3,
                levels: 3
            })
        );
        assert!(!book.is_synced());
        assert!(book.bids().is_empty() && book.asks().is_empty());
        assert_eq!(book.snapshot_time(), None);
        assert_eq!(
            book.apply_diff(&L2Diff::default(), DIFF_TIME),
            Err(BookError::NotSynced)
        );

        book.apply_snapshot(&snapshot(), DIFF_TIME).unwrap();
        assert!(book.is_synced(), "the next snapshot recovers the book");
    }

    #[test]
    fn a_diff_that_crosses_the_book_is_rejected() {
        let mut book = synced();
        let diff = L2Diff {
            set_bids: vec![level("101", "1", 0)],
            ..L2Diff::default()
        };
        assert_eq!(book.apply_diff(&diff, DIFF_TIME), Err(BookError::Crossed));
        assert!(!book.is_synced());
        assert_eq!(book.snapshot_time(), None);
    }

    #[test]
    fn a_diff_with_a_non_positive_level_is_rejected() {
        for bad in [
            level("99", "0", 0),
            level("0", "1", 0),
            level("99", "-1", 0),
        ] {
            let mut book = synced();
            let diff = L2Diff {
                set_bids: vec![bad],
                ..L2Diff::default()
            };
            assert_eq!(
                book.apply_diff(&diff, DIFF_TIME),
                Err(BookError::InvalidLevel { side: Side::Buy })
            );
            assert!(!book.is_synced());
            assert_eq!(book.snapshot_time(), None);
        }
    }

    #[test]
    fn invalid_snapshots_are_rejected() {
        let mut unsorted = snapshot();
        unsorted.asks.swap(0, 1);
        let mut duplicate = snapshot();
        duplicate.bids[1].px = duplicate.bids[0].px;
        let mut crossed = snapshot();
        crossed.asks[0].px = crossed.bids[0].px;
        let mut zero = snapshot();
        zero.bids[2].qty = Qty::ZERO;
        let mut negative = snapshot();
        negative.asks[0].px = Px::from_e8(-1);

        let cases = [
            (unsorted, BookError::Unsorted { side: Side::Sell }),
            (duplicate, BookError::Unsorted { side: Side::Buy }),
            (crossed, BookError::Crossed),
            (zero, BookError::InvalidLevel { side: Side::Buy }),
            (negative, BookError::InvalidLevel { side: Side::Sell }),
        ];
        for (bad, expected) in cases {
            let mut book = synced();
            assert_eq!(book.apply_snapshot(&bad, DIFF_TIME), Err(expected));
            assert!(
                !book.is_synced(),
                "a rejected snapshot does not leave the old book behind"
            );
            assert_eq!(book.snapshot_time(), None);
        }
    }

    #[test]
    fn one_sided_and_empty_books_are_valid() {
        let mut book = L2Book::new();
        book.apply_snapshot(&L2Snapshot::default(), SNAPSHOT_TIME)
            .unwrap();
        assert!(book.is_synced());
        let diff = L2Diff {
            set_asks: vec![level("5", "1", 0)],
            ..L2Diff::default()
        };
        book.apply_diff(&diff, DIFF_TIME).unwrap();
        assert_eq!(
            (book.best_bid(), book.best_ask()),
            (None, Some(&level("5", "1", 0)))
        );
    }

    #[test]
    fn events_drive_the_book() {
        let mut book = L2Book::new();
        assert_eq!(
            book.apply(&event(
                MarketEventKind::L2Snapshot(snapshot()),
                SNAPSHOT_TIME
            )),
            Ok(None)
        );
        for kind in [
            MarketEventKind::Bbo(Bbo::default()),
            MarketEventKind::Trade(Trade {
                tid: TradeId(1),
                px: "100".parse().unwrap(),
                qty: "1".parse().unwrap(),
                aggressor: Side::Buy,
            }),
        ] {
            assert_eq!(book.apply(&event(kind, DIFF_TIME)), Ok(None));
        }
        assert!(book.is_synced(), "other event kinds are ignored");
        assert_eq!(book.to_snapshot(), Some(snapshot()));
        assert_eq!(book.snapshot_time(), Some(SNAPSHOT_TIME));

        let diff = L2Diff {
            remove_bids: vec![0],
            ..L2Diff::default()
        };
        for time in [SNAPSHOT_TIME, Timestamp::from_millis(999)] {
            assert_eq!(
                book.apply(&event(MarketEventKind::L2Diff(diff.clone()), time)),
                Ok(Some(DiffOutcome::SkippedStale))
            );
            assert_eq!(book.to_snapshot(), Some(snapshot()));
            assert!(book.is_synced());
        }
        assert_eq!(
            book.apply(&event(MarketEventKind::L2Diff(diff), DIFF_TIME)),
            Ok(Some(DiffOutcome::Applied))
        );
        assert_eq!(prices(book.bids()), ["99", "98"]);

        assert_eq!(
            book.apply(&event(MarketEventKind::FeedGap, DIFF_TIME)),
            Ok(None)
        );
        assert!(
            !book.is_synced(),
            "a feed gap means diffs may have been missed"
        );
        assert_eq!(book.snapshot_time(), None);
        assert_eq!(
            book.apply(&event(
                MarketEventKind::L2Diff(L2Diff::default()),
                SNAPSHOT_TIME
            )),
            Err(BookError::NotSynced)
        );
    }
}
