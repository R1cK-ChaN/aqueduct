use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use hl_backtest::recording::Recording;
use hl_run::ReplayTrace;
use hl_types::{OrderAction, OrderEventKind, Qty, RejectReason, Side};
use serde_json::{Value, json};

pub(super) struct JsonObject {
    members: Vec<(&'static str, String)>,
}

impl JsonObject {
    pub(super) fn new<const N: usize>(fields: [(&'static str, Value); N]) -> Self {
        Self {
            members: fields
                .into_iter()
                .map(|(name, value)| (name, value.to_string()))
                .collect(),
        }
    }

    fn append(&mut self, name: &'static str, value: Value) {
        self.members.push((name, value.to_string()));
    }

    pub(super) fn append_object(&mut self, name: &'static str, value: Self) {
        self.members.push((name, value.to_string()));
    }
}

impl fmt::Display for JsonObject {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("{")?;
        for (index, (name, value)) in self.members.iter().enumerate() {
            if index > 0 {
                formatter.write_str(",")?;
            }
            write!(formatter, "{}:{value}", json!(name))?;
        }
        formatter.write_str("}")
    }
}

/// Nearest-rank percentiles over all delivered reports, including zero waits.
fn wait_summary(mut waits: Vec<u64>) -> JsonObject {
    waits.sort_unstable();
    let count = waits.len();
    let percentile = |percent: u128| {
        if count == 0 {
            return None;
        }
        let rank = (count as u128 * percent).div_ceil(100) as usize;
        Some(waits[rank - 1])
    };
    JsonObject::new([
        ("count", json!(count)),
        (
            "delayed",
            json!(count - waits.partition_point(|&ns| ns == 0)),
        ),
        ("p50", json!(percentile(50))),
        ("p90", json!(percentile(90))),
        ("p99", json!(percentile(99))),
        ("max", json!(waits.last())),
    ])
}

pub(super) fn add_results(
    summary: &mut JsonObject,
    recording: &Recording,
    trace: &ReplayTrace,
    open_orders: usize,
    report_latency: Duration,
) -> Result<(), String> {
    let (first, last) = recording
        .events
        .first()
        .zip(recording.events.last())
        .ok_or_else(|| "cannot summarize an empty recording".to_owned())?;
    let mut actions = BTreeMap::from([
        ("place", 0_u64),
        ("modify", 0),
        ("cancel", 0),
        ("cancel_all", 0),
    ]);
    for (_, action) in &trace.actions {
        let name = match action {
            OrderAction::Place { .. } => "place",
            OrderAction::Modify { .. } => "modify",
            OrderAction::Cancel { .. } => "cancel",
            OrderAction::CancelAll { .. } => "cancel_all",
        };
        *actions.entry(name).or_default() += 1;
    }

    let mut reports = BTreeMap::from([
        ("accepted", 0_u64),
        ("rejected", 0),
        ("filled", 0),
        ("modified", 0),
        ("modify_rejected", 0),
        ("canceled", 0),
        ("cancel_rejected", 0),
    ]);
    let mut rejections = BTreeMap::from([
        ("would_cross", 0_u64),
        ("below_min_notional", 0),
        ("invalid_order", 0),
        ("duplicate_client_order_id", 0),
        ("unknown_order", 0),
        ("unsupported", 0),
    ]);
    let mut bought_qty = Qty::ZERO;
    let mut sold_qty = Qty::ZERO;
    let mut action_waits = Vec::new();
    let mut fill_waits = Vec::new();
    for event in &trace.order_events {
        let nominal_due = i128::from(event.ts_exch.as_nanos()) + report_latency.as_nanos() as i128;
        let wait = u64::try_from(i128::from(event.ts_local.as_nanos()) - nominal_due)
            .map_err(|_| "report delivered before its configured latency")?;
        match event.kind {
            OrderEventKind::Filled(_) => fill_waits.push(wait),
            _ => action_waits.push(wait),
        }
        let (name, reason) =
            match event.kind {
                OrderEventKind::Accepted { .. } => ("accepted", None),
                OrderEventKind::Rejected { reason } => ("rejected", Some(reason)),
                OrderEventKind::Filled(fill) => {
                    let total = match fill.side {
                        Side::Buy => &mut bought_qty,
                        Side::Sell => &mut sold_qty,
                    };
                    *total =
                        Qty::from_e8(total.as_e8().checked_add(fill.qty.as_e8()).ok_or_else(
                            || "total filled size exceeds fixed-point range".to_owned(),
                        )?);
                    ("filled", None)
                }
                OrderEventKind::Modified { .. } => ("modified", None),
                OrderEventKind::ModifyRejected { reason } => ("modify_rejected", Some(reason)),
                OrderEventKind::Canceled { .. } => ("canceled", None),
                OrderEventKind::CancelRejected { reason } => ("cancel_rejected", Some(reason)),
            };
        *reports.entry(name).or_default() += 1;
        if let Some(reason) = reason {
            let name = match reason {
                RejectReason::WouldCross => "would_cross",
                RejectReason::BelowMinNotional => "below_min_notional",
                RejectReason::InvalidOrder => "invalid_order",
                RejectReason::DuplicateClientOrderId => "duplicate_client_order_id",
                RejectReason::UnknownOrder => "unknown_order",
                RejectReason::Unsupported => "unsupported",
            };
            *rejections.entry(name).or_default() += 1;
        }
    }

    let stats = &recording.stats;
    let mut report_summary = json!(reports);
    report_summary["rejections_by_reason"] = json!(rejections);
    summary.append("first_ts_local_ms", json!(first.ts_local.as_millis()));
    summary.append("last_ts_local_ms", json!(last.ts_local.as_millis()));
    summary.append_object(
        "events",
        JsonObject::new([
            ("bbo", json!(stats.bbo)),
            ("trades_live", json!(stats.trades_live)),
            ("feed_gaps", json!(stats.feed_gaps)),
        ]),
    );
    summary.append_object(
        "skipped",
        JsonObject::new([
            ("trades_replayed", json!(stats.trades_replayed)),
            ("decode_errors", json!(stats.decode_errors)),
            ("truncated_files", json!(stats.truncated_files)),
        ]),
    );
    summary.append_object(
        "gaps",
        JsonObject::new([
            ("disconnected", json!(stats.gaps.disconnected)),
            ("shutdown", json!(stats.gaps.shutdown)),
            ("dropped", json!(stats.gaps.dropped)),
            ("process_change", json!(stats.gaps.process_change)),
            ("truncated", json!(stats.gaps.truncated)),
        ]),
    );
    summary.append_object(
        "time",
        JsonObject::new([
            ("negative_latency", json!(stats.negative_latency)),
            ("local_time_raised", json!(stats.local_time_raised)),
            (
                "exchange_time_out_of_order",
                json!(stats.exchange_time_out_of_order),
            ),
            ("process_clock_raised", json!(stats.process_clock_raised)),
            (
                "process_clock_max_raise_ns",
                json!(stats.process_clock_max_raise_ns),
            ),
        ]),
    );
    summary.append("actions", json!(actions));
    summary.append("reports", report_summary);
    summary.append_object(
        "fills",
        JsonObject::new([
            ("count", json!(reports["filled"])),
            ("bought_qty", json!(bought_qty.to_string())),
            ("sold_qty", json!(sold_qty.to_string())),
        ]),
    );
    summary.append("open_orders_at_end", json!(open_orders));
    summary.append_object(
        "late_print_misses",
        JsonObject::new([
            ("trades", json!(trace.late_print_misses.trades)),
            ("qty", json!(trace.late_print_misses.qty.to_string())),
        ]),
    );
    let pending = &trace.pending_at_end;
    summary.append_object(
        "pending_at_end",
        JsonObject::new([
            ("actions", json!(pending.actions)),
            ("reports", json!(pending.reports)),
            ("timers", json!(pending.timers)),
        ]),
    );
    let mut waits = JsonObject::new([]);
    waits.append_object("action", wait_summary(action_waits));
    waits.append_object("fill", wait_summary(fill_waits));
    summary.append_object("report_wait_ns", waits);
    Ok(())
}

#[cfg(test)]
mod tests {
    use hl_run::recording::{GapCounts, ReaderStats};
    use hl_types::{
        CancelReason, ClientOrderId, Fill, InstrumentId, Liquidity, MarketEvent, MarketEventKind,
        OrderEvent, OrderId, OrderSpec, Px, TimeInForce, Timestamp, TradeId, Usd,
    };

    use super::*;

    const INSTRUMENT: InstrumentId = InstrumentId::new(1);
    const CLOID: ClientOrderId = ClientOrderId::from_u128(1);

    fn recording() -> Recording {
        Recording {
            events: [1_500, 2_500]
                .map(|time| MarketEvent {
                    instrument: INSTRUMENT,
                    ts_exch: Timestamp::from_millis(time),
                    ts_local: Timestamp::from_millis(time),
                    kind: MarketEventKind::FeedGap,
                })
                .to_vec(),
            stats: ReaderStats::default(),
            stop_time: Some(Timestamp::from_millis(2_500)),
            ..Recording::default()
        }
    }

    fn report(kind: OrderEventKind) -> OrderEvent {
        OrderEvent {
            instrument: INSTRUMENT,
            cloid: CLOID,
            ts_exch: Timestamp::EPOCH,
            ts_local: Timestamp::EPOCH,
            kind,
        }
    }

    fn fill(side: Side, qty: Qty) -> OrderEvent {
        report(OrderEventKind::Filled(Fill {
            tid: TradeId(1),
            oid: OrderId(1),
            side,
            px: Px::from_e8(100_000_000),
            qty,
            liquidity: Liquidity::Maker,
            fee: Usd::ZERO,
        }))
    }

    #[test]
    fn summary_counts_every_action_report_and_rejection_reason() {
        let mut recording = recording();
        recording.stats = ReaderStats {
            bbo: 1,
            trades_live: 2,
            feed_gaps: 3,
            trades_replayed: 4,
            decode_errors: 5,
            truncated_files: 6,
            gaps: GapCounts {
                disconnected: 7,
                shutdown: 8,
                dropped: 9,
                process_change: 10,
                truncated: 11,
            },
            negative_latency: 12,
            local_time_raised: 13,
            exchange_time_out_of_order: 14,
            process_clock_raised: 15,
            process_clock_max_raise_ns: 16,
        };
        let order = OrderSpec {
            instrument: INSTRUMENT,
            side: Side::Buy,
            px: Px::from_e8(100_000_000),
            qty: Qty::from_e8(100_000_000),
            tif: TimeInForce::Alo,
            reduce_only: false,
        };
        let mut trace = ReplayTrace {
            actions: [
                OrderAction::Place {
                    cloid: CLOID,
                    order,
                },
                OrderAction::Modify {
                    instrument: INSTRUMENT,
                    cloid: CLOID,
                    px: order.px,
                    qty: order.qty,
                },
                OrderAction::Cancel {
                    instrument: INSTRUMENT,
                    cloid: CLOID,
                },
                OrderAction::CancelAll {
                    instrument: Some(INSTRUMENT),
                },
            ]
            .map(|action| (Timestamp::EPOCH, action))
            .to_vec(),
            order_events: vec![
                report(OrderEventKind::Accepted { oid: OrderId(1) }),
                report(OrderEventKind::Modified {
                    px: order.px,
                    qty: order.qty,
                }),
                report(OrderEventKind::Canceled {
                    reason: CancelReason::Requested,
                }),
                fill(Side::Buy, "0.125".parse().unwrap()),
                fill(Side::Sell, "2.00000001".parse().unwrap()),
            ],
            ..ReplayTrace::default()
        };
        for reason in [
            RejectReason::WouldCross,
            RejectReason::BelowMinNotional,
            RejectReason::InvalidOrder,
            RejectReason::DuplicateClientOrderId,
            RejectReason::UnknownOrder,
            RejectReason::Unsupported,
        ] {
            trace.order_events.extend([
                report(OrderEventKind::Rejected { reason }),
                report(OrderEventKind::ModifyRejected { reason }),
                report(OrderEventKind::CancelRejected { reason }),
            ]);
        }
        let mut summary = JsonObject::new([("coin", json!("BTC")), ("min_notional", json!("10"))]);
        add_results(&mut summary, &recording, &trace, 2, Duration::ZERO).unwrap();
        let stdout = summary.to_string();
        // Distinct fixture values also check the order of all five clock fields.
        assert!(stdout.contains(concat!(
            r#""time":{"negative_latency":12,"local_time_raised":13,"exchange_time_out_of_order":14,"#,
            r#""process_clock_raised":15,"process_clock_max_raise_ns":16}"#,
        )));
        let summary: Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(summary["first_ts_local_ms"], 1_500);
        assert_eq!(summary["last_ts_local_ms"], 2_500);
        assert_eq!(
            summary["events"],
            json!({"bbo": 1, "trades_live": 2, "feed_gaps": 3})
        );
        assert_eq!(
            summary["skipped"],
            json!({"trades_replayed": 4, "decode_errors": 5, "truncated_files": 6})
        );
        assert_eq!(
            summary["gaps"],
            json!({"disconnected": 7, "shutdown": 8, "dropped": 9, "process_change": 10, "truncated": 11})
        );
        assert_eq!(
            summary["actions"],
            json!({"place": 1, "modify": 1, "cancel": 1, "cancel_all": 1})
        );
        assert_eq!(
            summary["reports"],
            json!({
                "accepted": 1, "rejected": 6, "filled": 2, "modified": 1, "modify_rejected": 6,
                "canceled": 1, "cancel_rejected": 6,
                "rejections_by_reason": {
                    "would_cross": 3, "below_min_notional": 3, "invalid_order": 3,
                    "duplicate_client_order_id": 3, "unknown_order": 3, "unsupported": 3,
                },
            })
        );
        assert_eq!(
            summary["fills"],
            json!({"count": 2, "bought_qty": "0.125", "sold_qty": "2.00000001"})
        );
        assert_eq!(summary["open_orders_at_end"], 2);
        // 1 accepted + 1 modified + 1 canceled + 6*3 rejections = 21
        // action reports. The two fills have their own distribution.
        assert_eq!(summary["report_wait_ns"]["action"]["count"], 21);
        assert_eq!(summary["report_wait_ns"]["fill"]["count"], 2);
    }

    #[test]
    fn empty_trace_retains_all_zero_counts() {
        let mut summary = JsonObject::new([]);
        add_results(
            &mut summary,
            &recording(),
            &ReplayTrace::default(),
            0,
            Duration::ZERO,
        )
        .unwrap();
        let summary: Value = serde_json::from_str(&summary.to_string()).unwrap();
        for (key, expected_count) in [("actions", 4), ("reports", 8)] {
            assert_eq!(summary[key].as_object().unwrap().len(), expected_count);
        }
        assert!(
            summary["actions"]
                .as_object()
                .unwrap()
                .values()
                .all(|count| count == 0)
        );
        for (name, count) in summary["reports"].as_object().unwrap() {
            if name != "rejections_by_reason" {
                assert_eq!(count, 0);
            }
        }
        let reasons = summary["reports"]["rejections_by_reason"]
            .as_object()
            .unwrap();
        assert_eq!(reasons.len(), 6);
        assert!(reasons.values().all(|count| count == 0));
        assert_eq!(
            summary["fills"],
            json!({"count": 0, "bought_qty": "0", "sold_qty": "0"})
        );
    }

    #[test]
    fn filled_size_overflow_is_an_error() {
        let trace = ReplayTrace {
            order_events: vec![
                fill(Side::Buy, Qty::from_e8(i64::MAX)),
                fill(Side::Buy, Qty::from_e8(1)),
            ],
            ..ReplayTrace::default()
        };
        let mut summary = JsonObject::new([]);
        let error = add_results(&mut summary, &recording(), &trace, 0, Duration::ZERO).unwrap_err();
        assert!(error.contains("filled size"));
        assert_eq!(summary.to_string(), "{}");
    }

    #[test]
    fn report_waits_subtract_the_configured_latency_and_use_nearest_rank() {
        let mut trace = ReplayTrace::default();
        for (index, wait) in [99, 0, 7, 6, 5, 4, 3, 2, 1, 0].into_iter().enumerate() {
            let mut ack = report(OrderEventKind::Accepted {
                oid: OrderId(index as u64),
            });
            ack.ts_local = Timestamp::from_nanos(200);
            ack.ts_exch = Timestamp::from_nanos(190 - wait);
            trace.order_events.push(ack);
        }
        let mut execution = fill(Side::Buy, "1".parse().unwrap());
        execution.ts_exch = Timestamp::from_nanos(175);
        execution.ts_local = Timestamp::from_nanos(200);
        trace.order_events.push(execution);
        let mut summary = JsonObject::new([]);
        add_results(
            &mut summary,
            &recording(),
            &trace,
            0,
            Duration::from_nanos(10),
        )
        .unwrap();
        let summary: Value = serde_json::from_str(&summary.to_string()).unwrap();

        // Each ack waits 200-(X+10) ns. Sorted: 0,0,1,2,3,4,5,6,7,99.
        // Of ten acks, eight wait. Ranks ceil(10*q) = 5,9,10 select 3,7,99.
        // The single fill waits 200-(175+10)=15 ns at every rank.
        assert_eq!(
            summary["report_wait_ns"],
            json!({
                "action": {"count": 10, "delayed": 8, "p50": 3, "p90": 7, "p99": 99, "max": 99},
                "fill": {"count": 1, "delayed": 1, "p50": 15, "p90": 15, "p99": 15, "max": 15}
            })
        );
    }

    #[test]
    fn ordered_objects_escape_strings_and_preserve_nested_order() {
        let mut object = JsonObject::new([("second", json!("a\"b\\c")), ("first", json!(2))]);
        object.append_object(
            "nested",
            JsonObject::new([("b", json!(0)), ("a", json!(1))]),
        );
        assert_eq!(
            object.to_string(),
            r#"{"second":"a\"b\\c","first":2,"nested":{"b":0,"a":1}}"#
        );
        let parsed: Value = serde_json::from_str(&object.to_string()).unwrap();
        assert_eq!(parsed["second"], "a\"b\\c");
    }
}
