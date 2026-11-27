//! The events a strategy receives: market data, order reports and timers.

use crate::fixed::{Px, Qty};
use crate::instrument::InstrumentId;
use crate::order::{ClientOrderId, Fill, OrderId, Side, TradeId};
use crate::time::Timestamp;

/// One price level of an L2 book (HL-76).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Level {
    /// Price of the level.
    pub px: Px,
    /// Total size resting at the price.
    pub qty: Qty,
    /// Number of orders at the price, or 0 when the source does not report
    /// it (levels set by an [`L2Diff`], HL-74, HL-76).
    pub orders: u32,
}

/// A full replacement of the visible book, best level first on each side (HL-76, HL-77).
///
/// Prices beyond the last level are unknown, not empty.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct L2Snapshot {
    /// Bid levels, highest price first (HL-77).
    pub bids: Vec<Level>,
    /// Ask levels, lowest price first (HL-77).
    pub asks: Vec<Level>,
}

/// A change to the visible book since the previous snapshot or diff.
///
/// Apply it in two steps: remove the levels at the given positions of the
/// book as it was before this diff (position 0 is the best level), then set
/// each listed level, replacing the size at an existing price or inserting a
/// new price. Positions are in no particular order. A diff never deletes by
/// setting a size of zero (HL-71, HL-72, HL-73).
///
/// Removal is by position, so a diff is only meaningful on top of the exact
/// book the venue had in mind. After a missed message the book must be
/// rebuilt from the next snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct L2Diff {
    /// Positions of bid levels to remove.
    pub remove_bids: Vec<u32>,
    /// Positions of ask levels to remove.
    pub remove_asks: Vec<u32>,
    /// Bid levels to set.
    pub set_bids: Vec<Level>,
    /// Ask levels to set.
    pub set_asks: Vec<Level>,
}

/// The best bid and best ask. A side is `None` when it is empty (HL-78).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Bbo {
    /// Best bid.
    pub bid: Option<Level>,
    /// Best ask.
    pub ask: Option<Level>,
}

/// One public trade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trade {
    /// Trade id.
    pub tid: TradeId,
    /// Execution price.
    pub px: Px,
    /// Executed size. Positive.
    pub qty: Qty,
    /// Side of the order that crossed the spread (HL-83).
    pub aggressor: Side,
}

/// What a market event carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarketEventKind {
    /// The visible book was replaced.
    L2Snapshot(L2Snapshot),
    /// The visible book changed relative to the previous snapshot or diff.
    L2Diff(L2Diff),
    /// The best bid or ask changed (HL-79).
    Bbo(Bbo),
    /// A trade printed.
    Trade(Trade),
    /// Data for the instrument is missing from this point until the next
    /// snapshot. Replay emits it for holes in recorded data; the live feed
    /// emits it when a stream goes stale or disconnects. The book must be
    /// treated as unreliable until then.
    FeedGap,
}

/// One piece of market data for one instrument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketEvent {
    /// Instrument the data is about.
    pub instrument: InstrumentId,
    /// Exchange (block) time of the data (HL-95).
    pub ts_exch: Timestamp,
    /// Time the strategy sees the data.
    pub ts_local: Timestamp,
    /// The data.
    pub kind: MarketEventKind,
}

/// Why the venue or the gateway refused a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RejectReason {
    /// A post-only order would have traded on arrival (HL-15).
    WouldCross,
    /// The order value is below the venue minimum (HL-27).
    BelowMinNotional,
    /// Price or size is not acceptable: not positive, or too large to value.
    InvalidOrder,
    /// A `Place` reused a cloid.
    DuplicateClientOrderId,
    /// The target of a modify or cancel is not a live order.
    UnknownOrder,
    /// The gateway does not implement the requested order type.
    Unsupported,
}

/// Why an order was canceled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CancelReason {
    /// We asked for it, with `Cancel` or `CancelAll`.
    Requested,
    /// The venue canceled it without a request from us (HL-36).
    Venue,
}

/// What happened to an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderEventKind {
    /// The order was accepted and is live.
    Accepted {
        /// Id the venue assigned (HL-50).
        oid: OrderId,
    },
    /// The place request was refused. The order never became live.
    Rejected {
        /// Why.
        reason: RejectReason,
    },
    /// Part or all of the order traded.
    Filled(Fill),
    /// A modify took effect.
    Modified {
        /// Limit price now in force.
        px: Px,
        /// Open quantity now in force.
        qty: Qty,
    },
    /// A modify was refused. The order is unchanged.
    ModifyRejected {
        /// Why.
        reason: RejectReason,
    },
    /// The order is no longer live.
    Canceled {
        /// Why.
        reason: CancelReason,
    },
    /// A cancel was refused. If the order was still live it is unchanged.
    CancelRejected {
        /// Why.
        reason: RejectReason,
    },
}

/// A report about one of our orders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrderEvent {
    /// Instrument of the order.
    pub instrument: InstrumentId,
    /// The order the report is about.
    pub cloid: ClientOrderId,
    /// Exchange time at which the change took effect.
    pub ts_exch: Timestamp,
    /// Time the strategy sees the report.
    pub ts_local: Timestamp,
    /// What happened.
    pub kind: OrderEventKind,
}

/// A label a strategy attaches to a timer so it can tell its timers apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TimerId(pub u64);

/// A timer the strategy asked for has fired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimerEvent {
    /// The label given when the timer was set.
    pub id: TimerId,
    /// Time the timer fired.
    pub ts_local: Timestamp,
}

/// Everything a strategy can be told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Market data.
    Market(MarketEvent),
    /// A report about one of our orders.
    Order(OrderEvent),
    /// A timer fired.
    Timer(TimerEvent),
}

impl Event {
    /// The time at which the strategy sees this event.
    pub fn ts_local(&self) -> Timestamp {
        match self {
            Self::Market(event) => event.ts_local,
            Self::Order(event) => event.ts_local,
            Self::Timer(event) => event.ts_local,
        }
    }
}
