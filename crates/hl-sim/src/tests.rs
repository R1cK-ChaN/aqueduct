use hl_types::{Bbo, Level};

use super::*;

const MON: InstrumentId = InstrumentId::new(7);
const JUP: InstrumentId = InstrumentId::new(8);

fn px(text: &str) -> Px {
    text.parse().unwrap()
}

fn qty(text: &str) -> Qty {
    text.parse().unwrap()
}

fn usd(text: &str) -> Usd {
    text.parse().unwrap()
}

fn ms(millis: i64) -> Timestamp {
    Timestamp::from_millis(millis)
}

fn sim(order_latency_ms: u64, report_latency_ms: u64) -> SimGateway {
    SimGateway::new(SimConfig {
        order_latency: Duration::from_millis(order_latency_ms),
        report_latency: Duration::from_millis(report_latency_ms),
        maker_fee: "0.00015".parse().unwrap(),
        min_order_notional: usd("10"),
    })
}

fn cloid(seq: u64) -> ClientOrderId {
    ClientOrderId::from_parts(1, MON, Timestamp::EPOCH, seq)
}

fn spec(instrument: InstrumentId, side: Side, price: &str, size: &str) -> OrderSpec {
    OrderSpec {
        instrument,
        side,
        px: px(price),
        qty: qty(size),
        tif: TimeInForce::Alo,
        reduce_only: false,
    }
}

fn place(seq: u64, side: Side, price: &str, size: &str) -> OrderAction {
    OrderAction::Place {
        cloid: cloid(seq),
        order: spec(MON, side, price, size),
    }
}

fn cancel(seq: u64) -> OrderAction {
    OrderAction::Cancel {
        instrument: MON,
        cloid: cloid(seq),
    }
}

fn bbo(at_ms: i64, bid: &str, ask: &str) -> MarketEvent {
    let level = |price: &str| {
        Some(Level {
            px: px(price),
            qty: qty("1000"),
            orders: 3,
        })
    };
    MarketEvent {
        instrument: MON,
        ts_exch: ms(at_ms),
        ts_local: ms(at_ms),
        kind: MarketEventKind::Bbo(Bbo {
            bid: level(bid),
            ask: level(ask),
        }),
    }
}

fn trade(at_ms: i64, price: &str, size: &str, aggressor: Side) -> MarketEvent {
    MarketEvent {
        instrument: MON,
        ts_exch: ms(at_ms),
        ts_local: ms(at_ms),
        kind: MarketEventKind::Trade(Trade {
            tid: TradeId(900),
            px: px(price),
            qty: qty(size),
            aggressor,
        }),
    }
}

fn poll(sim: &mut SimGateway, at_ms: i64) -> Vec<OrderEvent> {
    let mut out = Vec::new();
    sim.poll_events(ms(at_ms), &mut out);
    out
}

/// Polls and returns `(sequence number of the cloid, kind)` for each report.
fn kinds(sim: &mut SimGateway, at_ms: i64) -> Vec<(u64, OrderEventKind)> {
    poll(sim, at_ms)
        .into_iter()
        .map(|event| (event.cloid.seq(), event.kind))
        .collect()
}

/// Proves a zero-latency action with E = A + 1 ns and polls at R = E.
/// Validation and matching fixtures keep their original outcomes. The later
/// quote supplies the evidence that their old poll-only setup lacked.
fn prove_and_poll(sim: &mut SimGateway, effect_ms: i64) -> Vec<OrderEvent> {
    let mut proof = bbo(effect_ms, "100", "101");
    proof.ts_exch = ms(effect_ms) + Duration::from_nanos(1);
    proof.ts_local = proof.ts_exch;
    sim.on_market_event(&proof);
    let mut out = Vec::new();
    sim.poll_events(proof.ts_local, &mut out);
    out
}

fn proved_kinds(sim: &mut SimGateway, effect_ms: i64) -> Vec<(u64, OrderEventKind)> {
    prove_and_poll(sim, effect_ms)
        .into_iter()
        .map(|event| (event.cloid.seq(), event.kind))
        .collect()
}

fn received(mut event: MarketEvent, received_ms: i64) -> MarketEvent {
    event.ts_local = ms(received_ms);
    event
}

fn accepted(oid: u64) -> OrderEventKind {
    OrderEventKind::Accepted { oid: OrderId(oid) }
}

fn rejected(reason: RejectReason) -> OrderEventKind {
    OrderEventKind::Rejected { reason }
}

fn canceled() -> OrderEventKind {
    OrderEventKind::Canceled {
        reason: CancelReason::Requested,
    }
}

fn filled(kind: OrderEventKind) -> Fill {
    match kind {
        OrderEventKind::Filled(fill) => fill,
        other => panic!("expected a fill, got {other:?}"),
    }
}

#[test]
fn post_only_order_is_accepted_and_reported() {
    let mut sim = sim(0, 0);
    sim.submit(ms(5), &[place(0, Side::Buy, "100", "1")]);

    // A = 5 + 0 = 5 ms. Proof at E = R = 5 ms + 1 ns gives
    // delivery max(5 ms + 0, 5 ms + 1 ns) = 5 ms + 1 ns.
    let events = prove_and_poll(&mut sim, 5);
    assert_eq!(
        events,
        [OrderEvent {
            instrument: MON,
            cloid: cloid(0),
            ts_exch: ms(5),
            ts_local: ms(5) + Duration::from_nanos(1),
            kind: accepted(1),
        }]
    );
    assert!(poll(&mut sim, 6).is_empty(), "each report is returned once");
}

#[test]
fn post_only_order_that_would_cross_is_rejected() {
    let mut sim = sim(0, 0);
    sim.on_market_event(&bbo(0, "100", "101"));
    sim.submit(
        ms(0),
        &[
            place(0, Side::Buy, "101", "1"),
            place(1, Side::Sell, "100", "1"),
            place(2, Side::Buy, "100.5", "1"),
            place(3, Side::Sell, "100.6", "1"),
        ],
    );

    assert_eq!(
        proved_kinds(&mut sim, 0),
        [
            (0, rejected(RejectReason::WouldCross)),
            (1, rejected(RejectReason::WouldCross)),
            (2, accepted(1)),
            (3, accepted(2)),
        ]
    );
}

#[test]
fn order_rests_when_the_other_side_is_unknown() {
    let mut sim = sim(0, 0);
    sim.submit(ms(0), &[place(0, Side::Buy, "5000", "1")]);
    assert_eq!(proved_kinds(&mut sim, 0), [(0, accepted(1))]);
}

#[test]
fn order_below_the_minimum_notional_is_rejected() {
    let mut sim = sim(0, 0);
    sim.submit(
        ms(0),
        &[
            place(0, Side::Buy, "100", "0.0999"),
            place(1, Side::Buy, "100", "0.1"),
        ],
    );
    assert_eq!(
        proved_kinds(&mut sim, 0),
        [
            (0, rejected(RejectReason::BelowMinNotional)),
            (1, accepted(1))
        ]
    );
}

#[test]
fn non_positive_price_or_size_is_rejected() {
    let mut sim = sim(0, 0);
    sim.submit(
        ms(0),
        &[
            place(0, Side::Buy, "0", "1"),
            place(1, Side::Buy, "100", "0"),
            place(2, Side::Sell, "100", "-1"),
        ],
    );
    let invalid = rejected(RejectReason::InvalidOrder);
    assert_eq!(
        proved_kinds(&mut sim, 0),
        [(0, invalid), (1, invalid), (2, invalid)]
    );
}

#[test]
fn unsupported_order_types_are_rejected() {
    let mut sim = sim(0, 0);
    let base = spec(MON, Side::Buy, "100", "1");
    sim.submit(
        ms(0),
        &[
            OrderAction::Place {
                cloid: cloid(0),
                order: OrderSpec {
                    tif: TimeInForce::Gtc,
                    ..base
                },
            },
            OrderAction::Place {
                cloid: cloid(1),
                order: OrderSpec {
                    tif: TimeInForce::Ioc,
                    ..base
                },
            },
            OrderAction::Place {
                cloid: cloid(2),
                order: OrderSpec {
                    reduce_only: true,
                    ..base
                },
            },
        ],
    );
    let unsupported = rejected(RejectReason::Unsupported);
    assert_eq!(
        proved_kinds(&mut sim, 0),
        [(0, unsupported), (1, unsupported), (2, unsupported)]
    );
}

#[test]
fn reused_cloid_is_rejected_without_touching_the_first_order() {
    let mut sim = sim(0, 0);
    sim.submit(
        ms(0),
        &[
            place(0, Side::Buy, "100", "1"),
            place(0, Side::Buy, "99", "1"),
        ],
    );
    assert_eq!(
        proved_kinds(&mut sim, 0),
        [
            (0, accepted(1)),
            (0, rejected(RejectReason::DuplicateClientOrderId))
        ]
    );

    sim.submit(ms(1), &[cancel(0)]);
    assert_eq!(
        proved_kinds(&mut sim, 1),
        [(0, canceled())],
        "the first order is still live"
    );
}

#[test]
fn trade_through_the_price_fills_at_the_order_price_with_the_maker_fee() {
    let mut sim = sim(0, 0);
    sim.submit(ms(0), &[place(0, Side::Buy, "100", "2")]);
    prove_and_poll(&mut sim, 0);

    sim.on_market_event(&trade(10, "99.9", "5", Side::Sell));
    let events = poll(&mut sim, 10);
    // Fill min(2, 5) = 2 at 100. Notional 100 * 2 = 200,
    // maker fee 200 * 0.00015 = 0.03.
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].ts_exch, ms(10));
    assert_eq!(
        filled(events[0].kind),
        Fill {
            tid: TradeId(1),
            oid: OrderId(1),
            side: Side::Buy,
            px: px("100"),
            qty: qty("2"),
            liquidity: Liquidity::Maker,
            fee: usd("0.03"),
        }
    );
}

#[test]
fn ask_fills_when_a_buyer_trades_above_it() {
    let mut sim = sim(0, 0);
    sim.submit(ms(0), &[place(0, Side::Sell, "101", "1")]);
    prove_and_poll(&mut sim, 0);

    sim.on_market_event(&trade(10, "101.5", "1", Side::Buy));
    let fill = filled(poll(&mut sim, 10)[0].kind);
    assert_eq!(
        (fill.side, fill.px, fill.qty),
        (Side::Sell, px("101"), qty("1"))
    );
}

#[test]
fn trade_at_the_order_price_does_not_fill() {
    let mut sim = sim(0, 0);
    sim.submit(
        ms(0),
        &[
            place(0, Side::Buy, "100", "2"),
            place(1, Side::Sell, "101", "2"),
        ],
    );
    prove_and_poll(&mut sim, 0);

    sim.on_market_event(&trade(10, "100", "5", Side::Sell));
    sim.on_market_event(&trade(11, "101", "5", Side::Buy));
    assert!(poll(&mut sim, 11).is_empty());
}

#[test]
fn trade_on_another_instrument_does_not_fill() {
    let mut sim = sim(0, 0);
    sim.submit(ms(0), &[place(0, Side::Buy, "100", "2")]);
    prove_and_poll(&mut sim, 0);

    let mut other = trade(10, "99", "5", Side::Sell);
    other.instrument = JUP;
    sim.on_market_event(&other);
    assert!(poll(&mut sim, 10).is_empty());
}

#[test]
fn small_trades_fill_an_order_in_parts() {
    let mut sim = sim(0, 0);
    sim.submit(ms(0), &[place(0, Side::Buy, "100", "2")]);
    prove_and_poll(&mut sim, 0);

    sim.on_market_event(&trade(10, "99", "0.5", Side::Sell));
    assert_eq!(filled(poll(&mut sim, 10)[0].kind).qty, qty("0.5"));

    sim.on_market_event(&trade(20, "99", "9", Side::Sell));
    assert_eq!(filled(poll(&mut sim, 20)[0].kind).qty, qty("1.5"));

    sim.on_market_event(&trade(30, "99", "9", Side::Sell));
    assert!(
        poll(&mut sim, 30).is_empty(),
        "the order is gone once fully filled"
    );
}

#[test]
fn one_trade_is_shared_best_price_first() {
    let mut sim = sim(0, 0);
    sim.submit(
        ms(0),
        &[
            place(0, Side::Buy, "100", "2"),
            place(1, Side::Buy, "100.5", "2"),
            place(2, Side::Buy, "100.5", "2"),
        ],
    );
    prove_and_poll(&mut sim, 0);

    sim.on_market_event(&trade(10, "99", "5", Side::Sell));
    let fills: Vec<_> = kinds(&mut sim, 10)
        .into_iter()
        .map(|(seq, kind)| (seq, filled(kind).px, filled(kind).qty))
        .collect();
    // One print has 5 units. The two older/higher bids each take 2,
    // leaving 5 - 2 - 2 = 1 for the lower bid at 100.
    assert_eq!(
        fills,
        [
            (1, px("100.5"), qty("2")),
            (2, px("100.5"), qty("2")),
            (0, px("100"), qty("1")),
        ]
    );
}

#[test]
fn canceled_order_no_longer_fills_and_cannot_be_canceled_twice() {
    let mut sim = sim(0, 0);
    sim.submit(ms(0), &[place(0, Side::Buy, "100", "2")]);
    prove_and_poll(&mut sim, 0);

    sim.submit(ms(1), &[cancel(0)]);
    assert_eq!(proved_kinds(&mut sim, 1), [(0, canceled())]);

    sim.on_market_event(&trade(10, "99", "5", Side::Sell));
    sim.submit(ms(10), &[cancel(0)]);
    assert_eq!(
        proved_kinds(&mut sim, 10),
        [(
            0,
            OrderEventKind::CancelRejected {
                reason: RejectReason::UnknownOrder
            }
        )]
    );
}

#[test]
fn cancel_all_can_be_limited_to_one_instrument() {
    let mut sim = sim(0, 0);
    sim.submit(
        ms(0),
        &[
            place(0, Side::Buy, "100", "2"),
            OrderAction::Place {
                cloid: cloid(1),
                order: spec(JUP, Side::Buy, "50", "2"),
            },
            place(2, Side::Sell, "101", "2"),
        ],
    );
    prove_and_poll(&mut sim, 0);

    sim.submit(
        ms(1),
        &[OrderAction::CancelAll {
            instrument: Some(MON),
        }],
    );
    assert_eq!(
        proved_kinds(&mut sim, 1),
        [(0, canceled()), (2, canceled())]
    );

    sim.submit(ms(2), &[OrderAction::CancelAll { instrument: None }]);
    let events = prove_and_poll(&mut sim, 2);
    assert_eq!(events.len(), 1);
    assert_eq!(
        (events[0].instrument, events[0].cloid, events[0].kind),
        (JUP, cloid(1), canceled())
    );

    sim.submit(ms(3), &[OrderAction::CancelAll { instrument: None }]);
    assert!(
        prove_and_poll(&mut sim, 3).is_empty(),
        "nothing left to cancel"
    );
}

#[test]
fn modify_changes_price_and_open_quantity() {
    let mut sim = sim(0, 0);
    sim.submit(ms(0), &[place(0, Side::Buy, "100", "2")]);
    prove_and_poll(&mut sim, 0);

    let modify = OrderAction::Modify {
        instrument: MON,
        cloid: cloid(0),
        px: px("99"),
        qty: qty("3"),
    };
    sim.submit(ms(1), &[modify]);
    assert_eq!(
        proved_kinds(&mut sim, 1),
        [(
            0,
            OrderEventKind::Modified {
                px: px("99"),
                qty: qty("3")
            }
        )]
    );

    sim.on_market_event(&trade(10, "99.5", "9", Side::Sell));
    assert!(
        poll(&mut sim, 10).is_empty(),
        "the old price no longer rests"
    );

    sim.on_market_event(&trade(20, "98", "9", Side::Sell));
    let fill = filled(poll(&mut sim, 20)[0].kind);
    assert_eq!((fill.px, fill.qty), (px("99"), qty("3")));
}

#[test]
fn rejected_modify_leaves_the_order_unchanged() {
    let mut sim = sim(0, 0);
    sim.on_market_event(&bbo(0, "100", "101"));
    sim.submit(ms(0), &[place(0, Side::Buy, "100", "2")]);
    prove_and_poll(&mut sim, 0);

    let modify = |seq: u64, price: &str, size: &str| OrderAction::Modify {
        instrument: MON,
        cloid: cloid(seq),
        px: px(price),
        qty: qty(size),
    };
    sim.submit(
        ms(1),
        &[
            modify(0, "101", "2"),
            modify(0, "100", "0.01"),
            modify(5, "100", "2"),
        ],
    );
    let reject = |reason| OrderEventKind::ModifyRejected { reason };
    assert_eq!(
        proved_kinds(&mut sim, 1),
        [
            (0, reject(RejectReason::WouldCross)),
            (0, reject(RejectReason::BelowMinNotional)),
            (5, reject(RejectReason::UnknownOrder)),
        ]
    );

    sim.on_market_event(&trade(10, "99", "9", Side::Sell));
    let fill = filled(poll(&mut sim, 10)[0].kind);
    assert_eq!((fill.px, fill.qty), (px("100"), qty("2")));
}

#[test]
fn actions_and_reports_are_delayed_by_the_configured_latencies() {
    let mut sim = sim(100, 50);
    assert_eq!(sim.next_wakeup(), None);

    sim.submit(ms(0), &[place(0, Side::Buy, "100", "2")]);
    // A = 0 + 100 = 100. Pending actions have no known report deadline.
    assert_eq!(sim.next_wakeup(), None);
    assert!(poll(&mut sim, 99).is_empty());
    assert!(poll(&mut sim, 120).is_empty());
    assert_eq!(sim.pending_actions(), 1);

    sim.on_market_event(&bbo(121, "100", "101"));
    // E = 121 > 100 proves acceptance. max(100 + 50, 121) = 150.
    assert_eq!(sim.pending_actions(), 0);
    assert_eq!(sim.undelivered_reports(), 1);
    assert_eq!(sim.next_wakeup(), Some(ms(150)));
    assert!(poll(&mut sim, 149).is_empty());

    let events = poll(&mut sim, 150);
    assert_eq!(events.len(), 1);
    assert_eq!((events[0].ts_exch, events[0].ts_local), (ms(100), ms(150)));
    assert_eq!(events[0].kind, accepted(1));
    assert_eq!(sim.undelivered_reports(), 0);
    assert_eq!(sim.next_wakeup(), None);
}

#[test]
fn order_is_checked_against_the_book_at_arrival_time() {
    let mut sim = sim(100, 0);
    sim.on_market_event(&bbo(0, "100", "101"));
    sim.submit(ms(0), &[place(0, Side::Buy, "100.5", "2")]);

    sim.on_market_event(&bbo(50, "99", "100"));
    // A = 0 + 100 = 100. E = 101 proves the action before its own
    // 100 / 101 quote is applied, so bid 100.5 crosses the saved ask 100.
    sim.on_market_event(&bbo(101, "100", "101"));
    assert_eq!(
        kinds(&mut sim, 101),
        [(0, rejected(RejectReason::WouldCross))]
    );
}

#[test]
fn order_is_not_live_before_it_arrives() {
    let mut sim = sim(100, 0);
    sim.submit(ms(0), &[place(0, Side::Buy, "100", "2")]);

    sim.on_market_event(&trade(50, "99", "9", Side::Sell));
    assert!(poll(&mut sim, 100).is_empty());
    // E = 50 < A = 100 cannot prove or fill the order. The later BBO
    // E = R = 101 proves acceptance, delivered at max(100, 101) = 101.
    sim.on_market_event(&bbo(101, "100", "101"));
    assert_eq!(kinds(&mut sim, 101), [(0, accepted(1))]);
}

#[test]
fn cancel_in_flight_loses_to_a_fill() {
    let mut sim = sim(100, 0);
    sim.submit(ms(0), &[place(0, Side::Buy, "100", "2")]);
    // Placement A = 100 is first proved at 101.
    sim.on_market_event(&bbo(101, "100", "101"));
    assert_eq!(kinds(&mut sim, 101), [(0, accepted(1))]);

    sim.submit(ms(101), &[cancel(0)]);
    sim.on_market_event(&trade(150, "99", "9", Side::Sell));
    // Cancel S = 101 + 100 = 201. The E = 150 print first fills
    // min(2, 9) = 2. E = 202 then proves cancel, which finds no order.
    sim.on_market_event(&bbo(202, "100", "101"));
    let events = kinds(&mut sim, 202);
    assert_eq!(events.len(), 2);
    assert_eq!(filled(events[0].1).qty, qty("2"));
    assert_eq!(
        events[1],
        (
            0,
            OrderEventKind::CancelRejected {
                reason: RejectReason::UnknownOrder
            }
        )
    );
}

#[test]
fn cancel_that_arrives_first_prevents_the_fill() {
    let mut sim = sim(100, 0);
    sim.submit(ms(0), &[place(0, Side::Buy, "100", "2")]);
    sim.on_market_event(&bbo(101, "100", "101"));
    poll(&mut sim, 101);

    sim.submit(ms(101), &[cancel(0)]);
    // S = 101 + 100 = 201. E = 202 > S proves cancel before matching,
    // so there is no resting order left for the trade to fill.
    sim.on_market_event(&trade(202, "99", "9", Side::Sell));
    assert_eq!(kinds(&mut sim, 202), [(0, canceled())]);
}

#[test]
fn l2_snapshot_sets_the_best_prices() {
    let mut sim = sim(0, 0);
    let level = |price: &str| Level {
        px: px(price),
        qty: qty("10"),
        orders: 1,
    };
    sim.on_market_event(&MarketEvent {
        instrument: MON,
        ts_exch: ms(0),
        ts_local: ms(0),
        kind: MarketEventKind::L2Snapshot(hl_types::L2Snapshot {
            bids: vec![level("100"), level("99")],
            asks: vec![level("101"), level("102")],
        }),
    });
    sim.submit(
        ms(0),
        &[
            place(0, Side::Buy, "101", "1"),
            place(1, Side::Buy, "100.5", "1"),
        ],
    );
    assert_eq!(
        proved_kinds(&mut sim, 0),
        [(0, rejected(RejectReason::WouldCross)), (1, accepted(1))]
    );
}

#[test]
fn equality_leaves_actions_pending_and_one_nanosecond_later_proves_them() {
    let mut sim = sim(10, 0);
    sim.submit(
        ms(5),
        &[
            place(0, Side::Buy, "100", "1"),
            place(1, Side::Sell, "101", "1"),
        ],
    );
    // A = 5 + 10 = 15 ms. E = 15 is equality, so neither action is proved.
    sim.on_market_event(&received(bbo(15, "100", "101"), 20));
    assert!(poll(&mut sim, 20).is_empty());
    assert_eq!(sim.pending_actions(), 2);
    assert_eq!(sim.next_wakeup(), None);

    let mut proof = received(bbo(15, "100", "101"), 21);
    proof.ts_exch = Timestamp::from_nanos(15_000_001);
    sim.on_market_event(&proof);
    // E = 15,000,001 ns > A = 15,000,000 ns. Both deliveries are
    // max(15 + 0, 21) = 21 ms, with batch order breaking the tie.
    let reports = poll(&mut sim, 21);
    assert_eq!(reports.len(), 2);
    assert_eq!((reports[0].cloid, reports[0].kind), (cloid(0), accepted(1)));
    assert_eq!((reports[1].cloid, reports[1].kind), (cloid(1), accepted(2)));
    for report in reports {
        assert_eq!((report.ts_exch, report.ts_local), (ms(15), ms(21)));
    }
    assert_eq!(sim.pending_actions(), 0);
}

#[test]
fn zero_latency_response_needs_a_later_qualifying_event() {
    let mut sim = sim(0, 0);
    sim.on_market_event(&bbo(5, "100", "101"));
    sim.submit(ms(5), &[place(0, Side::Buy, "100", "1")]);
    assert!(poll(&mut sim, 5).is_empty());
    // A = 5 + 0 = 5. Receiving E = 5 again still proves nothing.
    sim.on_market_event(&received(bbo(5, "100", "101"), 6));
    assert!(poll(&mut sim, 6).is_empty());
    sim.on_market_event(&received(bbo(6, "100", "101"), 7));
    // E = 6 > 5 first proves it, delivery max(5 + 0, 7) = 7.
    let reports = poll(&mut sim, 7);
    assert_eq!(reports.len(), 1);
    assert_eq!((reports[0].ts_exch, reports[0].ts_local), (ms(5), ms(7)));
}

#[test]
fn a_late_trade_must_follow_the_resting_versions_effect_time() {
    let mut sim = sim(100, 0);
    sim.submit(ms(0), &[place(0, Side::Buy, "100", "1")]);
    sim.on_market_event(&received(bbo(150, "100", "101"), 200));
    assert_eq!(kinds(&mut sim, 200), [(0, accepted(1))]);

    // A = 100. Neither E = 99 nor E = 100 may fill this version.
    sim.on_market_event(&received(trade(99, "99", "1", Side::Sell), 201));
    sim.on_market_event(&received(trade(100, "99", "1", Side::Sell), 202));
    assert!(poll(&mut sim, 202).is_empty());
    sim.on_market_event(&received(trade(101, "99", "0.4", Side::Sell), 203));
    // E = 101 > 100 fills min(1, 0.4) = 0.4 despite arriving after E = 150.
    let reports = poll(&mut sim, 203);
    assert_eq!(reports.len(), 1);
    assert_eq!(
        (reports[0].ts_exch, reports[0].ts_local),
        (ms(101), ms(203))
    );
    assert_eq!(filled(reports[0].kind).qty, qty("0.4"));
}

#[test]
fn an_old_bbo_cannot_replace_the_quote_used_for_post_only_validation() {
    let mut sim = sim(0, 0);
    sim.on_market_event(&received(bbo(100, "100", "101"), 110));
    sim.on_market_event(&received(bbo(50, "98", "99"), 120));
    sim.submit(ms(120), &[place(0, Side::Buy, "100.5", "1")]);
    sim.on_market_event(&received(bbo(121, "98", "99"), 130));
    // A = 120 and E = 121 proves it. The saved quote has E = 100,
    // so 100.5 < 101 passes. Both the old and the proving ask 99 would fail.
    let reports = poll(&mut sim, 130);
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].kind, accepted(1));
    assert_eq!(
        (reports[0].ts_exch, reports[0].ts_local),
        (ms(120), ms(130))
    );
}

#[test]
fn equal_exchange_time_quotes_use_the_last_receipt() {
    let mut sim = sim(0, 0);
    sim.on_market_event(&received(bbo(10, "100", "101"), 10));
    sim.on_market_event(&received(bbo(10, "99", "100"), 11));
    sim.submit(ms(11), &[place(0, Side::Buy, "100.5", "1")]);
    sim.on_market_event(&bbo(12, "100", "101"));
    // Both saved quotes have E = 10. The last receipt supplies ask 100,
    // and 100.5 >= 100 rejects before the proving ask 101 is applied.
    assert_eq!(
        kinds(&mut sim, 12),
        [(0, rejected(RejectReason::WouldCross))]
    );
}

#[test]
fn reports_wait_for_receipt_and_stay_in_order_for_each_cloid() {
    let mut sim = sim(0, 100);
    sim.submit(ms(0), &[place(0, Side::Buy, "100", "2")]);
    sim.on_market_event(&received(bbo(1, "100", "101"), 10));
    // Acceptance X = 0, R = 10. max(0 + 100, 10) = 100.
    assert!(poll(&mut sim, 99).is_empty());
    assert_eq!(poll(&mut sim, 100)[0].kind, accepted(1));

    sim.on_market_event(&received(trade(200, "99", "0.4", Side::Sell), 210));
    sim.on_market_event(&received(trade(150, "99", "0.3", Side::Sell), 220));
    // First fill is due at max(200 + 100, 210) = 300.
    // Second nominal delivery max(150 + 100, 220) = 250 is raised to
    // this cloid's prior scheduled delivery 300. Remaining size is 1.3.
    assert_eq!(sim.next_wakeup(), Some(ms(300)));
    assert!(poll(&mut sim, 299).is_empty());
    let reports = poll(&mut sim, 300);
    assert_eq!(reports.len(), 2);
    assert_eq!(
        (reports[0].ts_exch, reports[0].ts_local),
        (ms(200), ms(300))
    );
    assert_eq!(
        (reports[1].ts_exch, reports[1].ts_local),
        (ms(150), ms(300))
    );
    assert_eq!(filled(reports[0].kind).qty, qty("0.4"));
    assert_eq!(filled(reports[1].kind).qty, qty("0.3"));

    sim.on_market_event(&received(trade(250, "99", "0.2", Side::Sell), 400));
    // The receipt floor now dominates, max(250 + 100, 400, 300) = 400.
    let reports = poll(&mut sim, 400);
    assert_eq!(
        (reports[0].ts_exch, reports[0].ts_local),
        (ms(250), ms(400))
    );
    assert_eq!(filled(reports[0].kind).qty, qty("0.2"));
}

#[test]
fn reports_across_orders_follow_due_time_then_creation_order() {
    let mut sim = sim(0, 100);
    sim.submit(
        ms(0),
        &[
            place(0, Side::Buy, "100", "1"),
            place(1, Side::Sell, "101", "1"),
        ],
    );
    sim.on_market_event(&bbo(1, "100", "101"));
    poll(&mut sim, 100);

    sim.on_market_event(&received(trade(200, "99", "0.2", Side::Sell), 210));
    sim.on_market_event(&received(trade(150, "102", "0.3", Side::Buy), 220));
    // Bid fill created first is due 200 + 100 = 300. Ask fill is due
    // 150 + 100 = 250, so the latter report must be polled first.
    assert_eq!(sim.next_wakeup(), Some(ms(250)));
    let reports = poll(&mut sim, 300);
    assert_eq!(reports.len(), 2);
    assert_eq!((reports[0].cloid, reports[0].ts_local), (cloid(1), ms(250)));
    assert_eq!((reports[1].cloid, reports[1].ts_local), (cloid(0), ms(300)));

    sim.on_market_event(&received(trade(350, "99", "0.2", Side::Sell), 500));
    sim.on_market_event(&received(trade(360, "102", "0.2", Side::Buy), 500));
    // Both receipt floors give delivery 500. Creation order breaks this tie.
    let reports = poll(&mut sim, 500);
    assert_eq!(
        reports.iter().map(|event| event.cloid).collect::<Vec<_>>(),
        [cloid(0), cloid(1)]
    );
}

#[test]
fn poll_releases_determined_reports_without_proving_pending_actions() {
    let mut sim = sim(100, 50);
    sim.submit(ms(0), &[place(0, Side::Buy, "100", "1")]);
    sim.on_market_event(&bbo(101, "100", "101"));
    sim.submit(ms(110), &[cancel(0)]);
    // Acceptance A = 100 was proved by E = 101 and is due at 150.
    // Cancel S = 110 + 100 = 210 has no later evidence, even at local 1000.
    assert_eq!(sim.next_wakeup(), Some(ms(150)));
    let reports = poll(&mut sim, 1000);
    assert_eq!(reports.len(), 1);
    assert_eq!(
        (reports[0].ts_exch, reports[0].ts_local),
        (ms(100), ms(150))
    );
    assert_eq!(reports[0].kind, accepted(1));
    assert_eq!(sim.pending_actions(), 1);
    assert_eq!(sim.undelivered_reports(), 0);
    assert_eq!(sim.next_wakeup(), None);
}

#[test]
fn gaps_diffs_and_snapshots_supply_no_action_evidence() {
    let mut sim = sim(0, 0);
    sim.submit(ms(0), &[place(0, Side::Buy, "100", "1")]);
    for (time, kind) in [
        (1, MarketEventKind::FeedGap),
        (
            2,
            MarketEventKind::L2Diff(hl_types::L2Diff {
                remove_bids: vec![],
                remove_asks: vec![],
                set_bids: vec![],
                set_asks: vec![],
            }),
        ),
        (
            3,
            MarketEventKind::L2Snapshot(hl_types::L2Snapshot {
                bids: vec![],
                asks: vec![],
            }),
        ),
    ] {
        sim.on_market_event(&MarketEvent {
            instrument: MON,
            ts_exch: ms(time),
            ts_local: ms(time),
            kind,
        });
    }
    // Every E is positive, but only BBO/trade is evidence for A = 0.
    assert!(poll(&mut sim, 3).is_empty());
    assert_eq!(sim.pending_actions(), 1);
    sim.on_market_event(&bbo(4, "100", "101"));
    assert_eq!(kinds(&mut sim, 4), [(0, accepted(1))]);
}

#[test]
fn snapshots_follow_quote_recency_without_proving_an_action() {
    let mut sim = sim(0, 0);
    sim.on_market_event(&bbo(10, "100", "101"));
    sim.submit(ms(10), &[place(0, Side::Buy, "100.5", "1")]);
    let snapshot = |exchange_ms, received_ms, ask| MarketEvent {
        instrument: MON,
        ts_exch: ms(exchange_ms),
        ts_local: ms(received_ms),
        kind: MarketEventKind::L2Snapshot(hl_types::L2Snapshot {
            bids: vec![],
            asks: vec![Level {
                px: px(ask),
                qty: qty("1"),
                orders: 1,
            }],
        }),
    };
    sim.on_market_event(&snapshot(9, 11, "99"));
    assert!(poll(&mut sim, 11).is_empty());
    sim.on_market_event(&bbo(12, "100", "101"));
    // The E = 9 snapshot cannot replace E = 10 ask 101, so 100.5 passes.
    assert_eq!(kinds(&mut sim, 12), [(0, accepted(1))]);

    sim.submit(ms(12), &[place(1, Side::Buy, "100.5", "1")]);
    sim.on_market_event(&snapshot(12, 13, "100"));
    assert!(poll(&mut sim, 13).is_empty());
    sim.on_market_event(&bbo(14, "100", "101"));
    // The equal-E snapshot replaces the earlier BBO by receipt order.
    // A = 12, saved ask 100, bid 100.5 >= 100 is rejected at proof E = 14.
    assert_eq!(
        kinds(&mut sim, 14),
        [(1, rejected(RejectReason::WouldCross))]
    );

    sim.submit(ms(15), &[place(2, Side::Buy, "100.5", "1")]);
    sim.on_market_event(&snapshot(16, 16, "100"));
    assert!(poll(&mut sim, 16).is_empty());
    sim.on_market_event(&bbo(17, "100", "101"));
    // The snapshot at E=16>A=15 updates the ask to 100 but proves no
    // action. The BBO at E=17 proves the bid against that stored ask.
    assert_eq!(
        kinds(&mut sim, 17),
        [(2, rejected(RejectReason::WouldCross))]
    );
}

#[test]
fn evidence_from_another_instrument_proves_actions_globally() {
    let mut sim = sim(10, 0);
    sim.submit(ms(0), &[place(0, Side::Buy, "100", "1")]);
    let mut proof = bbo(11, "50", "51");
    proof.instrument = JUP;
    sim.on_market_event(&proof);
    // A = 10, any qualifying E = 11 proves it. JUP quotes cannot cross MON.
    assert_eq!(kinds(&mut sim, 11), [(0, accepted(1))]);
}

#[test]
fn equal_time_cancel_boundary_trade_faces_the_previously_resting_order() {
    let mut sim = sim(10, 0);
    sim.submit(ms(0), &[place(0, Side::Buy, "100", "1")]);
    sim.on_market_event(&bbo(11, "100", "101"));
    poll(&mut sim, 11);
    sim.submit(ms(20), &[cancel(0)]);
    sim.on_market_event(&trade(30, "99", "0.4", Side::Sell));
    // Cancel S = 20 + 10 = 30. E = S leaves cancel pending, and
    // the old A = 10 order fills min(1, 0.4) = 0.4 at E = 30.
    let reports = poll(&mut sim, 30);
    assert_eq!(reports.len(), 1);
    assert_eq!(filled(reports[0].kind).qty, qty("0.4"));
    assert_eq!(sim.pending_actions(), 1);
    sim.on_market_event(&bbo(31, "100", "101"));
    assert_eq!(kinds(&mut sim, 31), [(0, canceled())]);
    assert_eq!(
        sim.late_print_misses(),
        LatePrintMisses {
            trades: 0,
            qty: Qty::ZERO
        }
    );
}

#[test]
fn a_late_print_after_cancel_counts_the_saved_remainder_without_a_fill() {
    let mut sim = sim(10, 0);
    sim.submit(ms(0), &[place(0, Side::Buy, "100", "1")]);
    sim.on_market_event(&bbo(11, "100", "101"));
    poll(&mut sim, 11);
    sim.on_market_event(&trade(15, "99", "0.6", Side::Sell));
    assert_eq!(filled(poll(&mut sim, 15)[0].kind).qty, qty("0.6"));
    sim.submit(ms(20), &[cancel(0)]);
    sim.on_market_event(&received(bbo(31, "100", "101"), 40));
    assert_eq!(kinds(&mut sim, 40), [(0, canceled())]);
    sim.on_market_event(&received(trade(25, "99", "0.6", Side::Sell), 50));
    // A = 10 < E = 25 <= S = 30. Saved open size is 1 - 0.6 = 0.4,
    // so the missed allocation is min(0.6, 0.4) = 0.4, one trade.
    assert_eq!(
        sim.late_print_misses(),
        LatePrintMisses {
            trades: 1,
            qty: qty("0.4")
        }
    );
    assert!(poll(&mut sim, 50).is_empty());
    sim.submit(ms(50), &[cancel(0)]);
    sim.on_market_event(&bbo(61, "100", "101"));
    assert_eq!(
        kinds(&mut sim, 61),
        [(
            0,
            OrderEventKind::CancelRejected {
                reason: RejectReason::UnknownOrder
            }
        )]
    );
}

#[test]
fn a_late_print_after_modify_counts_only_the_old_version() {
    let mut sim = sim(10, 0);
    sim.submit(ms(0), &[place(0, Side::Buy, "100", "0.4")]);
    sim.on_market_event(&bbo(11, "100", "101"));
    poll(&mut sim, 11);
    sim.submit(
        ms(20),
        &[OrderAction::Modify {
            instrument: MON,
            cloid: cloid(0),
            px: px("98"),
            qty: qty("0.7"),
        }],
    );
    sim.on_market_event(&received(bbo(31, "100", "101"), 40));
    assert_eq!(
        kinds(&mut sim, 40),
        [(
            0,
            OrderEventKind::Modified {
                px: px("98"),
                qty: qty("0.7")
            }
        )]
    );
    sim.on_market_event(&received(trade(25, "99", "0.6", Side::Sell), 50));
    // Old A = 10 < E = 25 <= modify S = 30 gives min(0.4, 0.6) = 0.4.
    // New A = 30 > E = 25 is ineligible, and its bid 98 is below print 99.
    assert_eq!(
        sim.late_print_misses(),
        LatePrintMisses {
            trades: 1,
            qty: qty("0.4")
        }
    );
    assert!(poll(&mut sim, 50).is_empty());
    sim.on_market_event(&received(trade(35, "97", "1", Side::Sell), 60));
    // New A = 30 < E = 35 fills 0.7 at 98. Fee 98 * 0.7 * 0.00015 = 0.01029.
    let reports = poll(&mut sim, 60);
    assert_eq!(reports.len(), 1);
    let fill = filled(reports[0].kind);
    assert_eq!(
        (fill.px, fill.qty, fill.fee),
        (px("98"), qty("0.7"), usd("0.01029"))
    );
    assert_eq!(
        sim.late_print_misses(),
        LatePrintMisses {
            trades: 1,
            qty: qty("0.4")
        }
    );
}

#[test]
fn late_print_interval_excludes_start_and_includes_superseding_time() {
    for (exchange_ms, expected_trades, expected_qty) in [(10, 0, "0"), (30, 1, "0.4"), (31, 0, "0")]
    {
        let mut sim = sim(10, 0);
        sim.submit(ms(0), &[place(0, Side::Buy, "100", "0.4")]);
        sim.on_market_event(&bbo(11, "100", "101"));
        poll(&mut sim, 11);
        sim.submit(ms(20), &[cancel(0)]);
        sim.on_market_event(&received(bbo(31, "100", "101"), 40));
        poll(&mut sim, 40);
        sim.on_market_event(&received(trade(exchange_ms, "99", "0.6", Side::Sell), 50));
        // The saved interval is 10 < E <= 30. E = 10 and 31 allocate 0.
        // E = S = 30 allocates min(saved 0.4, print 0.6) = 0.4 exactly once.
        assert_eq!(
            sim.late_print_misses(),
            LatePrintMisses {
                trades: expected_trades,
                qty: qty(expected_qty)
            }
        );
        assert!(poll(&mut sim, 50).is_empty());
    }
}

#[test]
fn late_prints_share_one_budget_best_price_then_oldest_and_exhaust_saved_sizes() {
    let mut sim = sim(0, 0);
    sim.submit(
        ms(0),
        &[
            place(0, Side::Buy, "100", "0.3"),
            place(1, Side::Buy, "101", "0.2"),
            place(2, Side::Buy, "101", "0.4"),
        ],
    );
    // With no initial quote all three bids pass before the proof quote arrives.
    prove_and_poll(&mut sim, 0);
    sim.submit(ms(10), &[cancel(0), cancel(1)]);
    prove_and_poll(&mut sim, 10);
    sim.submit(ms(20), &[cancel(2)]);
    prove_and_poll(&mut sim, 20);

    sim.on_market_event(&received(trade(5, "99", "0.5", Side::Sell), 30));
    // Best bids 101 first, older id 1 takes 0.2, then id 2 takes 0.3.
    // One print contributes 0.5 total. Saved remainders are 0.3, 0, 0.1.
    assert_eq!(
        sim.late_print_misses(),
        LatePrintMisses {
            trades: 1,
            qty: qty("0.5")
        }
    );
    sim.on_market_event(&received(trade(15, "100.5", "0.5", Side::Sell), 31));
    // Only id 2 has S >= E = 15. It contributes its last 0.1, total 0.6.
    assert_eq!(
        sim.late_print_misses(),
        LatePrintMisses {
            trades: 2,
            qty: qty("0.6")
        }
    );
    sim.on_market_event(&received(trade(5, "99", "1", Side::Sell), 32));
    // Only id 0 has quantity left. min(1, 0.3) = 0.3, total 0.9.
    assert_eq!(
        sim.late_print_misses(),
        LatePrintMisses {
            trades: 3,
            qty: qty("0.9")
        }
    );
    sim.on_market_event(&received(trade(5, "99", "1", Side::Sell), 33));
    assert_eq!(
        sim.late_print_misses(),
        LatePrintMisses {
            trades: 3,
            qty: qty("0.9")
        }
    );
    assert!(poll(&mut sim, 33).is_empty());
}

#[test]
fn consecutive_late_prints_claim_only_the_saved_remainder() {
    let mut sim = sim(0, 0);
    sim.submit(ms(0), &[place(0, Side::Buy, "100", "0.4")]);
    prove_and_poll(&mut sim, 0);
    sim.submit(ms(10), &[cancel(0)]);
    prove_and_poll(&mut sim, 10);
    for (received_ms, expected_trades, expected_qty) in
        [(20, 1, "0.3"), (21, 2, "0.4"), (22, 2, "0.4")]
    {
        sim.on_market_event(&received(trade(5, "99", "0.3", Side::Sell), received_ms));
        // Allocations are min(0.3, 0.4) = 0.3, then min(0.3, 0.1) = 0.1,
        // then zero. The same saved 0.4 can be claimed only once in total.
        assert_eq!(
            sim.late_print_misses(),
            LatePrintMisses {
                trades: expected_trades,
                qty: qty(expected_qty)
            }
        );
    }
    assert!(poll(&mut sim, 22).is_empty());
}

#[test]
fn cancel_all_preserves_missed_versions_across_a_gap() {
    let mut sim = sim(0, 0);
    sim.submit(
        ms(0),
        &[
            place(0, Side::Buy, "100", "0.4"),
            place(1, Side::Sell, "101", "0.4"),
        ],
    );
    prove_and_poll(&mut sim, 0);
    sim.submit(
        ms(10),
        &[OrderAction::CancelAll {
            instrument: Some(MON),
        }],
    );
    prove_and_poll(&mut sim, 10);
    sim.on_market_event(&MarketEvent {
        instrument: MON,
        ts_exch: ms(100),
        ts_local: ms(100),
        kind: MarketEventKind::FeedGap,
    });
    sim.on_market_event(&received(trade(5, "99", "0.6", Side::Sell), 200));
    sim.on_market_event(&received(trade(5, "102", "0.6", Side::Buy), 201));
    // Each side has one saved 0.4. Opposite prints allocate 0.4 + 0.4 = 0.8.
    assert_eq!(
        sim.late_print_misses(),
        LatePrintMisses {
            trades: 2,
            qty: qty("0.8")
        }
    );
    assert!(poll(&mut sim, 201).is_empty());
}

#[test]
fn missed_print_matching_requires_instrument_side_and_strict_trade_through() {
    let mut sim = sim(0, 0);
    sim.submit(ms(0), &[place(0, Side::Buy, "100", "0.4")]);
    prove_and_poll(&mut sim, 0);
    sim.submit(ms(10), &[cancel(0)]);
    prove_and_poll(&mut sim, 10);
    let mut other = received(trade(5, "99", "0.6", Side::Sell), 20);
    other.instrument = JUP;
    sim.on_market_event(&other);
    sim.on_market_event(&received(trade(5, "99", "0.6", Side::Buy), 21));
    sim.on_market_event(&received(trade(5, "100", "0.6", Side::Sell), 22));
    // Wrong instrument, same-side aggressor and price equality allocate zero.
    assert_eq!(
        sim.late_print_misses(),
        LatePrintMisses {
            trades: 0,
            qty: Qty::ZERO
        }
    );
    sim.on_market_event(&received(trade(5, "99", "0.6", Side::Sell), 23));
    assert_eq!(
        sim.late_print_misses(),
        LatePrintMisses {
            trades: 1,
            qty: qty("0.4")
        }
    );
    assert!(poll(&mut sim, 23).is_empty());
}

#[test]
fn a_rejected_modify_keeps_the_original_effect_time_and_saves_no_version() {
    let mut sim = sim(10, 0);
    sim.submit(ms(0), &[place(0, Side::Buy, "100", "0.4")]);
    sim.on_market_event(&bbo(11, "100", "101"));
    poll(&mut sim, 11);
    sim.submit(
        ms(20),
        &[OrderAction::Modify {
            instrument: MON,
            cloid: cloid(0),
            px: px("101"),
            qty: qty("0.4"),
        }],
    );
    sim.on_market_event(&received(bbo(31, "100", "101"), 40));
    assert_eq!(
        kinds(&mut sim, 40),
        [(
            0,
            OrderEventKind::ModifyRejected {
                reason: RejectReason::WouldCross
            }
        )]
    );
    sim.on_market_event(&received(trade(25, "99", "0.6", Side::Sell), 50));
    // Rejection leaves A = 10. E = 25 fills min(0.4, 0.6) = 0.4 live,
    // and no superseded version exists to claim a missed allocation.
    assert_eq!(filled(poll(&mut sim, 50)[0].kind).qty, qty("0.4"));
    assert_eq!(
        sim.late_print_misses(),
        LatePrintMisses {
            trades: 0,
            qty: Qty::ZERO
        }
    );
}

#[test]
fn modifying_a_price_preserves_the_existing_matching_priority() {
    let mut sim = sim(0, 0);
    sim.submit(
        ms(0),
        &[
            place(0, Side::Buy, "99", "1"),
            place(1, Side::Buy, "100", "1"),
        ],
    );
    prove_and_poll(&mut sim, 0);
    sim.submit(
        ms(10),
        &[OrderAction::Modify {
            instrument: MON,
            cloid: cloid(0),
            px: px("100"),
            qty: qty("1"),
        }],
    );
    prove_and_poll(&mut sim, 10);
    sim.on_market_event(&trade(20, "99", "1.5", Side::Sell));
    // Both bids are 100. Existing priority keeps id 0 first after modify,
    // so its 1 fills before id 1 takes the remaining 1.5 - 1 = 0.5.
    let reports = kinds(&mut sim, 20);
    assert_eq!(reports.len(), 2);
    assert_eq!((reports[0].0, filled(reports[0].1).qty), (0, qty("1")));
    assert_eq!((reports[1].0, filled(reports[1].1).qty), (1, qty("0.5")));
}

#[test]
fn a_late_trade_cannot_fill_the_modified_version_even_through_its_price() {
    let mut sim = sim(10, 0);
    sim.submit(ms(0), &[place(0, Side::Buy, "100", "0.4")]);
    sim.on_market_event(&bbo(11, "100", "101"));
    poll(&mut sim, 11);
    sim.submit(
        ms(20),
        &[OrderAction::Modify {
            instrument: MON,
            cloid: cloid(0),
            px: px("99"),
            qty: qty("0.7"),
        }],
    );
    sim.on_market_event(&received(bbo(31, "100", "101"), 40));
    poll(&mut sim, 40);
    sim.on_market_event(&received(trade(25, "98", "0.2", Side::Sell), 50));
    sim.on_market_event(&received(trade(30, "98", "0.2", Side::Sell), 51));
    // Both prices 100 and 99 are through the print 98, but the new version
    // has A = 30. E = 25 and E = 30 are ineligible for a real fill.
    // The old interval 10 < E <= 30 receives 0.2 + 0.2 = 0.4 missed units.
    assert!(poll(&mut sim, 51).is_empty());
    assert_eq!(
        sim.late_print_misses(),
        LatePrintMisses {
            trades: 2,
            qty: qty("0.4")
        }
    );
    sim.on_market_event(&received(trade(31, "98", "1", Side::Sell), 52));
    // New A = 30 < E = 31 now fills its untouched min(0.7, 1) = 0.7.
    let reports = poll(&mut sim, 52);
    assert_eq!(reports.len(), 1);
    assert_eq!(
        (filled(reports[0].kind).px, filled(reports[0].kind).qty),
        (px("99"), qty("0.7"))
    );
}

#[test]
fn same_time_modifications_preserve_the_earlier_nonempty_saved_interval() {
    let mut sim = sim(0, 0);
    sim.submit(ms(0), &[place(0, Side::Buy, "100", "0.4")]);
    prove_and_poll(&mut sim, 0);
    sim.submit(
        ms(10),
        &[OrderAction::Modify {
            instrument: MON,
            cloid: cloid(0),
            px: px("99"),
            qty: qty("0.5"),
        }],
    );
    sim.submit(
        ms(10),
        &[
            OrderAction::Modify {
                instrument: MON,
                cloid: cloid(0),
                px: px("98"),
                qty: qty("0.6"),
            },
            cancel(0),
        ],
    );
    let reports = proved_kinds(&mut sim, 10);
    // Both submit calls have A = 10. Submission and batch order produce
    // modify 99, modify 98, cancel. Only the original interval 0 < E <= 10
    // is nonempty. The intermediate versions each have 10 < E <= 10.
    assert_eq!(
        reports,
        [
            (
                0,
                OrderEventKind::Modified {
                    px: px("99"),
                    qty: qty("0.5")
                }
            ),
            (
                0,
                OrderEventKind::Modified {
                    px: px("98"),
                    qty: qty("0.6")
                }
            ),
            (0, canceled()),
        ]
    );
    sim.on_market_event(&received(trade(5, "99.5", "1", Side::Sell), 20));
    // The original bid 100 contributes min(0.4, 1) = 0.4 exactly once.
    assert_eq!(
        sim.late_print_misses(),
        LatePrintMisses {
            trades: 1,
            qty: qty("0.4")
        }
    );
    assert!(poll(&mut sim, 20).is_empty());
}
