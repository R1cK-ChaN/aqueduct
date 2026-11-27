//! The recording driver selects frames, applies the trade cutoff and sets delivery time.

use hl_codec::hyperliquid::{Network, instrument_id};
use hl_codec::rawlog::{Record, RecordKind};
use hl_run::recording::{GapCause, GapCounts, ReaderStats, RecordConverter};
use hl_types::{Bbo, Level, MarketEvent, MarketEventKind, Side, Timestamp, Trade, TradeId};
use serde_json::{Value, json};

fn record(kind: RecordKind, wall_ms: i64, mono_ms: u64, payload: Value) -> Record {
    Record {
        kind,
        recv_wall_ns: Timestamp::from_millis(wall_ms).as_nanos(),
        recv_mono_ns: mono_ms * 1_000_000,
        payload: serde_json::to_vec(&payload).expect("encode synthetic record"),
    }
}

fn trade(coin: &str, time: u64, tid: u64) -> Value {
    json!({
        "coin": coin, "time": time, "tid": tid, "side": "A", "px": "98", "sz": "0.5",
        "hash": format!("0x{}", "00".repeat(32)),
        "users": [format!("0x{}", "01".repeat(20)), format!("0x{}", "02".repeat(20))]
    })
}

fn trades(wall_ms: i64, mono_ms: u64, rows: &[Value]) -> Record {
    record(
        RecordKind::RecvText,
        wall_ms,
        mono_ms,
        json!({"channel": "trades", "data": rows}),
    )
}

#[test]
fn checkpoint_raw_time_includes_a_frame_with_trades_on_both_sides_of_midnight() {
    const MIDNIGHT_MS: i64 = 86_400_000;
    let mut reader = RecordConverter::new(Network::Testnet, "BTC");
    let mut events = Vec::new();
    let anchor = Timestamp::from_millis(MIDNIGHT_MS - 10);
    reader
        .file_start(
            anchor.as_nanos(),
            0,
            Some(anchor.as_nanos()),
            false,
            &mut events,
        )
        .unwrap();
    assert_eq!(reader.checkpoint_record_time(&events), Some(anchor));

    let frame = trades(
        MIDNIGHT_MS - 1,
        8,
        &[trade("BTC", 86_399_998, 1), trade("BTC", 86_400_000, 2)],
    );
    reader.record(&frame, &mut events).unwrap();
    let before = Timestamp::from_millis(MIDNIGHT_MS - 1);
    let midnight = Timestamp::from_millis(MIDNIGHT_MS);
    assert_eq!([events[0].ts_local, events[1].ts_local], [before, midnight]);
    assert_eq!(reader.checkpoint_record_time(&events), Some(before));
    assert_eq!(reader.stop_time(), Some(midnight));
    events.clear();
    let control = record(RecordKind::SentText, MIDNIGHT_MS + 2, 9, json!({}));
    reader.record(&control, &mut events).unwrap();
    assert_eq!(
        reader.checkpoint_record_time(&events),
        Some(Timestamp::from_millis(MIDNIGHT_MS + 2))
    );
}

#[test]
fn mixed_trades_keep_frame_order_and_follow_the_connection_cutoff() {
    let mut reader = RecordConverter::new(Network::Testnet, "BTC");
    let mut events = Vec::new();
    reader
        .file_start(0, 0, Some(0), false, &mut events)
        .expect("start file");
    for input in [
        trades(8, 8, &[trade("BTC", 7, 1)]),
        record(RecordKind::Meta, 10, 11, json!({"event": "connected"})),
        trades(
            11,
            13,
            &[
                trade("BTC", 9, 2),
                trade("BTC", 12, 8),
                trade("btc", 9, 3),
                trade("ETH", 11, 4),
                trade("BTC", 10, 5),
            ],
        ),
        record(RecordKind::Meta, 15, 15, json!({"event": "disconnected"})),
        trades(16, 16, &[trade("BTC", 16, 9)]),
    ] {
        reader.record(&input, &mut events).expect("convert record");
    }

    let instrument = instrument_id(Network::Testnet, "BTC");
    let expected_trade = |tid, exchange_ms, local_ms| MarketEvent {
        instrument,
        ts_exch: Timestamp::from_millis(exchange_ms),
        ts_local: Timestamp::from_millis(local_ms),
        kind: MarketEventKind::Trade(Trade {
            tid: TradeId(tid),
            px: "98".parse().expect("valid price"),
            qty: "0.5".parse().expect("valid size"),
            aggressor: Side::Sell,
        }),
    };
    assert_eq!(
        events,
        [
            expected_trade(1, 7, 8),
            expected_trade(8, 12, 13),
            expected_trade(5, 10, 13),
            MarketEvent {
                instrument,
                ts_exch: Timestamp::from_millis(15),
                ts_local: Timestamp::from_millis(15),
                kind: MarketEventKind::FeedGap,
            },
        ]
    );
    assert_eq!(
        reader.stats(),
        &ReaderStats {
            trades_live: 3,
            trades_replayed: 2,
            feed_gaps: 1,
            gaps: GapCounts {
                disconnected: 1,
                ..GapCounts::default()
            },
            negative_latency: 1,
            exchange_time_out_of_order: 1,
            process_clock_raised: 2,
            process_clock_max_raise_ns: 2_000_000,
            ..ReaderStats::default()
        }
    );
}

#[test]
fn process_changes_and_truncation_suppress_trades_until_reconnect() {
    for (cause, anchor_ns, mono_ns) in [
        (GapCause::ProcessChange, 1_000_000, 0),
        (GapCause::Truncated, 0, 1_000_000),
    ] {
        let mut reader = RecordConverter::new(Network::Testnet, "BTC");
        let mut events = Vec::new();
        reader
            .file_start(0, 0, Some(0), false, &mut events)
            .unwrap();
        if cause == GapCause::Truncated {
            reader.truncated(&mut events).unwrap();
        }
        reader
            .file_start(
                1_000_000,
                mono_ns,
                Some(anchor_ns),
                cause == GapCause::ProcessChange,
                &mut events,
            )
            .unwrap();
        assert_eq!(reader.origins()[0].gap_cause, Some(cause));
        events.clear();
        reader
            .record(&trades(2, 1, &[trade("BTC", 2, 1)]), &mut events)
            .unwrap();
        assert!(events.is_empty(), "{cause:?}");
        for input in [
            record(RecordKind::Meta, 3, 2, json!({"event": "connected"})),
            trades(4, 3, &[trade("BTC", 3, 2)]),
        ] {
            reader.record(&input, &mut events).unwrap();
        }
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0].kind, MarketEventKind::Trade(trade) if trade.tid == TradeId(2))
        );
        assert_eq!(reader.stats().trades_replayed, 1);
        assert_eq!(reader.stats().trades_live, 1);
    }
}

#[test]
fn other_bbos_and_books_leave_counters_unchanged_and_advance_the_stop() {
    let mut reader = RecordConverter::new(Network::Mainnet, "BTC");
    let mut events = Vec::new();
    reader
        .file_start(0, 0, Some(0), false, &mut events)
        .expect("start file");
    let bbo = |coin| {
        json!({"channel": "bbo", "data": {
            "coin": coin, "time": 4, "bbo": [null, {"px": "103", "sz": "2", "n": 3}]
        }})
    };
    reader
        .record(&record(RecordKind::RecvText, 5, 5, bbo("BTC")), &mut events)
        .expect("convert selected BBO");
    let malformed = json!({"channel": "bbo", "data": {}});
    reader
        .record(&record(RecordKind::RecvText, 6, 6, malformed), &mut events)
        .expect("count decode error");
    let stats = ReaderStats {
        bbo: 1,
        decode_errors: 1,
        ..ReaderStats::default()
    };
    assert_eq!(reader.stats(), &stats);

    let book =
        json!({"channel": "l2Book", "data": {"coin": "BTC", "time": 30, "levels": [[], []]}});
    let diff = json!({"channel": "l2", "data": {"u": {"c": "BTC", "t": 31}}});
    for (wall, mono, frame) in [(7, 7, bbo("btc")), (8, 8, book), (9, 12, diff)] {
        reader
            .record(
                &record(RecordKind::RecvText, wall, mono, frame),
                &mut events,
            )
            .expect("convert ignored frame");
        assert_eq!(reader.stats(), &stats);
    }
    assert_eq!(reader.stop_time(), Some(Timestamp::from_millis(12)));
    assert_eq!(
        events,
        [MarketEvent {
            instrument: instrument_id(Network::Mainnet, "BTC"),
            ts_exch: Timestamp::from_millis(4),
            ts_local: Timestamp::from_millis(5),
            kind: MarketEventKind::Bbo(Bbo {
                bid: None,
                ask: Some(Level {
                    px: "103".parse().expect("valid price"),
                    qty: "2".parse().expect("valid size"),
                    orders: 3,
                }),
            }),
        }]
    );
}
