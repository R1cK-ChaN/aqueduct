//! Shared vocabulary for the whole workspace.
//!
//! - [`Px`], [`Qty`], [`Usd`] and [`FeeRate`] are fixed-point values with
//!   eight decimal places. Accounting never uses floating point.
//! - [`Timestamp`] is the one time type. Events carry it twice: `ts_exch`
//!   (block time, HL-95) and `ts_local` (when the strategy sees the event).
//! - [`InstrumentId`] is our own stable instrument number.
//! - [`OrderAction`] is what a strategy can ask for; [`Event`] is what it is
//!   told.
//!
//! The crate has no dependencies and performs no IO. The reasons behind these
//! types are in `docs/rfc/0001-core-types-and-interfaces.md`.

#![deny(clippy::float_arithmetic)]

mod event;
mod fixed;
mod instrument;
mod order;
mod time;

pub use event::{
    Bbo, CancelReason, Event, L2Diff, L2Snapshot, Level, MarketEvent, MarketEventKind, OrderEvent,
    OrderEventKind, RejectReason, TimerEvent, TimerId, Trade,
};
pub use fixed::{DECIMALS, FeeRate, ParseFixedError, Px, Qty, SCALE, Usd};
pub use instrument::InstrumentId;
pub use order::{
    ClientOrderId, Fill, Liquidity, Order, OrderAction, OrderId, OrderSpec, OrderState, Side,
    TimeInForce, TradeId,
};
pub use time::Timestamp;
