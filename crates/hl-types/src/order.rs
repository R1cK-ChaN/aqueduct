//! Orders, the actions a strategy can request and the fills that result.

use core::fmt;

use crate::fixed::{Px, Qty, Usd};
use crate::instrument::InstrumentId;
use crate::time::Timestamp;

/// Which side of the book an order or the aggressor of a trade is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    /// Bid side: buys the instrument.
    Buy,
    /// Ask side: sells the instrument.
    Sell,
}

impl Side {
    /// The other side.
    pub const fn opposite(self) -> Self {
        match self {
            Self::Buy => Self::Sell,
            Self::Sell => Self::Buy,
        }
    }

    /// `+1` for a buy and `-1` for a sell: the sign a fill on this side
    /// applies to a position.
    pub const fn sign(self) -> i64 {
        match self {
            Self::Buy => 1,
            Self::Sell => -1,
        }
    }
}

/// The 128-bit id we assign to an order (the venue calls it a cloid, HL-51).
///
/// It identifies the order for its whole life, including across modifies,
/// and is the only order identity a strategy uses.
///
/// Layout, most significant byte first:
///
/// | Bytes | Field |
/// | --- | --- |
/// | 1 | layout version, currently 1 |
/// | 1 | strategy tag |
/// | 2 | low 16 bits of the instrument id |
/// | 6 | millisecond timestamp |
/// | 6 | sequence number |
///
/// The time and sequence fields make the id unique. The other fields are
/// there for people reading logs and must not be parsed back into state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClientOrderId(u128);

const MASK_48: u64 = (1 << 48) - 1;

impl ClientOrderId {
    /// The layout version written by [`ClientOrderId::from_parts`].
    pub const LAYOUT_VERSION: u8 = 1;

    /// Assembles an id from its fields.
    ///
    /// `ts` is truncated to 48 bits of milliseconds (times before the epoch
    /// become 0) and `seq` to 48 bits.
    pub fn from_parts(strategy_tag: u8, instrument: InstrumentId, ts: Timestamp, seq: u64) -> Self {
        let millis = u64::try_from(ts.as_millis()).unwrap_or(0) & MASK_48;
        let instrument_tag = instrument.get() & 0xFFFF;
        Self(
            (u128::from(Self::LAYOUT_VERSION) << 120)
                | (u128::from(strategy_tag) << 112)
                | (u128::from(instrument_tag) << 96)
                | (u128::from(millis) << 48)
                | u128::from(seq & MASK_48),
        )
    }

    /// Wraps a raw 128-bit value, for ids read back from the venue or a log.
    pub const fn from_u128(raw: u128) -> Self {
        Self(raw)
    }

    /// The raw 128-bit value.
    pub const fn as_u128(self) -> u128 {
        self.0
    }

    /// The id as 16 big-endian bytes.
    pub const fn to_bytes(self) -> [u8; 16] {
        self.0.to_be_bytes()
    }

    /// The strategy tag field.
    pub const fn strategy_tag(self) -> u8 {
        self.to_bytes()[1]
    }

    /// The sequence number field.
    pub const fn seq(self) -> u64 {
        (self.0 as u64) & MASK_48
    }
}

impl fmt::Display for ClientOrderId {
    /// Formats as `0x` followed by 32 hex digits, the form the venue uses (HL-51).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:032x}", self.0)
    }
}

/// The id the venue assigns to an order once it is accepted (oid, HL-50).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OrderId(pub u64);

/// The venue's id for one trade (tid). Both sides of a trade share it (HL-85).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TradeId(pub u64);

/// How long an order stays active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TimeInForce {
    /// Add liquidity only (post-only). Canceled on arrival if it would trade,
    /// reported as `BadAloPx` (HL-15).
    Alo,
    /// Good till canceled. Trades what it can on arrival, then rests (HL-17).
    Gtc,
    /// Immediate or cancel. Trades what it can on arrival; the rest is
    /// canceled (HL-16).
    Ioc,
}

/// Everything a strategy chooses about a new order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrderSpec {
    /// Instrument to trade.
    pub instrument: InstrumentId,
    /// Buy or sell.
    pub side: Side,
    /// Limit price.
    pub px: Px,
    /// Size. Must be positive.
    pub qty: Qty,
    /// Time in force.
    pub tif: TimeInForce,
    /// When true the order may only reduce the current position (HL-22).
    pub reduce_only: bool,
}

/// A request from a strategy to the venue.
///
/// Outcomes come back as [`crate::OrderEvent`]s; the rules are documented on
/// the gateway interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderAction {
    /// Place a new order under a fresh `cloid`.
    Place {
        /// Id for the new order. Never reused.
        cloid: ClientOrderId,
        /// The order to place.
        order: OrderSpec,
    },
    /// Change the price and open quantity of a live order (HL-48). The order keeps
    /// its `cloid`.
    Modify {
        /// Instrument of the order.
        instrument: InstrumentId,
        /// The order to change.
        cloid: ClientOrderId,
        /// New limit price.
        px: Px,
        /// New open (unfilled) quantity. Must be positive.
        qty: Qty,
    },
    /// Cancel one live order.
    Cancel {
        /// Instrument of the order.
        instrument: InstrumentId,
        /// The order to cancel.
        cloid: ClientOrderId,
    },
    /// Cancel every live order, or every live order on one instrument.
    CancelAll {
        /// `Some` limits the cancel to one instrument.
        instrument: Option<InstrumentId>,
    },
}

/// Where a live order is in its life, as known locally.
///
/// Only live orders have a state. An order that is filled, canceled or
/// rejected is removed from the order table; the event that ended it is the
/// record of the outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OrderState {
    /// Place sent, no report yet.
    PendingNew,
    /// Resting on the book with nothing filled.
    Open,
    /// Resting on the book with part of it filled.
    PartiallyFilled,
    /// Modify sent, no report yet.
    PendingModify,
    /// Cancel sent, no report yet.
    PendingCancel,
}

impl OrderState {
    /// True while a request for this order is waiting for its report.
    pub const fn is_pending(self) -> bool {
        matches!(
            self,
            Self::PendingNew | Self::PendingModify | Self::PendingCancel
        )
    }
}

/// A live order as known locally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Order {
    /// Our id for the order.
    pub cloid: ClientOrderId,
    /// The venue's id, known once the order is accepted (HL-50).
    pub oid: Option<OrderId>,
    /// Instrument traded.
    pub instrument: InstrumentId,
    /// Buy or sell.
    pub side: Side,
    /// Current limit price as last confirmed by the venue, or as requested
    /// while the order is still `PendingNew`.
    pub px: Px,
    /// Quantity still open.
    pub open_qty: Qty,
    /// Quantity filled so far.
    pub filled_qty: Qty,
    /// Time in force.
    pub tif: TimeInForce,
    /// Whether the order may only reduce the position (HL-22).
    pub reduce_only: bool,
    /// Current state.
    pub state: OrderState,
}

/// Whether a fill added or removed liquidity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Liquidity {
    /// Our resting order was hit.
    Maker,
    /// Our order crossed the spread.
    Taker,
}

/// One execution of one of our orders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fill {
    /// Trade id.
    pub tid: TradeId,
    /// Venue id of our order.
    pub oid: OrderId,
    /// Side of our order.
    pub side: Side,
    /// Execution price.
    pub px: Px,
    /// Executed size. Positive.
    pub qty: Qty,
    /// Whether we were maker or taker.
    pub liquidity: Liquidity,
    /// Fee charged by the venue. Positive is a cost, negative a rebate (HL-29).
    pub fee: Usd,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn side_helpers() {
        assert_eq!(Side::Buy.opposite(), Side::Sell);
        assert_eq!(Side::Sell.opposite(), Side::Buy);
        assert_eq!(Side::Buy.sign(), 1);
        assert_eq!(Side::Sell.sign(), -1);
    }

    #[test]
    fn cloid_layout_matches_the_documented_fields() {
        let ts = Timestamp::from_millis(0x0123_4567_89AB);
        let cloid = ClientOrderId::from_parts(0x07, InstrumentId::new(0x0001_BEEF), ts, 0x42);
        assert_eq!(
            cloid.to_bytes(),
            [
                0x01, // layout version
                0x07, // strategy tag
                0xBE, 0xEF, // low 16 bits of the instrument id
                0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, // milliseconds
                0x00, 0x00, 0x00, 0x00, 0x00, 0x42, // sequence
            ]
        );
        assert_eq!(cloid.strategy_tag(), 0x07);
        assert_eq!(cloid.seq(), 0x42);
    }

    #[test]
    fn cloid_displays_as_the_venue_expects() {
        let cloid = ClientOrderId::from_u128(0x0107_beef_0123_4567_89ab_0000_0000_0042);
        assert_eq!(cloid.to_string(), "0x0107beef0123456789ab000000000042");
        assert_eq!(cloid.as_u128(), 0x0107_beef_0123_4567_89ab_0000_0000_0042);
    }

    #[test]
    fn cloid_orders_by_time_then_sequence() {
        let instrument = InstrumentId::new(1);
        let early = ClientOrderId::from_parts(1, instrument, Timestamp::from_millis(10), 9);
        let late = ClientOrderId::from_parts(1, instrument, Timestamp::from_millis(11), 0);
        let later = ClientOrderId::from_parts(1, instrument, Timestamp::from_millis(11), 1);
        assert!(early < late && late < later);
    }

    #[test]
    fn cloid_clamps_times_before_the_epoch() {
        let cloid =
            ClientOrderId::from_parts(1, InstrumentId::new(1), Timestamp::from_millis(-5), 3);
        assert_eq!(cloid.to_bytes()[4..10], [0; 6]);
    }

    #[test]
    fn pending_states() {
        assert!(OrderState::PendingNew.is_pending());
        assert!(OrderState::PendingModify.is_pending());
        assert!(OrderState::PendingCancel.is_pending());
        assert!(!OrderState::Open.is_pending());
        assert!(!OrderState::PartiallyFilled.is_pending());
    }
}
