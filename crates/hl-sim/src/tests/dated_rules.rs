use hl_rules::{RuleBook, RuleError, RuleHistory, RuleVersion};

use super::*;

const ABSENT: InstrumentId = InstrumentId::new(9);

fn ns(nanos: i64) -> Timestamp {
    Timestamp::from_nanos(nanos)
}

fn sim_with_rules(order_latency_ns: u64) -> SimGateway {
    let mut rules = RuleBook::new();
    let history = RuleHistory::new(vec![
        RuleVersion::new(ns(100), 2, 17).unwrap(),
        RuleVersion::new(ns(200), 3, 21).unwrap(),
    ])
    .unwrap();
    rules.insert(MON, history).unwrap();
    let other = RuleHistory::new(vec![RuleVersion::new(ns(0), 2, 30).unwrap()]).unwrap();
    rules.insert(JUP, other).unwrap();
    SimGateway::with_rules(
        SimConfig {
            order_latency: Duration::from_nanos(order_latency_ns),
            report_latency: Duration::ZERO,
            maker_fee: FeeRate::ZERO,
            min_order_notional: usd("10"),
        },
        rules,
    )
}

fn place_spec(seq: u64, order: OrderSpec) -> OrderAction {
    OrderAction::Place {
        cloid: cloid(seq),
        order,
    }
}

fn modify(seq: u64, instrument: InstrumentId, price: &str, size: &str) -> OrderAction {
    OrderAction::Modify {
        instrument,
        cloid: cloid(seq),
        px: px(price),
        qty: qty(size),
    }
}

fn modify_rejected(reason: RejectReason) -> OrderEventKind {
    OrderEventKind::ModifyRejected { reason }
}

fn outcomes(reports: &[OrderEvent]) -> Vec<(u64, Timestamp, OrderEventKind)> {
    reports
        .iter()
        .map(|report| (report.cloid.seq(), report.ts_exch, report.kind))
        .collect()
}

#[test]
fn price_and_size_violations_reject_the_action_and_preserve_the_resting_order() {
    for (price, size) in [
        ("99.9999", "1"),
        ("99", "1.0001"),
        ("101.001", "1"),
        ("101", "1.0001"),
        ("101.001", "1.0001"),
    ] {
        let mut sim = sim_with_rules(0);
        sim.submit(ms(1), &[place(0, Side::Buy, "100", "2")]);
        // The proof also installs a 100 / 101 BBO for both checks below.
        assert_eq!(proved_kinds(&mut sim, 1), [(0, accepted(1))]);
        sim.submit(
            ms(2),
            &[
                place(1, Side::Buy, price, size),
                modify(0, MON, price, size),
            ],
        );
        let invalid = RejectReason::InvalidOrder;
        assert_eq!(
            outcomes(&prove_and_poll(&mut sim, 2)),
            [
                (1, ms(2), rejected(invalid)),
                (0, ms(2), modify_rejected(invalid))
            ],
            "price {price}, size {size}"
        );
        sim.on_market_event(&trade(3, "99.99", "3", Side::Sell))
            .unwrap();
        let reports = poll(&mut sim, 3);
        assert_eq!(reports.len(), 1);
        let fill = filled(reports[0].kind);
        assert_eq!(
            (reports[0].cloid, fill.px, fill.qty),
            (cloid(0), px("100"), qty("2"))
        );
    }
}

#[test]
fn covered_orders_keep_the_earliest_rejection() {
    let mut sim = sim_with_rules(0);
    sim.submit(ms(1), &[place(0, Side::Buy, "100", "1")]);
    assert_eq!(proved_kinds(&mut sim, 1), [(0, accepted(1))]);
    let unsupported = OrderSpec {
        tif: TimeInForce::Ioc,
        ..spec(MON, Side::Buy, "101.001", "0")
    };
    sim.submit(
        ms(2),
        &[
            place_spec(0, unsupported),
            place_spec(1, unsupported),
            place(2, Side::Buy, "100", "0"),
            place(3, Side::Buy, "100.001", "0.01"),
            place(4, Side::Buy, "101", "1"),
            modify(0, MON, "100", "0"),
            modify(0, MON, "100.001", "0.01"),
            modify(0, MON, "101", "1"),
            modify(5, MON, "101.001", "0"),
        ],
    );
    assert_eq!(
        proved_kinds(&mut sim, 2),
        [
            (0, rejected(RejectReason::DuplicateClientOrderId)),
            (1, rejected(RejectReason::Unsupported)),
            (2, rejected(RejectReason::InvalidOrder)),
            (3, rejected(RejectReason::BelowMinNotional)),
            (4, rejected(RejectReason::WouldCross)),
            (0, modify_rejected(RejectReason::InvalidOrder)),
            (0, modify_rejected(RejectReason::BelowMinNotional)),
            (0, modify_rejected(RejectReason::WouldCross)),
            (5, modify_rejected(RejectReason::UnknownOrder)),
        ]
    );
}

#[test]
fn one_bbo_proves_two_orders_on_opposite_sides_of_a_rule_change() {
    let mut sim = sim_with_rules(10);
    sim.submit(ns(189), &[place(0, Side::Buy, "100", "1.001")]);
    sim.submit(ns(190), &[place(1, Side::Buy, "100", "1.001")]);
    sim.on_market_event(&MarketEvent {
        ts_exch: ns(199),
        ts_local: ns(199),
        ..bbo(0, "99", "101")
    })
    .unwrap();
    assert_eq!((sim.pending_actions(), sim.undelivered_reports()), (2, 0));
    sim.on_market_event(&bbo(1, "99", "101")).unwrap();
    assert_eq!(
        outcomes(&poll(&mut sim, 1)),
        [
            (0, ns(199), rejected(RejectReason::InvalidOrder)),
            (1, ns(200), accepted(1)),
        ]
    );
}

#[test]
fn modify_uses_the_resting_instrument_at_the_modifications_effect_time() {
    let mut sim = sim_with_rules(10);
    sim.submit(ns(90), &[place(0, Side::Buy, "100", "2")]);
    // These actions name an absent instrument. The resting order is on MON.
    sim.submit(ns(189), &[modify(0, ABSENT, "100", "1.001")]);
    sim.submit(ns(190), &[modify(0, ABSENT, "100", "1.001")]);
    sim.on_market_event(&bbo(1, "99", "101")).unwrap();
    assert_eq!(
        outcomes(&poll(&mut sim, 1)),
        [
            (0, ns(100), accepted(1)),
            (0, ns(199), modify_rejected(RejectReason::InvalidOrder)),
            (
                0,
                ns(200),
                OrderEventKind::Modified {
                    px: px("100"),
                    qty: qty("1.001")
                }
            ),
        ]
    );
}

#[test]
fn missing_coverage_or_history_stops_before_any_ordinary_rejection() {
    let missing = RuleError::MissingCoverage {
        ts_exch: ns(99),
        first_start: ns(100),
    };
    for (id, cause) in [
        (MON, missing),
        (ABSENT, RuleError::UnknownInstrument(ABSENT)),
    ] {
        let order = spec(id, Side::Buy, "100", "1");
        for action in [
            place_spec(1, order),
            place_spec(0, order),
            place_spec(
                1,
                OrderSpec {
                    tif: TimeInForce::Ioc,
                    ..order
                },
            ),
            modify(1, id, "100", "1"),
        ] {
            let mut sim = sim_with_rules(10);
            // JUP is covered at 98 ns. Its cloid is already in use at 99 ns.
            sim.submit(ns(88), &[place_spec(0, spec(JUP, Side::Buy, "100", "1"))]);
            sim.submit(ns(89), &[action]);
            let error = sim
                .on_market_event(&trade(1, "99", "2", Side::Sell))
                .unwrap_err();
            assert_eq!(
                (error.instrument, error.effective_at, error.source),
                (id, ns(99), cause)
            );
            assert_eq!(outcomes(&poll(&mut sim, 1)), [(0, ns(98), accepted(1))]);
            assert_eq!(sim.pending_actions(), 0);
        }
    }
}

#[test]
fn a_lookup_error_consumes_the_action_and_preserves_the_rest_of_the_queue() {
    let mut sim = sim_with_rules(0);
    sim.submit(ns(98), &[place_spec(0, spec(JUP, Side::Buy, "100", "1"))]);
    sim.submit(ns(99), &[place(1, Side::Buy, "100", "1"), cancel(2)]);
    sim.submit(ns(100), &[place(1, Side::Buy, "100", "1")]);
    let proof = MarketEvent {
        instrument: JUP,
        ..trade(1, "99", "1", Side::Sell)
    };
    assert!(sim.on_market_event(&proof).is_err());
    assert_eq!(sim.pending_actions(), 2);
    // The first order rested, and the trade that would fill it has not run.
    assert_eq!(outcomes(&poll(&mut sim, 1)), [(0, ns(98), accepted(1))]);

    sim.on_market_event(&proof).unwrap();
    let reports = poll(&mut sim, 1);
    assert_eq!(reports.len(), 3);
    assert_eq!(
        outcomes(&reports[..2]),
        [
            (
                2,
                ns(99),
                OrderEventKind::CancelRejected {
                    reason: RejectReason::UnknownOrder
                }
            ),
            (1, ns(100), accepted(2)),
        ]
    );
    assert_eq!(
        (reports[2].cloid, filled(reports[2].kind).qty),
        (cloid(0), qty("1"))
    );
    assert_eq!(sim.pending_actions(), 0);
}

#[test]
fn cancels_and_cancel_all_bypass_rule_coverage() {
    let mut sim = sim_with_rules(0);
    sim.submit(ns(98), &[place_spec(0, spec(JUP, Side::Buy, "100", "1"))]);
    sim.submit(
        ns(99),
        &[
            cancel(1),
            OrderAction::Cancel {
                instrument: ABSENT,
                cloid: cloid(2),
            },
            OrderAction::CancelAll {
                instrument: Some(MON),
            },
            OrderAction::CancelAll {
                instrument: Some(ABSENT),
            },
            OrderAction::CancelAll { instrument: None },
        ],
    );
    let unknown = OrderEventKind::CancelRejected {
        reason: RejectReason::UnknownOrder,
    };
    assert_eq!(
        proved_kinds(&mut sim, 1),
        [
            (0, accepted(1)),
            (1, unknown),
            (2, unknown),
            (0, canceled())
        ]
    );
}

#[test]
fn a_venue_without_rules_accepts_off_grid_values() {
    let mut sim = sim(0, 0);
    sim.submit(
        ns(99),
        &[
            place(0, Side::Buy, "99.9999", "1.0001"),
            modify(0, MON, "99.9998", "2.0001"),
        ],
    );
    assert_eq!(
        proved_kinds(&mut sim, 1),
        [
            (0, accepted(1)),
            (
                0,
                OrderEventKind::Modified {
                    px: px("99.9998"),
                    qty: qty("2.0001")
                }
            )
        ]
    );
}
