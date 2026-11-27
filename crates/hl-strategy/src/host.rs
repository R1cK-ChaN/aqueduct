//! The strategy interface and the IO-free runtime that drives it.

use hl_types::{Event, Timestamp};

use crate::actions::Actions;
use crate::orders::OpenOrders;

/// What a strategy may read while handling an event.
///
/// Everything here is state the strategy could have observed by the event's
/// `ts_local`. There is no way to reach the venue, a clock or future data.
#[derive(Debug)]
pub struct Context<'a> {
    now: Timestamp,
    orders: &'a OpenOrders,
}

impl Context<'_> {
    /// The current local time: the `ts_local` of the event being handled.
    pub fn now(&self) -> Timestamp {
        self.now
    }

    /// The strategy's live orders, already updated with the event being
    /// handled.
    pub fn orders(&self) -> &OpenOrders {
        self.orders
    }
}

/// A trading strategy.
///
/// The same implementation runs in backtest, paper trading and live trading.
/// It receives one event at a time, reads state through `ctx` and requests
/// actions through `actions`. Actions take effect after the callback returns;
/// their outcomes arrive later as order events.
///
/// Implementations must be deterministic: the same sequence of events must
/// produce the same sequence of actions. That rules out reading a clock,
/// random numbers without a fixed seed, and any IO.
pub trait Strategy {
    /// Handles one event.
    fn on_event(&mut self, ctx: &Context<'_>, event: &Event, actions: &mut Actions);
}

/// Runs one strategy and owns the state it reads through [`Context`].
///
/// A driver (backtest replay, paper or live loop) feeds every event to
/// [`StrategyHost::on_event`] in `ts_local` order and routes the returned
/// actions: order actions to its gateway, timer requests to its scheduler.
/// Reports from the gateway come back in as [`Event::Order`].
#[derive(Debug)]
pub struct StrategyHost<S> {
    strategy: S,
    orders: OpenOrders,
    actions: Actions,
    now: Timestamp,
}

impl<S: Strategy> StrategyHost<S> {
    /// Wraps `strategy`. `strategy_tag` is written into the id of every order
    /// the strategy places, so orders from different strategies can be told
    /// apart in logs.
    pub fn new(strategy: S, strategy_tag: u8) -> Self {
        Self {
            strategy,
            orders: OpenOrders::default(),
            actions: Actions::new(strategy_tag),
            now: Timestamp::EPOCH,
        }
    }

    /// Delivers one event and returns what the strategy asked for.
    ///
    /// Order events update the order table before the strategy sees them.
    /// Orders the strategy acts on are marked pending before this returns.
    /// The returned actions are valid until the next call.
    pub fn on_event(&mut self, event: &Event) -> &Actions {
        self.now = self.now.max(event.ts_local());
        if let Event::Order(order_event) = event {
            self.orders.apply_event(order_event);
        }

        self.actions.begin(self.now);
        let ctx = Context {
            now: self.now,
            orders: &self.orders,
        };
        self.strategy.on_event(&ctx, event, &mut self.actions);

        for action in self.actions.order_actions() {
            self.orders.apply_action(action);
        }
        &self.actions
    }

    /// The wrapped strategy.
    pub fn strategy(&self) -> &S {
        &self.strategy
    }

    /// The strategy's live orders.
    pub fn orders(&self) -> &OpenOrders {
        &self.orders
    }

    /// The latest local time the host has seen.
    pub fn now(&self) -> Timestamp {
        self.now
    }
}

#[cfg(test)]
mod tests {
    use hl_types::{
        InstrumentId, MarketEvent, MarketEventKind, OrderAction, OrderEvent, OrderEventKind,
        OrderId, OrderSpec, OrderState, Px, Qty, Side, TimeInForce, TimerEvent, TimerId,
    };

    use super::*;
    use crate::actions::TimerRequest;

    const BTC: InstrumentId = InstrumentId::new(1);

    /// Places one order per market event, sets a timer on the first, and
    /// records what it saw.
    #[derive(Debug, Default)]
    struct Probe {
        seen_now: Vec<Timestamp>,
        live_orders_seen: Vec<usize>,
        timers_fired: Vec<TimerId>,
    }

    impl Strategy for Probe {
        fn on_event(&mut self, ctx: &Context<'_>, event: &Event, actions: &mut Actions) {
            self.seen_now.push(ctx.now());
            self.live_orders_seen.push(ctx.orders().len());
            match event {
                Event::Market(_) => {
                    if self.seen_now.len() == 1 {
                        actions.set_timer(Timestamp::from_millis(500), TimerId(9));
                    }
                    actions.place(OrderSpec {
                        instrument: BTC,
                        side: Side::Buy,
                        px: Px::from_e8(100),
                        qty: Qty::from_e8(1),
                        tif: TimeInForce::Alo,
                        reduce_only: false,
                    });
                }
                Event::Timer(timer) => self.timers_fired.push(timer.id),
                Event::Order(_) => {}
            }
        }
    }

    fn market(ts_local_ms: i64) -> Event {
        Event::Market(MarketEvent {
            instrument: BTC,
            ts_exch: Timestamp::from_millis(ts_local_ms - 1),
            ts_local: Timestamp::from_millis(ts_local_ms),
            kind: MarketEventKind::FeedGap,
        })
    }

    fn placed_cloids(actions: &Actions) -> Vec<hl_types::ClientOrderId> {
        actions
            .order_actions()
            .iter()
            .filter_map(|action| match action {
                OrderAction::Place { cloid, .. } => Some(*cloid),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn strategy_sees_event_time_and_its_actions_are_returned() {
        let mut host = StrategyHost::new(Probe::default(), 3);
        let actions = host.on_event(&market(100));

        assert_eq!(actions.order_actions().len(), 1);
        assert_eq!(
            actions.timer_requests(),
            [TimerRequest {
                at: Timestamp::from_millis(500),
                id: TimerId(9)
            }]
        );
        assert_eq!(host.strategy().seen_now, [Timestamp::from_millis(100)]);
        assert_eq!(host.now(), Timestamp::from_millis(100));
    }

    #[test]
    fn placed_orders_become_pending_after_the_callback() {
        let mut host = StrategyHost::new(Probe::default(), 3);
        let cloid = placed_cloids(host.on_event(&market(100)))[0];

        assert_eq!(host.strategy().live_orders_seen, [0]);
        assert_eq!(
            host.orders().get(cloid).unwrap().state,
            OrderState::PendingNew
        );
        assert_eq!(cloid.strategy_tag(), 3);
    }

    #[test]
    fn order_events_update_the_table_before_the_strategy_runs() {
        let mut host = StrategyHost::new(Probe::default(), 3);
        let cloid = placed_cloids(host.on_event(&market(100)))[0];

        let actions = host.on_event(&Event::Order(OrderEvent {
            instrument: BTC,
            cloid,
            ts_exch: Timestamp::from_millis(110),
            ts_local: Timestamp::from_millis(120),
            kind: OrderEventKind::Accepted { oid: OrderId(1) },
        }));
        assert!(actions.is_empty());
        assert_eq!(host.strategy().live_orders_seen, [0, 1]);
        assert_eq!(host.orders().get(cloid).unwrap().state, OrderState::Open);
    }

    #[test]
    fn actions_from_one_callback_do_not_leak_into_the_next() {
        let mut host = StrategyHost::new(Probe::default(), 3);
        host.on_event(&market(100));
        let actions = host.on_event(&Event::Timer(TimerEvent {
            id: TimerId(9),
            ts_local: Timestamp::from_millis(500),
        }));

        assert!(actions.is_empty());
        assert_eq!(host.strategy().timers_fired, [TimerId(9)]);
    }

    #[test]
    fn same_events_produce_the_same_order_ids() {
        let run = || {
            let mut host = StrategyHost::new(Probe::default(), 3);
            let mut cloids = placed_cloids(host.on_event(&market(100)));
            cloids.extend(placed_cloids(host.on_event(&market(100))));
            cloids.extend(placed_cloids(host.on_event(&market(250))));
            cloids
        };
        let first = run();
        assert_eq!(first, run());
        assert_eq!(first.len(), 3);
        assert!(
            first[0] < first[1] && first[1] < first[2],
            "ids are unique and ordered"
        );
    }

    #[test]
    fn local_time_never_goes_backwards() {
        let mut host = StrategyHost::new(Probe::default(), 3);
        host.on_event(&market(200));
        host.on_event(&market(150));
        assert_eq!(host.strategy().seen_now[1], Timestamp::from_millis(200));
    }
}
