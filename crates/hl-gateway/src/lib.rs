//! The order gateway interface.
//!
//! A gateway turns [`OrderAction`]s into [`OrderEvent`]s. The simulator in
//! `hl-sim` implements it for backtests and paper trading; the Hyperliquid
//! client will implement it for live trading, in this crate behind a
//! non-default cargo feature so that the simulator never pulls in signing or
//! network dependencies.
//!
//! Strategies never see a gateway. A driver takes the actions a
//! `StrategyHost` returns, passes them to [`OrderGateway::submit`], and feeds
//! the reports from [`OrderGateway::poll_events`] back to the host.
//!
//! The reasons behind this shape are in
//! `docs/rfc/0001-core-types-and-interfaces.md`.

use hl_types::{OrderAction, OrderEvent, Timestamp};

/// Sends order actions to a venue and returns the venue's reports.
///
/// # Contract
///
/// **Time.** Implementations never read a clock. The caller passes the
/// current local time as `now`, and `now` never decreases from one call to
/// the next, across both methods.
///
/// **No synchronous failure.** `submit` does not block and does not return an
/// error. Every outcome, including a request the gateway refuses before it
/// reaches the venue, is reported as an [`OrderEvent`].
///
/// **Outcomes.** Each action leads to these reports:
///
/// | Action | Reports |
/// | --- | --- |
/// | `Place` | exactly one `Accepted` or `Rejected` |
/// | `Modify` | exactly one `Modified` or `ModifyRejected` |
/// | `Cancel` | exactly one `Canceled` or `CancelRejected` |
/// | `CancelAll` | one `Canceled` per order removed, none if there were none |
///
/// A live order can also receive `Filled` at any time, including while a
/// modify or cancel for it is in flight, and `Canceled` with
/// [`hl_types::CancelReason::Venue`] when the venue removes it (HL-36).
///
/// **Identity.** A cloid is placed at most once. A `Place` that reuses one is
/// rejected with [`hl_types::RejectReason::DuplicateClientOrderId`] and has
/// no effect on the original order.
///
/// **Order.** Actions in one `submit` call are processed in slice order, and
/// earlier calls before later ones. Reports about one order are delivered in
/// the order the venue produced them.
pub trait OrderGateway {
    /// Sends `actions` to the venue.
    fn submit(&mut self, now: Timestamp, actions: &[OrderAction]);

    /// Appends to `out` every report the strategy can have seen by `now`,
    /// that is, every pending report with `ts_local <= now`, oldest first.
    /// Each report is returned once.
    fn poll_events(&mut self, now: Timestamp, out: &mut Vec<OrderEvent>);
}
