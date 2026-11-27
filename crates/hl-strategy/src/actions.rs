//! The sink a strategy writes its decisions into.

use hl_types::{ClientOrderId, InstrumentId, OrderAction, OrderSpec, Px, Qty, TimerId, Timestamp};

/// A request to be called back at a given time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimerRequest {
    /// Earliest time the timer may fire. A time in the past fires as soon as
    /// possible.
    pub at: Timestamp,
    /// Label returned in the [`hl_types::TimerEvent`].
    pub id: TimerId,
}

/// What a strategy decided during one callback.
///
/// This is the only way a strategy can act. The driver routes order actions
/// to the gateway and timer requests to its scheduler after the callback
/// returns; nothing happens while the strategy is still running.
#[derive(Debug)]
pub struct Actions {
    strategy_tag: u8,
    now: Timestamp,
    next_seq: u64,
    orders: Vec<OrderAction>,
    timers: Vec<TimerRequest>,
}

impl Actions {
    pub(crate) fn new(strategy_tag: u8) -> Self {
        Self {
            strategy_tag,
            now: Timestamp::EPOCH,
            next_seq: 0,
            orders: Vec::new(),
            timers: Vec::new(),
        }
    }

    /// Clears the previous callback's output and sets the time used for the
    /// ids of orders placed in the next one.
    pub(crate) fn begin(&mut self, now: Timestamp) {
        self.now = now;
        self.orders.clear();
        self.timers.clear();
    }

    /// Places a new order and returns the id it will be known by.
    ///
    /// The id is derived from the current event time and a counter, so the
    /// same event stream always yields the same ids.
    pub fn place(&mut self, order: OrderSpec) -> ClientOrderId {
        let cloid =
            ClientOrderId::from_parts(self.strategy_tag, order.instrument, self.now, self.next_seq);
        self.next_seq += 1;
        self.orders.push(OrderAction::Place { cloid, order });
        cloid
    }

    /// Changes the price and open quantity of a live order.
    pub fn modify(&mut self, instrument: InstrumentId, cloid: ClientOrderId, px: Px, qty: Qty) {
        self.orders.push(OrderAction::Modify {
            instrument,
            cloid,
            px,
            qty,
        });
    }

    /// Cancels one live order.
    pub fn cancel(&mut self, instrument: InstrumentId, cloid: ClientOrderId) {
        self.orders.push(OrderAction::Cancel { instrument, cloid });
    }

    /// Cancels every live order, or every live order on one instrument.
    pub fn cancel_all(&mut self, instrument: Option<InstrumentId>) {
        self.orders.push(OrderAction::CancelAll { instrument });
    }

    /// Asks for a [`hl_types::TimerEvent`] carrying `id` at or after `at`.
    /// Timers fire once and cannot be canceled; ignore ids that are no
    /// longer wanted.
    pub fn set_timer(&mut self, at: Timestamp, id: TimerId) {
        self.timers.push(TimerRequest { at, id });
    }

    /// Order actions requested in this callback, in request order.
    pub fn order_actions(&self) -> &[OrderAction] {
        &self.orders
    }

    /// Timers requested in this callback, in request order.
    pub fn timer_requests(&self) -> &[TimerRequest] {
        &self.timers
    }

    /// True when the strategy asked for nothing.
    pub fn is_empty(&self) -> bool {
        self.orders.is_empty() && self.timers.is_empty()
    }
}
