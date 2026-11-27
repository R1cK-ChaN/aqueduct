//! The smallest end-to-end chain: hand-written market events drive a strategy
//! whose orders go to a gateway and whose reports come back.
//!
//! The same `FixedSpreadQuoter` type is run against two gateways: the
//! simulator, through `hl_run::replay`, and `InstantGateway`, a second
//! implementation written here that stands in for any other venue client.
//! The strategy is constructed by one function and is not told which gateway
//! it is talking to.

use std::collections::{BTreeSet, VecDeque};
use std::time::Duration;

use hl_gateway::OrderGateway;
use hl_run::recording::{EventOrigin, GapCause, RawPosition};
use hl_run::session::{Session, SessionStart};
use hl_run::{IncrementalDriver, PendingAtEnd, ReplayError, ReplayTrace, replay};
use hl_sim::{SimConfig, SimGateway};
use hl_strategy::{Actions, Context, FixedSpreadConfig, FixedSpreadQuoter, Strategy, StrategyHost};
use hl_types::{
    Bbo, CancelReason, ClientOrderId, Event, InstrumentId, Level, Liquidity, MarketEvent,
    MarketEventKind, OrderAction, OrderEvent, OrderEventKind, OrderId, OrderSpec, Px, Qty, Side,
    TimeInForce, TimerId, Timestamp, Trade, TradeId, Usd,
};

const MON: InstrumentId = InstrumentId::new(7);

/// Market data reaches the strategy this long after its exchange time.
const FEED_LATENCY_MS: i64 = 250;

fn px(text: &str) -> Px {
    text.parse().expect("valid price")
}

fn qty(text: &str) -> Qty {
    text.parse().expect("valid size")
}

fn usd(text: &str) -> Usd {
    text.parse().expect("valid amount")
}

fn ms(millis: i64) -> Timestamp {
    Timestamp::from_millis(millis)
}

fn market(ts_exch_ms: i64, kind: MarketEventKind) -> MarketEvent {
    MarketEvent {
        instrument: MON,
        ts_exch: ms(ts_exch_ms),
        ts_local: ms(ts_exch_ms + FEED_LATENCY_MS),
        kind,
    }
}

fn bbo(ts_exch_ms: i64, bid: &str, ask: &str) -> MarketEvent {
    let level = |price: &str| {
        Some(Level {
            px: px(price),
            qty: qty("40000"),
            orders: 4,
        })
    };
    market(
        ts_exch_ms,
        MarketEventKind::Bbo(Bbo {
            bid: level(bid),
            ask: level(ask),
        }),
    )
}

fn trade(ts_exch_ms: i64, price: &str, size: &str, aggressor: Side) -> MarketEvent {
    let trade = Trade {
        tid: TradeId(900),
        px: px(price),
        qty: qty(size),
        aggressor,
    };
    market(ts_exch_ms, MarketEventKind::Trade(trade))
}

/// A wide-spread market: quotes appear, a seller trades through the bid, the
/// market moves up, then the feed drops.
fn script() -> Vec<MarketEvent> {
    vec![
        bbo(0, "0.0250", "0.0260"),
        trade(1_000, "0.0251", "200", Side::Sell),
        bbo(2_000, "0.0252", "0.0262"),
        market(3_000, MarketEventKind::FeedGap),
    ]
}

#[derive(Debug)]
struct ObservedQuoter {
    inner: FixedSpreadQuoter,
    seen: Vec<Event>,
}

impl Strategy for ObservedQuoter {
    fn on_event(&mut self, ctx: &Context<'_>, event: &Event, actions: &mut Actions) {
        self.seen.push(event.clone());
        self.inner.on_event(ctx, event, actions);
    }
}

/// The one place the strategy under test is built.
fn quoter() -> StrategyHost<ObservedQuoter> {
    let config = FixedSpreadConfig {
        instrument: MON,
        half_spread: px("0.0002"),
        qty: qty("500"),
    };
    StrategyHost::new(
        ObservedQuoter {
            inner: FixedSpreadQuoter::new(config),
            seen: Vec::new(),
        },
        1,
    )
}

fn sim() -> SimGateway {
    SimGateway::new(SimConfig {
        order_latency: Duration::from_millis(100),
        report_latency: Duration::from_millis(50),
        maker_fee: "0.00015".parse().expect("valid rate"),
        min_order_notional: usd("10"),
    })
}

fn run_on_sim() -> (ReplayTrace, StrategyHost<ObservedQuoter>) {
    let mut host = quoter();
    let mut sim = sim();
    let trace = replay(&mut host, &mut sim, &script(), ms(3_250)).expect("script is ordered");
    (trace, host)
}

fn place(cloid: ClientOrderId, side: Side, price: &str) -> OrderAction {
    OrderAction::Place {
        cloid,
        order: OrderSpec {
            instrument: MON,
            side,
            px: px(price),
            qty: qty("500"),
            tif: TimeInForce::Alo,
            reduce_only: false,
        },
    }
}

fn modify(cloid: ClientOrderId, price: &str) -> OrderAction {
    OrderAction::Modify {
        instrument: MON,
        cloid,
        px: px(price),
        qty: qty("500"),
    }
}

/// The actions the quoter is expected to request for `script`, whichever
/// gateway is behind it.
fn expected_actions() -> Vec<(Timestamp, OrderAction)> {
    let bid = ClientOrderId::from_parts(1, MON, ms(250), 0);
    let ask = ClientOrderId::from_parts(1, MON, ms(250), 1);
    vec![
        (ms(250), place(bid, Side::Buy, "0.0253")),
        (ms(250), place(ask, Side::Sell, "0.0257")),
        (ms(2_250), modify(bid, "0.0255")),
        (ms(2_250), modify(ask, "0.0259")),
        (
            ms(3_250),
            OrderAction::CancelAll {
                instrument: Some(MON),
            },
        ),
    ]
}

#[test]
fn quoter_trades_against_the_simulator() {
    let (trace, host) = run_on_sim();
    assert_eq!(trace.actions, expected_actions());

    let bid = ClientOrderId::from_parts(1, MON, ms(250), 0);
    let ask = ClientOrderId::from_parts(1, MON, ms(250), 1);
    let reports: Vec<_> = trace
        .order_events
        .iter()
        .map(|event| (event.cloid, event.ts_exch, event.ts_local, event.kind))
        .collect();

    let accepted = |oid| OrderEventKind::Accepted { oid: OrderId(oid) };
    let filled = OrderEventKind::Filled(hl_types::Fill {
        tid: TradeId(1),
        oid: OrderId(1),
        side: Side::Buy,
        px: px("0.0253"),
        qty: qty("200"),
        liquidity: Liquidity::Maker,
        // 0.0253 * 200 = 5.06 USD at 1.5 bps.
        fee: usd("0.000759"),
    });
    assert_eq!(
        reports,
        [
            // A = 250 + 100 = 350. The trade proves 1,000 > 350 at R=1,250,
            // so each acceptance arrives at max(350 + 50, 1,250) = 1,250.
            (bid, ms(350), ms(1_250), accepted(1)),
            (ask, ms(350), ms(1_250), accepted(2)),
            // min(500, 200) = 200 at our 0.0253 bid. The fill arrives at
            // max(1,000 + 50, 1,250) = 1,250, after its acceptance.
            (bid, ms(1_000), ms(1_250), filled),
        ]
    );
    // The two modifies have A=2,250+100=2,350 and CancelAll has
    // A=3,250+100=3,350. The final gap supplies no proof for any of them.
    assert_eq!(host.orders().len(), 2);
    assert_eq!(host.orders().get(bid).unwrap().open_qty, qty("300"));
    assert_eq!(
        trace.pending_at_end,
        PendingAtEnd {
            actions: 3,
            reports: 0,
            timers: 0
        }
    );
    let seen = &host.strategy().seen;
    assert!(
        matches!(&seen[1], Event::Market(event) if matches!(event.kind, MarketEventKind::Trade(_)))
    );
    assert!(
        matches!(&seen[2], Event::Order(event) if event.cloid == bid && matches!(event.kind, OrderEventKind::Accepted { .. }))
    );
    assert!(
        matches!(&seen[4], Event::Order(event) if event.cloid == bid && matches!(event.kind, OrderEventKind::Filled(_)))
    );
}

#[test]
fn replay_is_deterministic() {
    assert_eq!(run_on_sim().0, run_on_sim().0);
}

#[test]
fn a_late_trade_fills_at_its_exchange_time() {
    let mut events = script();
    events[1].ts_local = ms(2_500);
    events.swap(1, 2);

    let trace =
        replay(&mut quoter(), &mut sim(), &events, ms(3_250)).expect("receive time is ordered");

    // The BBO at R=2,250 first proves the placements with A=350. Its
    // callback still sees both placements pending, so it cannot modify.
    // Only the two placements and the final CancelAll are requested.
    let expected = expected_actions();
    assert_eq!(trace.actions, [expected[0], expected[1], expected[4]]);
    let fills: Vec<_> = trace
        .order_events
        .iter()
        .filter_map(|event| match event.kind {
            OrderEventKind::Filled(fill) => Some((event.ts_exch, event.ts_local, fill.qty)),
            _ => None,
        })
        .collect();
    // A=350 < E=1,000, so the late print fills min(500,200)=200.
    // Delivery is max(1,000+50, 2,500)=2,500 while effect time stays 1,000.
    assert_eq!(fills, [(ms(1_000), ms(2_500), qty("200"))]);
}

#[test]
fn equal_exchange_times_keep_input_order() {
    let mut events = vec![
        bbo(0, "0.0250", "0.0260"),
        bbo(2_000, "0.0250", "0.0260"),
        trade(1_000, "0.0251", "200", Side::Sell),
        trade(1_000, "0.0251", "400", Side::Sell),
    ];
    events[2].ts_local = ms(2_300);
    events[3].ts_local = ms(2_350);

    let trace =
        replay(&mut quoter(), &mut sim(), &events, ms(2_350)).expect("receive time is ordered");
    let filled_sizes: Vec<_> = trace
        .order_events
        .iter()
        .filter_map(|event| match event.kind {
            OrderEventKind::Filled(fill) => Some(fill.qty),
            _ => None,
        })
        .collect();
    assert_eq!(filled_sizes, [qty("200"), qty("300")]);
}

#[derive(Debug, Default)]
struct RecordMarketEvents {
    events: Vec<MarketEvent>,
}

impl Strategy for RecordMarketEvents {
    fn on_event(&mut self, _ctx: &Context<'_>, event: &Event, _actions: &mut Actions) {
        if let Event::Market(event) = event {
            self.events.push(event.clone());
        }
    }
}

#[test]
fn the_strategy_keeps_receive_order_including_equal_local_times() {
    let mut events = vec![
        bbo(2_000, "0.0250", "0.0260"),
        trade(1_000, "0.0251", "200", Side::Sell),
        bbo(1_000, "0.0252", "0.0262"),
    ];
    for event in &mut events {
        event.ts_local = ms(2_250);
    }
    let mut host = StrategyHost::new(RecordMarketEvents::default(), 1);

    replay(&mut host, &mut sim(), &events, ms(2_250)).expect("receive time is ordered");

    assert_eq!(host.strategy().events, events);
}

/// A second gateway: accepts, modifies and cancels immediately and never
/// fills. It shares no code with the simulator.
#[derive(Debug, Default)]
struct InstantGateway {
    live: BTreeSet<(InstrumentId, ClientOrderId)>,
    reports: VecDeque<OrderEvent>,
    orders_accepted: u64,
}

impl InstantGateway {
    fn report(
        &mut self,
        now: Timestamp,
        instrument: InstrumentId,
        cloid: ClientOrderId,
        kind: OrderEventKind,
    ) {
        self.reports.push_back(OrderEvent {
            instrument,
            cloid,
            ts_exch: now,
            ts_local: now,
            kind,
        });
    }
}

impl OrderGateway for InstantGateway {
    fn submit(&mut self, now: Timestamp, actions: &[OrderAction]) {
        for &action in actions {
            match action {
                OrderAction::Place { cloid, order } => {
                    self.live.insert((order.instrument, cloid));
                    self.orders_accepted += 1;
                    let oid = OrderId(self.orders_accepted);
                    self.report(
                        now,
                        order.instrument,
                        cloid,
                        OrderEventKind::Accepted { oid },
                    );
                }
                OrderAction::Modify {
                    instrument,
                    cloid,
                    px,
                    qty,
                } => {
                    self.report(now, instrument, cloid, OrderEventKind::Modified { px, qty });
                }
                OrderAction::Cancel { instrument, cloid } => {
                    self.live.remove(&(instrument, cloid));
                    let kind = OrderEventKind::Canceled {
                        reason: CancelReason::Requested,
                    };
                    self.report(now, instrument, cloid, kind);
                }
                OrderAction::CancelAll { instrument } => {
                    let canceled: Vec<_> = self
                        .live
                        .iter()
                        .copied()
                        .filter(|(live, _)| instrument.is_none_or(|wanted| wanted == *live))
                        .collect();
                    for (live_instrument, cloid) in canceled {
                        self.live.remove(&(live_instrument, cloid));
                        let kind = OrderEventKind::Canceled {
                            reason: CancelReason::Requested,
                        };
                        self.report(now, live_instrument, cloid, kind);
                    }
                }
            }
        }
    }

    fn poll_events(&mut self, _now: Timestamp, out: &mut Vec<OrderEvent>) {
        out.extend(self.reports.drain(..));
    }
}

/// A driver that knows nothing but the two interfaces: it gives the strategy
/// each market event, submits what it asks for, and feeds reports back until
/// the gateway is quiet. The paper and live loops will have this shape.
fn drive<S: Strategy>(
    host: &mut StrategyHost<S>,
    gateway: &mut dyn OrderGateway,
    market: &[MarketEvent],
) -> Vec<(Timestamp, OrderAction)> {
    let mut requested = Vec::new();
    let mut reports = Vec::new();
    for market_event in market {
        let now = market_event.ts_local;
        let mut queue = VecDeque::from([Event::Market(market_event.clone())]);
        while let Some(event) = queue.pop_front() {
            let actions = host.on_event(&event);
            requested.extend(actions.order_actions().iter().map(|&action| (now, action)));
            gateway.submit(now, actions.order_actions());

            reports.clear();
            gateway.poll_events(now, &mut reports);
            queue.extend(reports.iter().copied().map(Event::Order));
        }
    }
    requested
}

#[test]
fn the_same_quoter_runs_against_a_different_gateway() {
    let mut host = quoter();
    let mut gateway = InstantGateway::default();
    let requested = drive(&mut host, &mut gateway, &script());

    assert_eq!(requested, expected_actions());
    assert_eq!(
        requested,
        run_on_sim().0.actions,
        "same requests as against the simulator"
    );
    assert_eq!(gateway.orders_accepted, 2);
    assert!(gateway.live.is_empty());
    assert!(host.orders().is_empty());
}

/// Sets a timer on the first market event and places an order when it fires.
#[derive(Debug)]
struct QuoteOnTimer {
    delay: Duration,
    armed: bool,
    fired_at: Vec<Timestamp>,
}

impl Strategy for QuoteOnTimer {
    fn on_event(&mut self, ctx: &Context<'_>, event: &Event, actions: &mut Actions) {
        match event {
            Event::Market(_) if !self.armed => {
                self.armed = true;
                actions.set_timer(ctx.now() + self.delay, TimerId(42));
                // Asking for a time in the past must not move time backwards.
                actions.set_timer(Timestamp::EPOCH, TimerId(1));
            }
            Event::Timer(timer) => {
                self.fired_at.push(timer.ts_local);
                if timer.id == TimerId(42) {
                    actions.place(OrderSpec {
                        instrument: MON,
                        side: Side::Buy,
                        px: px("0.0240"),
                        qty: qty("500"),
                        tif: TimeInForce::Alo,
                        reduce_only: false,
                    });
                }
            }
            _ => {}
        }
    }
}

#[test]
fn timers_fire_in_time_order_between_market_events() {
    let strategy = QuoteOnTimer {
        delay: Duration::from_millis(500),
        armed: false,
        fired_at: vec![],
    };
    let mut host = StrategyHost::new(strategy, 2);
    let mut sim = sim();
    let trace = replay(&mut host, &mut sim, &script(), ms(3_250)).expect("script is ordered");

    // First market event is seen at 250: the past-dated timer fires then, the
    // other one 500 ms later, before the trade is seen at 1250.
    assert_eq!(host.strategy().fired_at, [ms(250), ms(750)]);
    assert_eq!(trace.actions.len(), 1);
    assert_eq!(trace.actions[0].0, ms(750));
    assert_eq!(trace.order_events.len(), 1);
    assert_eq!(
        (
            trace.order_events[0].ts_exch,
            trace.order_events[0].ts_local
        ),
        // A=750+100=850. The trade at E=1,000 first proves it at R=1,250,
        // giving max(850+50, 1,250)=1,250 for the acceptance.
        (ms(850), ms(1_250))
    );
}

#[test]
fn replay_rejects_invalid_local_times() {
    let run = |events: &[MarketEvent]| replay(&mut quoter(), &mut sim(), events, ms(3_250));

    let mut early_local = bbo(1_000, "0.0250", "0.0260");
    early_local.ts_local = ms(999);
    assert_eq!(
        run(&[early_local]),
        Err(ReplayError::LocalBeforeExchange { index: 0 })
    );

    let mut late_then_early = [
        bbo(1_000, "0.0250", "0.0260"),
        bbo(1_100, "0.0250", "0.0260"),
    ];
    late_then_early[0].ts_local = ms(2_000);
    assert_eq!(
        run(&late_then_early),
        Err(ReplayError::LocalTimeDecreases { index: 1 })
    );
}

#[test]
fn incremental_input_and_explicit_closures_match_replay() {
    let events = script();
    let mut host = quoter();
    let mut venue = sim();
    let mut driver = IncrementalDriver::new(&mut host, &mut venue);
    for event in &events {
        driver.on_market_event(event).unwrap();
        driver.close_through(event.ts_local).unwrap();
    }
    let trace = driver.finish(ms(3_250)).unwrap();
    assert_eq!(trace, run_on_sim().0);
    assert_eq!(host.strategy().seen, run_on_sim().1.strategy().seen);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Callback {
    Market,
    Report,
    Timer(u64),
}

#[derive(Debug, Default)]
struct TiedCallbacks {
    seen: Vec<(Timestamp, Callback)>,
}

impl Strategy for TiedCallbacks {
    fn on_event(&mut self, ctx: &Context<'_>, event: &Event, actions: &mut Actions) {
        let kind = match event {
            Event::Market(_) => Callback::Market,
            Event::Order(_) => Callback::Report,
            Event::Timer(timer) => Callback::Timer(timer.id.0),
        };
        self.seen.push((ctx.now(), kind));
        if self.seen.len() == 1 {
            actions.place(OrderSpec {
                instrument: MON,
                side: Side::Buy,
                px: px("0.0240"),
                qty: qty("500"),
                tif: TimeInForce::Alo,
                reduce_only: false,
            });
            actions.set_timer(ms(20), TimerId(9));
            actions.set_timer(ms(20), TimerId(3));
        }
    }
}

fn zero_latency_sim() -> SimGateway {
    SimGateway::new(SimConfig {
        order_latency: Duration::ZERO,
        report_latency: Duration::ZERO,
        maker_fee: "0.00015".parse().expect("valid rate"),
        min_order_notional: usd("10"),
    })
}

#[test]
fn tied_inputs_precede_reports_and_timers_follow_registration_order() {
    let mut first = bbo(0, "0.0250", "0.0260");
    first.ts_local = ms(10);
    let mut proof = bbo(11, "0.0250", "0.0260");
    proof.ts_local = ms(20);
    let mut gap = market(12, MarketEventKind::FeedGap);
    gap.ts_local = ms(20);
    let mut host = StrategyHost::new(TiedCallbacks::default(), 1);
    let mut venue = zero_latency_sim();
    let mut driver = IncrementalDriver::new(&mut host, &mut venue);
    driver.on_market_event(&first).unwrap();
    driver.close_through(ms(10)).unwrap();
    driver.on_market_event(&proof).unwrap();
    driver.on_market_event(&gap).unwrap();
    let trace = driver.finish(ms(20)).unwrap();

    // The quote at E=0 cannot prove its response with A=10+0=10.
    // E=11>10 proves it, with delivery max(10+0,20)=20. Both inputs at
    // R=20 precede that report, followed by timers registered as 9 then 3.
    assert_eq!(
        host.strategy().seen,
        [
            (ms(10), Callback::Market),
            (ms(20), Callback::Market),
            (ms(20), Callback::Market),
            (ms(20), Callback::Report),
            (ms(20), Callback::Timer(9)),
            (ms(20), Callback::Timer(3)),
        ]
    );
    assert_eq!(trace.order_events.len(), 1);
    assert_eq!(trace.order_events[0].ts_exch, ms(10));
    assert_eq!(trace.order_events[0].ts_local, ms(20));
}

#[test]
fn an_unclosed_timestamp_defers_its_reports_and_timers() {
    let mut first = bbo(0, "0.0250", "0.0260");
    first.ts_local = ms(10);
    let mut proof = bbo(11, "0.0250", "0.0260");
    proof.ts_local = ms(20);
    let mut host = StrategyHost::new(TiedCallbacks::default(), 1);
    let mut venue = zero_latency_sim();
    {
        let mut driver = IncrementalDriver::new(&mut host, &mut venue);
        driver.on_market_event(&first).unwrap();
        driver.on_market_event(&proof).unwrap();
        // R=20 closes only times <20. Its tied report and two timers wait.
    }
    assert_eq!(
        host.strategy().seen,
        [(ms(10), Callback::Market), (ms(20), Callback::Market)]
    );
    assert_eq!(venue.undelivered_reports(), 1);
}

#[derive(Debug, Default)]
struct StopProbe {
    initialized: bool,
    timers: Vec<Timestamp>,
}

impl Strategy for StopProbe {
    fn on_event(&mut self, ctx: &Context<'_>, event: &Event, actions: &mut Actions) {
        match event {
            Event::Market(_) if !self.initialized => {
                self.initialized = true;
                actions.place(OrderSpec {
                    instrument: MON,
                    side: Side::Buy,
                    px: px("0.0240"),
                    qty: qty("500"),
                    tif: TimeInForce::Alo,
                    reduce_only: false,
                });
                actions.set_timer(ms(30), TimerId(1));
                actions.set_timer(ms(31), TimerId(2));
            }
            Event::Timer(timer) => {
                self.timers.push(ctx.now());
                if timer.id == TimerId(1) {
                    actions.cancel_all(Some(MON));
                }
            }
            _ => {}
        }
    }
}

#[test]
fn inclusive_stop_counts_unproved_actions_reports_and_timers() {
    let mut first = bbo(0, "0.0250", "0.0260");
    first.ts_local = ms(10);
    let mut proof = bbo(21, "0.0250", "0.0260");
    proof.ts_local = ms(25);
    let mut host = StrategyHost::new(StopProbe::default(), 1);
    let mut venue = SimGateway::new(SimConfig {
        order_latency: Duration::from_millis(10),
        report_latency: Duration::from_millis(20),
        maker_fee: "0.00015".parse().unwrap(),
        min_order_notional: usd("10"),
    });
    let events = [first, proof, bbo(100, "0.0250", "0.0260")];
    let trace = replay(&mut host, &mut venue, &events, ms(30)).unwrap();

    // Placement A=10+10=20 is proved by E=21 at R=25. Its report is due
    // at max(20+20,25)=40>30. The timer at 30 fires and submits CancelAll
    // with A=30+10=40, which has no evidence. The timer at 31 stays queued.
    // The input at 350 is outside the inclusive stop and supplies no proof.
    assert_eq!(
        trace.pending_at_end,
        PendingAtEnd {
            actions: 1,
            reports: 1,
            timers: 1
        }
    );
    assert!(trace.order_events.is_empty());
    assert_eq!(trace.actions.len(), 2);
    assert_eq!(
        trace.actions[1],
        (
            ms(30),
            OrderAction::CancelAll {
                instrument: Some(MON)
            }
        )
    );
    assert_eq!(host.strategy().timers, [ms(30)]);
}

#[test]
fn a_determined_report_can_arrive_during_silence_at_the_inclusive_stop() {
    let mut first = bbo(0, "0.0250", "0.0260");
    first.ts_local = ms(10);
    let mut proof = bbo(11, "0.0250", "0.0260");
    proof.ts_local = ms(20);
    let mut host = StrategyHost::new(TiedCallbacks::default(), 1);
    let mut venue = SimGateway::new(SimConfig {
        order_latency: Duration::ZERO,
        report_latency: Duration::from_millis(25),
        maker_fee: "0.00015".parse().unwrap(),
        min_order_notional: usd("10"),
    });
    let trace = replay(&mut host, &mut venue, &[first, proof], ms(35)).unwrap();
    // A=10, E=11 proves it at R=20, and max(10+25,20)=35 is included.
    assert_eq!(trace.order_events.len(), 1);
    assert_eq!(trace.order_events[0].ts_local, ms(35));
    assert_eq!(trace.pending_at_end, PendingAtEnd::default());
}

#[test]
fn a_recurring_timer_stops_at_the_cutoff() {
    #[derive(Debug, Default)]
    struct Recurring {
        fired: Vec<Timestamp>,
    }
    impl Strategy for Recurring {
        fn on_event(&mut self, ctx: &Context<'_>, event: &Event, actions: &mut Actions) {
            if matches!(event, Event::Timer(_)) {
                self.fired.push(ctx.now());
            }
            actions.set_timer(ctx.now() + Duration::from_millis(10), TimerId(1));
        }
    }
    let mut event = market(0, MarketEventKind::FeedGap);
    event.ts_local = ms(10);
    let mut host = StrategyHost::new(Recurring::default(), 1);
    let trace = replay(&mut host, &mut sim(), &[event], ms(30)).unwrap();
    // 10+10=20 and 20+10=30 fire. The next 30+10=40 is beyond stop.
    assert_eq!(host.strategy().fired, [ms(20), ms(30)]);
    assert_eq!(trace.pending_at_end.timers, 1);
}

#[test]
fn an_empty_replay_has_empty_counts_and_no_callbacks() {
    let mut host = quoter();
    let trace = replay(&mut host, &mut sim(), &[], ms(0)).unwrap();
    assert_eq!(trace, ReplayTrace::default());
    assert!(host.strategy().seen.is_empty());
}

#[test]
fn stopping_before_the_first_quote_keeps_the_ledger_flat() {
    let events = [market(0, MarketEventKind::FeedGap)];
    let mut host = quoter();
    let trace = replay(&mut host, &mut sim(), &events, ms(250)).unwrap();
    // A shutdown before the first quote requests one CancelAll at 250.
    // A=250+100=350 is unproved, with zero placements and zero fills.
    assert_eq!(
        trace.actions,
        [(
            ms(250),
            OrderAction::CancelAll {
                instrument: Some(MON)
            }
        )]
    );
    assert_eq!(
        trace.pending_at_end,
        PendingAtEnd {
            actions: 1,
            reports: 0,
            timers: 0
        }
    );
    assert!(host.orders().is_empty());
    let account = hl_run::pnl::summarize(MON, &events, &trace.order_events).unwrap();
    assert_eq!(account.position, Qty::ZERO);
    assert_eq!(account.fees, Usd::ZERO);
    assert_eq!(account.gross, Usd::ZERO);
    assert_eq!(account.net, Usd::ZERO);
    assert_eq!(account.mark_px, None);
}

#[test]
fn incremental_input_rejects_closed_or_decreasing_times_without_callbacks() {
    let mut first = bbo(0, "0.0250", "0.0260");
    first.ts_local = ms(10);
    let mut host = StrategyHost::new(RecordMarketEvents::default(), 1);
    let mut venue = sim();
    let mut driver = IncrementalDriver::new(&mut host, &mut venue);
    driver.on_market_event(&first).unwrap();
    driver.close_through(ms(10)).unwrap();
    assert_eq!(
        driver.on_market_event(&first),
        Err(ReplayError::TimeAlreadyClosed { index: 1 })
    );
    assert_eq!(
        driver.close_through(ms(9)),
        Err(ReplayError::CloseTimeDecreases)
    );
    let mut decreasing = first.clone();
    decreasing.ts_local = ms(9);
    assert_eq!(
        driver.on_market_event(&decreasing),
        Err(ReplayError::LocalTimeDecreases { index: 1 })
    );
    driver.finish(ms(10)).unwrap();
    assert_eq!(host.strategy().events, [first]);
}

#[test]
fn invalid_batch_is_rejected_before_any_strategy_callback() {
    let mut invalid = bbo(1_000, "0.0250", "0.0260");
    invalid.ts_local = ms(999);
    let mut host = quoter();
    let mut venue = sim();
    let events = [bbo(0, "0.0250", "0.0260"), invalid];
    assert_eq!(
        replay(&mut host, &mut venue, &events, ms(500)),
        Err(ReplayError::LocalBeforeExchange { index: 1 })
    );
    assert!(host.strategy().seen.is_empty());
    assert_eq!(venue.pending_actions(), 0);
}

fn session() -> Session {
    Session::new(&SessionStart {
        commit: "1111111111111111111111111111111111111111".to_owned(),
        dirty: false,
        driver: "backtest".to_owned(),
        network: hl_codec::hyperliquid::Network::Mainnet,
        source: None,
        coin: "MON".to_owned(),
        instrument: MON,
        process_start_wall_ns: None,
        strategy_name: "record_market_events".to_owned(),
        strategy_tag: 1,
        strategy: Vec::new(),
        sim_model: "strict_trade_through".to_owned(),
        sim: Vec::new(),
        rules: Vec::new(),
    })
}

#[test]
fn incremental_session_lines_can_be_drained_without_resetting_the_sequence() {
    let mut host = StrategyHost::new(RecordMarketEvents::default(), 1);
    let mut venue = sim();
    let mut session = session();
    let mut driver = IncrementalDriver::with_session(&mut host, &mut venue, &mut session);
    let mut event = market(0, MarketEventKind::FeedGap);
    event.ts_local = Timestamp::from_nanos(10);
    let mut origin = EventOrigin {
        position: RawPosition {
            file_index: 2,
            record_index: 4,
        },
        gap_cause: Some(GapCause::Dropped),
    };
    driver.on_market_event_with_origin(&event, &origin).unwrap();
    let first = driver.take_session_lines();
    assert_eq!(first.len(), 2);
    assert_eq!(
        first[1],
        "{\"kind\":\"gap\",\"seq\":\"0\",\"cause\":\"dropped\",\"instrument\":\"7\",\"ts_local_ns\":\"10\",\"raw\":{\"file_index\":\"2\",\"record_index\":\"4\"}}\n"
    );
    assert!(driver.take_session_lines().is_empty());

    event.ts_local = Timestamp::from_nanos(20);
    origin.position.record_index = 5;
    origin.gap_cause = Some(GapCause::Shutdown);
    driver.on_market_event_with_origin(&event, &origin).unwrap();
    assert_eq!(
        driver.take_session_lines(),
        [
            "{\"kind\":\"gap\",\"seq\":\"1\",\"cause\":\"shutdown\",\"instrument\":\"7\",\"ts_local_ns\":\"20\",\"raw\":{\"file_index\":\"2\",\"record_index\":\"5\"}}\n"
        ]
    );
    driver.finish(Timestamp::from_nanos(20)).unwrap();
    assert_eq!(host.strategy().events.len(), 2);
}

#[test]
fn session_rejects_missing_and_inconsistent_gap_provenance_before_callbacks() {
    let gap = market(0, MarketEventKind::FeedGap);
    let mut host = StrategyHost::new(RecordMarketEvents::default(), 1);
    let mut venue = sim();
    let mut session = session();
    let origin = EventOrigin {
        position: RawPosition::default(),
        gap_cause: None,
    };
    assert_eq!(
        hl_run::replay_with_session(
            &mut host,
            &mut venue,
            std::slice::from_ref(&gap),
            &[],
            ms(250),
            &mut session,
        ),
        Err(ReplayError::OriginCount {
            events: 1,
            origins: 0,
        })
    );
    assert_eq!(
        hl_run::replay_with_session(
            &mut host,
            &mut venue,
            std::slice::from_ref(&gap),
            &[origin],
            ms(250),
            &mut session,
        ),
        Err(ReplayError::GapOriginMismatch { index: 0 })
    );
    let mut driver = IncrementalDriver::with_session(&mut host, &mut venue, &mut session);
    assert_eq!(
        driver.on_market_event(&gap),
        Err(ReplayError::GapOriginMismatch { index: 0 })
    );
    let origin = EventOrigin {
        gap_cause: Some(GapCause::Dropped),
        ..origin
    };
    assert_eq!(
        driver.on_market_event_with_origin(&bbo(0, "9", "11"), &origin),
        Err(ReplayError::GapOriginMismatch { index: 0 })
    );
    driver.finish(ms(250)).unwrap();
    assert!(host.strategy().events.is_empty());
    assert_eq!(session.lines().len(), 1);
}

const MIDNIGHT: Timestamp = Timestamp::from_nanos(86_400_000_000_000);

fn at_midnight_offset(ns: i64) -> Timestamp {
    Timestamp::from_nanos(MIDNIGHT.as_nanos() + ns)
}

#[derive(Default)]
struct MidnightBid {
    quotes: usize,
}

impl Strategy for MidnightBid {
    fn on_event(&mut self, _ctx: &Context<'_>, event: &Event, actions: &mut Actions) {
        if let Event::Market(MarketEvent {
            kind: MarketEventKind::Bbo(_),
            ..
        }) = event
        {
            self.quotes += 1;
            match self.quotes {
                1 => {
                    actions.place(OrderSpec {
                        instrument: MON,
                        side: Side::Buy,
                        px: px("10"),
                        qty: qty("0.4"),
                        tif: TimeInForce::Alo,
                        reduce_only: false,
                    });
                    actions.set_timer(at_midnight_offset(-1), TimerId(1));
                }
                2 => actions.cancel_all(Some(MON)),
                _ => {}
            }
        }
    }
}

fn midnight_sim() -> SimGateway {
    SimGateway::new(SimConfig {
        order_latency: Duration::ZERO,
        report_latency: Duration::from_nanos(20),
        maker_fee: hl_types::FeeRate::ZERO,
        min_order_notional: Usd::ZERO,
    })
}

fn bid_canceled_before_midnight(driver: &mut IncrementalDriver<'_, MidnightBid>) {
    // Relative to midnight, place at -40, prove and cancel at -20, then
    // prove the cancel at -10. Delivery times are -20 and 0 respectively.
    for ns in [-40, -20, -10] {
        let mut quote = bbo(0, "9", "11");
        quote.ts_exch = at_midnight_offset(ns);
        quote.ts_local = quote.ts_exch;
        driver
            .on_market_event(&quote)
            .expect("receipt-ordered quote");
    }
}

fn late_sell(receipt_ns: i64) -> MarketEvent {
    // E=-30 is inside (-40,-20]. The sell crosses the old bid at 10
    // and claims min(0.6,0.4) from the superseded version.
    let mut sell = trade(0, "9", "0.6", Side::Sell);
    sell.ts_exch = at_midnight_offset(-30);
    sell.ts_local = at_midnight_offset(receipt_ns);
    sell
}

#[test]
fn midnight_checkpoint_excludes_report_due_at_cutoff_and_releases_prior_timer() {
    let mut host = StrategyHost::new(MidnightBid::default(), 1);
    let mut sim = midnight_sim();
    let mut session = session();
    let mut driver = IncrementalDriver::with_session(&mut host, &mut sim, &mut session);
    bid_canceled_before_midnight(&mut driver);
    let boundary = driver.checkpoint_before(MIDNIGHT).unwrap();
    // 0 place, 1 timer_set, 2 cancel_all, 3 accepted, 4 timer_fired at -1.
    assert_eq!(boundary.last_seq, Some(4));
    driver.finish(MIDNIGHT).unwrap();
    let report: serde_json::Value = serde_json::from_str(session.lines().last().unwrap()).unwrap();
    assert_eq!(report["seq"], "5");
    assert_eq!(report["variant"], "canceled");
    assert_eq!(report["ts_local_ns"], "86400000000000");
}

#[test]
fn midnight_checkpoint_excludes_late_print_received_at_cutoff() {
    let mut host = StrategyHost::new(MidnightBid::default(), 1);
    let mut sim = midnight_sim();
    let mut session = session();
    let mut driver = IncrementalDriver::with_session(&mut host, &mut sim, &mut session);
    bid_canceled_before_midnight(&mut driver);
    let boundary = driver.checkpoint_before(MIDNIGHT).unwrap();
    driver.on_market_event(&late_sell(0)).unwrap();
    let tomorrow = Timestamp::from_nanos(172_800_000_000_000);
    let next = driver.checkpoint_before(tomorrow).unwrap();
    assert_eq!(next.last_seq, Some(5));
    assert_eq!(next.late_print_misses.trades, 1);
    assert_eq!(next.late_print_misses.qty, qty("0.4"));
    let trace = driver.finish(tomorrow).unwrap();
    assert_eq!(boundary.late_print_misses.trades, 0);
    assert_eq!(boundary.late_print_misses.qty, Qty::ZERO);
    assert_eq!(trace.late_print_misses.trades, 1);
    assert_eq!(trace.late_print_misses.qty, qty("0.4"));
}

#[test]
fn midnight_checkpoint_counts_late_print_received_one_nanosecond_before_cutoff() {
    let mut host = StrategyHost::new(MidnightBid::default(), 1);
    let mut sim = midnight_sim();
    let mut session = session();
    let mut driver = IncrementalDriver::with_session(&mut host, &mut sim, &mut session);
    bid_canceled_before_midnight(&mut driver);
    driver.on_market_event(&late_sell(-1)).unwrap();
    let boundary = driver.checkpoint_before(MIDNIGHT).unwrap();
    assert_eq!(boundary.late_print_misses.trades, 1);
    assert_eq!(boundary.late_print_misses.qty, qty("0.4"));
    assert_eq!(boundary.last_seq, Some(4));
    let trace = driver.finish(MIDNIGHT).unwrap();
    assert_eq!(trace.late_print_misses.trades, 1);
    assert_eq!(trace.late_print_misses.qty, qty("0.4"));
}

#[test]
fn checkpoint_rejects_a_boundary_already_reached_by_input_or_closure() {
    for input_at_cutoff in [false, true] {
        let mut host = StrategyHost::new(MidnightBid::default(), 1);
        let mut sim = midnight_sim();
        let mut session = session();
        let mut driver = IncrementalDriver::with_session(&mut host, &mut sim, &mut session);
        assert_eq!(driver.checkpoint_before(MIDNIGHT).unwrap().last_seq, None);
        if input_at_cutoff {
            driver.on_market_event(&late_sell(0)).unwrap();
        } else {
            driver.close_through(MIDNIGHT).unwrap();
        }
        assert_eq!(
            driver.checkpoint_before(MIDNIGHT),
            Err(ReplayError::CloseTimeDecreases)
        );
    }
}

#[test]
fn paper_and_full_traces_agree_on_work_fills_end_counts_misses_and_account() {
    use hl_run::pnl::{BboMark, summarize, summarize_marks};

    let mut events = vec![
        bbo(0, "0.0250", "0.0260"),
        trade(1_000, "0.0251", "200", Side::Sell),
        bbo(2_000, "0.0252", "0.0262"),
        bbo(2_400, "0.0252", "0.0262"),
    ];
    // The partial fill leaves 300 of the old bid. R=2750 follows proof
    // of the modify, but E=2000 was inside the superseded interval.
    let mut late = trade(2_000, "0.0251", "400", Side::Sell);
    late.ts_local = ms(2_750);
    events.push(late);
    for i in 0..128 {
        let (bid, ask) = if i % 4 < 2 {
            ("0.0252", "0.0262")
        } else {
            ("0.0250", "0.0260")
        };
        events.push(bbo(4_000 + i * 500, bid, ask));
    }
    events.push(market(68_000, MarketEventKind::FeedGap));

    let run = |paper| {
        let mut host = quoter();
        let mut sim = sim();
        let mut session = session();
        let mut driver = if paper {
            IncrementalDriver::for_paper(&mut host, &mut sim, &mut session)
        } else {
            IncrementalDriver::with_session(&mut host, &mut sim, &mut session)
        };
        for (index, event) in events.iter().enumerate() {
            let origin = EventOrigin {
                position: RawPosition {
                    file_index: 0,
                    record_index: index as u64 + 1,
                },
                gap_cause: matches!(event.kind, MarketEventKind::FeedGap)
                    .then_some(GapCause::Shutdown),
            };
            driver.on_market_event_with_origin(event, &origin).unwrap();
        }
        let trace = driver.finish(ms(68_250)).unwrap();
        assert_eq!(host.orders().len(), 2);
        assert_eq!(
            trace.pending_at_end,
            PendingAtEnd {
                actions: 1,
                reports: 0,
                timers: 0
            }
        );
        assert_eq!(trace.late_print_misses.trades, 1);
        assert_eq!(trace.late_print_misses.qty, qty("300"));
        (trace, session.take_lines())
    };
    let (full, full_rows) = run(false);
    let (paper, paper_rows) = run(true);
    assert_eq!(paper_rows, full_rows);
    // Two places, 64 pairs of modifies, and the unproved shutdown cancel.
    assert_eq!(full.actions.len(), 131);
    // Two accepts and the partial fill, followed by 128 modify reports.
    assert_eq!(full.order_events.len(), 131);
    assert_eq!(paper.actions.capacity(), 0);
    assert_eq!(paper.order_events.len(), 1);
    assert!(matches!(
        paper.order_events[0].kind,
        OrderEventKind::Filled(_)
    ));
    let marks: Vec<_> = events
        .iter()
        .filter_map(|e| BboMark::from_event(MON, e))
        .collect();
    let paper_pnl = summarize_marks(MON, &marks, &paper.order_events).unwrap();
    assert_eq!(
        paper_pnl,
        summarize(MON, &events, &full.order_events).unwrap()
    );
    // (0.0255 - 0.0253) * 200 - 0.000759 fee = 0.039241.
    assert_eq!(paper_pnl.net, usd("0.039241"));
    assert_eq!(paper_pnl.position, qty("200"));
}

fn rule_test_bid() -> OrderSpec {
    OrderSpec {
        instrument: MON,
        side: Side::Buy,
        px: px("100"),
        qty: qty("1"),
        tif: TimeInForce::Alo,
        reduce_only: false,
    }
}

/// A strategy submits on the first BBO. A trade at 205 ns proves the batch.
fn replay_rule_orders(
    submitted_ns: i64,
    submit: impl FnOnce(&mut Actions),
) -> (Result<ReplayTrace, ReplayError>, Vec<serde_json::Value>) {
    use hl_rules::{RuleBook, RuleHistory, RuleVersion};

    struct SubmitOnce<F>(Option<F>);
    impl<F: FnOnce(&mut Actions)> Strategy for SubmitOnce<F> {
        fn on_event(&mut self, _: &Context<'_>, _: &Event, actions: &mut Actions) {
            if let Some(submit) = self.0.take() {
                submit(actions);
            }
        }
    }

    let ns = Timestamp::from_nanos;
    let history = RuleHistory::new(vec![
        RuleVersion::new(ns(100), 2, 17).expect("first version"),
        RuleVersion::new(ns(200), 3, 21).expect("second version"),
    ])
    .expect("increasing rule starts");
    let mut rules = RuleBook::new();
    rules.insert(MON, history).expect("first history for MON");
    let config = SimConfig {
        order_latency: Duration::from_nanos(1),
        report_latency: Duration::ZERO,
        maker_fee: hl_types::FeeRate::ZERO,
        min_order_notional: usd("10"),
    };
    let mut sim = SimGateway::with_rules(config, rules);
    let mut host = StrategyHost::new(SubmitOnce(Some(submit)), 1);
    let mut session = session();
    let events = [
        MarketEvent {
            ts_exch: ns(submitted_ns),
            ts_local: ns(submitted_ns),
            ..bbo(0, "99", "101")
        },
        MarketEvent {
            ts_exch: ns(205),
            ts_local: ns(205),
            ..trade(0, "100", "1", Side::Buy)
        },
    ];
    let origins = [1, 2].map(|record_index| EventOrigin {
        position: RawPosition {
            file_index: 0,
            record_index,
        },
        gap_cause: None,
    });
    let result = hl_run::replay_with_session(
        &mut host,
        &mut sim,
        &events,
        &origins,
        ns(205),
        &mut session,
    );
    let rows = session
        .lines()
        .iter()
        .map(|line| serde_json::from_str(line).expect("valid session JSON"))
        .collect();
    (result, rows)
}

#[test]
fn session_reports_rule_rejections_at_the_order_effect_time() {
    // The last two bids also cross the ask at 101.
    for (price, size) in [
        ("100.001", "1"),
        ("100", "1.001"),
        ("101.001", "1"),
        ("101", "1.001"),
    ] {
        let (result, rows) = replay_rule_orders(100, |actions| {
            let bid = rule_test_bid();
            let resting = actions.place(bid);
            let px = px(price);
            let qty = qty(size);
            actions.place(OrderSpec { px, qty, ..bid });
            actions.modify(MON, resting, px, qty);
        });
        result.expect("covered batch");
        let reports: Vec<_> = rows.iter().filter(|row| row["kind"] == "report").collect();
        assert_eq!(reports.len(), 3);
        assert_eq!(reports[0]["variant"], "accepted");
        for (row, variant) in reports[1..].iter().zip(["rejected", "modify_rejected"]) {
            assert_eq!(row["variant"], variant);
            assert_eq!(row["reason"], "invalid_order");
            assert_eq!(row["ts_exch_ns"], "101");
            assert_eq!(row["ts_local_ns"], "205");
        }
    }
}

#[test]
fn missing_rule_coverage_stops_replay_after_the_action_row() {
    let check = |submit: fn(&mut Actions)| {
        let (result, rows) = replay_rule_orders(98, submit);
        let error = result.expect_err("effect time precedes the first rule");
        let ns = Timestamp::from_nanos;
        assert_eq!(
            error,
            ReplayError::Sim(hl_sim::SimError {
                instrument: MON,
                effective_at: ns(99),
                source: hl_rules::RuleError::MissingCoverage {
                    ts_exch: ns(99),
                    first_start: ns(100),
                },
            })
        );
        let message = error.to_string();
        for detail in ["instrument 7", "99 ns", "100 ns"] {
            assert!(message.contains(detail), "{message}");
        }
        let kinds: Vec<_> = rows
            .iter()
            .map(|row| row["kind"].as_str().expect("session row kind"))
            .collect();
        assert_eq!(kinds, ["start", "action"]);
    };
    check(|actions| {
        actions.place(rule_test_bid());
    });
    check(|actions| {
        actions.place(OrderSpec {
            tif: TimeInForce::Ioc,
            ..rule_test_bid()
        });
    });
    check(|actions| {
        actions.modify(MON, ClientOrderId::from_u128(42), px("100"), qty("1"));
    });
}
