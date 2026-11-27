//! The strategy interface and its IO-free runtime.
//!
//! A [`Strategy`] receives [`hl_types::Event`]s one at a time, reads state
//! through a [`Context`] and requests actions through [`Actions`]. It has no
//! other way to observe or affect the world, which is what lets one
//! implementation run unchanged in backtest, paper trading and live trading.
//!
//! [`StrategyHost`] wraps a strategy and keeps the state behind `Context`
//! (currently the table of live orders). Drivers feed it events and route the
//! actions it returns.
//!
//! This crate depends only on `hl-types`. It must never depend on a gateway,
//! the simulator or anything that performs IO. The reasons are in
//! `docs/rfc/0001-core-types-and-interfaces.md`.

mod actions;
mod fixed_spread;
mod host;
mod orders;

pub use actions::{Actions, TimerRequest};
pub use fixed_spread::{FixedSpreadConfig, FixedSpreadQuoter};
pub use host::{Context, Strategy, StrategyHost};
pub use orders::OpenOrders;
