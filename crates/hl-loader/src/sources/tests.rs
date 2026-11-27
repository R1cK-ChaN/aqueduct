//! Existing fixtures are kept in `crates/hl-codec/tests/fixtures`; subscription
//! overlap and carried-state regressions use synthetic records.

use std::io::Cursor;

use hl_codec::rawlog::{Record, write_record};

use super::*;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../hl-codec/tests/fixtures/");

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("{FIXTURES}{name}")).unwrap()
}

fn e8(text: &str) -> i64 {
    text.parse::<hl_types::Px>().unwrap().as_e8()
}

fn id(coin: &str) -> u32 {
    instrument_id(Network::Mainnet, coin).get()
}

#[test]
fn input_kinds_have_names_and_sources() {
    assert_eq!(
        InputKind::from_name("archive-l2"),
        Some(InputKind::ArchiveL2)
    );
    assert_eq!(
        InputKind::from_name("node-fills"),
        Some(InputKind::NodeFills)
    );
    assert_eq!(InputKind::from_name("rawlog"), Some(InputKind::Rawlog));
    assert_eq!(InputKind::from_name("l2"), None);
    assert_eq!(InputKind::NodeFills.source(), "node_fills");
}

#[test]
fn archive_l2_lines_become_book_rows() {
    let batch = read_archive_l2(Cursor::new(fixture("archive_l2_jup_20260601.jsonl")), 7).unwrap();
    assert_eq!(batch.l2.len(), 4);
    assert_eq!(batch.rows(), 4);

    let first = &batch.l2[0];
    assert_eq!(first.instrument_id, id("JUP"));
    assert_eq!(first.source, "s3_archive");
    assert_eq!(first.ts_exch, 1_780_304_398_828);
    assert_eq!(first.ts_node, 1_780_304_403_736_457_346);
    assert_eq!(first.ts_recv, 0);
    assert_eq!((first.bid_px.len(), first.ask_px.len()), (20, 20));
    assert_eq!(
        (first.bid_px[0], first.bid_sz[0], first.bid_n[0]),
        (e8("0.18926"), e8("506"), 1)
    );
    assert_eq!(
        (first.ask_px[1], first.ask_sz[1], first.ask_n[1]),
        (e8("0.18932"), e8("3032"), 2)
    );
    assert_eq!(first.ingest_id, 7);

    assert_eq!(batch.ts_range, Some((1_780_304_398_828, 1_780_304_400_507)));
    let expected_key = ("hyperliquid-mainnet".to_owned(), "JUP".to_owned());
    assert_eq!(
        batch.instruments.keys().collect::<Vec<_>>(),
        [&expected_key]
    );
}

#[test]
fn a_bad_archive_line_names_its_line_number() {
    let text = String::from_utf8(fixture("archive_l2_jup_20260601.jsonl")).unwrap();
    let broken = text.replacen("\"ver_num\":1", "\"ver_num\":9", 3).replacen(
        "\"ver_num\":9",
        "\"ver_num\":1",
        2,
    );
    let error = read_archive_l2(Cursor::new(broken), 1).unwrap_err();
    assert!(
        matches!(error, SourceError::Line { line: 3, .. }),
        "{error}"
    );
}

#[test]
fn node_fill_blocks_become_one_row_per_party() {
    let batch = read_node_fills(Cursor::new(fixture("archive_fills_20260601.jsonl")), 9).unwrap();
    assert_eq!(batch.fills.len(), 16);
    assert_eq!(batch.counters["blocks"], 11);
    assert_eq!(batch.counters["spot_rows_skipped"], 2);
    assert_eq!(batch.counters["outcome_rows_skipped"], 2);
    // The fixture is a selection of blocks, so heights jump between lines.
    assert_eq!(batch.counters["block_gaps"], 10);
    assert_eq!(batch.problems.len(), MAX_PROBLEMS);

    let maker = batch
        .fills
        .iter()
        .find(|row| row.tid == 773_594_879_104_157 && row.side == 1)
        .unwrap();
    assert_eq!(maker.instrument_id, id("BTC"));
    assert_eq!(
        (maker.ts_exch, maker.block_number),
        (1_780_305_110_625, 1_018_632_945)
    );
    assert_eq!(maker.ts_node, 1_780_305_110_756_632_935);
    assert_eq!(maker.user[..3], [0x6b, 0x13, 0xde]);
    assert!(!maker.crossed);
    assert_eq!(
        (maker.px, maker.sz, maker.start_pos),
        (e8("72999"), e8("0.195"), e8("0.01345"))
    );
    assert_eq!(
        (maker.fee, maker.fee_token.as_str(), maker.dir.as_str()),
        (e8("2.13522"), "USDC", "Open Long")
    );
    assert_eq!(
        (maker.cloid, maker.builder, maker.twap_id),
        ([0; 16], [0; 20], 0)
    );
    assert_eq!(
        (maker.builder_fee, maker.deployer_fee, maker.priority_gas),
        (0, 0, 0)
    );
    assert!(!maker.liquidation && maker.liquidation_method.is_empty());
    assert_eq!((maker.source.as_str(), maker.ingest_id), ("node_fills", 9));

    let liquidated = batch
        .fills
        .iter()
        .find(|row| row.dir == "Close Long" && row.liquidation)
        .unwrap();
    assert_eq!(liquidated.instrument_id, id("xyz:INTC"));
    assert_eq!(liquidated.liquidated_user, liquidated.user);
    assert_eq!(liquidated.liquidation_mark_px, e8("109.07"));
    assert_eq!(liquidated.liquidation_method, "market");
    assert_eq!(liquidated.deployer_fee, e8("0.004466"));

    let with_cloid = batch
        .fills
        .iter()
        .find(|row| row.priority_gas != 0)
        .unwrap();
    assert_eq!(with_cloid.cloid[..2], [0xac, 0x09]);
    assert_eq!(with_cloid.priority_gas, e8("0.00013325"));

    let coins: Vec<&str> = batch
        .instruments
        .keys()
        .map(|(_, coin)| coin.as_str())
        .collect();
    assert_eq!(
        coins,
        ["BTC", "CRV", "LIT", "PENDLE", "TRX", "ZRO", "xyz:INTC"]
    );
}

fn fixture_records() -> Vec<Record> {
    RecordReader::new(Cursor::new(fixture("hl_ws_pons_20261124.hlrec")))
        .map(Result::unwrap)
        .collect()
}

fn encode(records: &[Record]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for record in records {
        write_record(
            &mut bytes,
            record.kind,
            record.recv_wall_ns,
            record.recv_mono_ns,
            &record.payload,
        )
        .unwrap();
    }
    bytes
}

#[test]
fn a_recording_becomes_books_bbo_and_trades() {
    let batch = read_rawlog(Cursor::new(fixture("hl_ws_pons_20261124.hlrec")), 3).unwrap();

    // One snapshot and 107 diffs, each giving a full book.
    assert_eq!(batch.l2.len(), 108);
    assert!(
        batch
            .l2
            .iter()
            .all(|row| row.bid_px.len() == 20 && row.ask_px.len() == 20)
    );
    assert!(
        batch
            .l2
            .iter()
            .all(|row| row.source == "ws_public" && row.ts_node == 0 && row.ts_recv > 0)
    );
    assert!(
        batch
            .l2
            .windows(2)
            .all(|pair| pair[0].ts_exch < pair[1].ts_exch)
    );
    assert!(
        batch.l2[0].bid_n.iter().all(|orders| *orders > 0),
        "the snapshot has order counts"
    );
    assert!(
        batch.l2[1..].iter().any(|row| row.bid_n.contains(&0)),
        "diff levels do not"
    );

    assert_eq!(batch.bbo.len(), 259);
    assert!(
        batch
            .bbo
            .iter()
            .all(|row| row.instrument_id == id("PONS") && row.bid_px > 0)
    );

    let replayed = batch.counters.get("trades_replayed").copied().unwrap_or(0);
    let live = batch.counters.get("trades_live").copied().unwrap_or(0);
    assert_eq!(replayed + live, batch.ws_trades.len() as u64);
    assert!(
        replayed > 0,
        "the first trades frame replays trades from before the connection"
    );
    assert!(live > 0);
    assert!(
        batch
            .ws_trades
            .iter()
            .all(|row| row.buyer != row.seller && row.px > 0)
    );

    assert_eq!(batch.counters["frames_not_loaded:activeAssetCtx"], 2);
    assert_eq!(batch.counters["frames_not_loaded:fastAssetCtxs"], 1);
    for counter in [
        "decode_errors",
        "book_errors",
        "diffs_without_snapshot",
        "truncated",
    ] {
        assert!(!batch.counters.contains_key(counter), "{counter}");
    }
    assert_eq!(batch.instruments.len(), 1);
}

#[test]
fn a_disconnect_drops_the_book_until_the_next_snapshot() {
    let mut records = fixture_records();
    let snapshot = records
        .iter()
        .position(|record| {
            record.kind == RecordKind::RecvText
                && record
                    .payload
                    .starts_with(br#"{"channel":"l2","data":{"s""#)
        })
        .unwrap();
    let at = records[snapshot].clone();
    records.insert(
        snapshot + 1,
        Record {
            kind: RecordKind::Meta,
            payload: br#"{"event":"disconnected","reason":"test"}"#.to_vec(),
            ..at
        },
    );

    let batch = read_rawlog(Cursor::new(encode(&records)), 1).unwrap();
    assert_eq!(batch.l2.len(), 1, "only the snapshot before the disconnect");
    assert_eq!(batch.counters["diffs_without_snapshot"], 107);
    assert_eq!(batch.counters["disconnects"], 1);
}

#[test]
fn a_truncated_recording_is_read_up_to_the_cut() {
    // Compress the way the recorder does: flushing every so often, so that
    // the stream holds blocks that can be decoded before the cut.
    let mut encoder = zstd::Encoder::new(Vec::new(), 3).unwrap();
    for chunk in fixture_records().chunks(50) {
        std::io::Write::write_all(&mut encoder, &encode(chunk)).unwrap();
        std::io::Write::flush(&mut encoder).unwrap();
    }
    let packed = encoder.finish().unwrap();
    let cut = &packed[..packed.len() / 2];
    let decoder = zstd::Decoder::new(Cursor::new(cut.to_vec())).unwrap();

    let batch = read_rawlog(decoder, 1).unwrap();
    assert_eq!(batch.counters["truncated"], 1);
    assert!(!batch.bbo.is_empty() && batch.bbo.len() < 259);
}

#[test]
fn recordings_of_other_venues_are_not_loaded() {
    let mut records = fixture_records();
    let open = records[0].payload.clone();
    let text = String::from_utf8(open).unwrap();
    assert!(text.contains("\"file_open\""));
    records[0].payload = text
        .replace(
            "wss://api.hyperliquid.xyz/ws",
            "wss://fstream.binance.com/public/ws",
        )
        .into_bytes();

    let error = read_rawlog(Cursor::new(encode(&records)), 1).unwrap_err();
    assert!(matches!(error, SourceError::Unsupported(_)), "{error}");
}

#[test]
fn a_recording_must_start_with_file_open() {
    let records = fixture_records();
    let without_header: Vec<Record> = records
        .into_iter()
        .filter(|record| record.kind != RecordKind::Meta)
        .collect();
    let error = read_rawlog(Cursor::new(encode(&without_header)), 1).unwrap_err();
    assert!(matches!(error, SourceError::Invalid(_)), "{error}");
}

#[test]
fn undecodable_frames_are_counted_not_fatal() {
    let mut records = fixture_records();
    let bbo = records
        .iter()
        .position(|record| record.payload.starts_with(br#"{"channel":"bbo""#))
        .unwrap();
    records[bbo].payload = br#"{"channel":"bbo","data":{"coin":"PONS"}}"#.to_vec();

    let batch = read_rawlog(Cursor::new(encode(&records)), 1).unwrap();
    assert_eq!(batch.counters["decode_errors"], 1);
    assert_eq!(batch.bbo.len(), 258);
    assert_eq!(batch.problems.len(), 1);
}

fn header_with_sequence(original: &Record, sequence: Option<u64>) -> Record {
    let mut header: Value = serde_json::from_slice(&original.payload).unwrap();
    match sequence {
        Some(sequence) => header["file_seq"] = sequence.into(),
        None => {
            header.as_object_mut().unwrap().remove("file_seq");
        }
    }
    Record {
        payload: serde_json::to_vec(&header).unwrap(),
        ..original.clone()
    }
}

fn decode_segments(records: &[Record], cuts: &[usize], numbered: bool) -> Batch {
    let boundaries: Vec<_> = [1]
        .into_iter()
        .chain(cuts.iter().copied())
        .chain([records.len()])
        .collect();
    let mut combined = Batch::default();
    let mut state = None;
    for (index, range) in boundaries.windows(2).enumerate() {
        let sequence = numbered.then_some(index as u64);
        let mut header = header_with_sequence(&records[0], sequence);
        header.recv_wall_ns = records[range[0]].recv_wall_ns;
        let mut part = vec![header];
        part.extend_from_slice(&records[range[0]..range[1]]);
        let (batch, end) =
            read_rawlog_with_state(Cursor::new(encode(&part)), index as u64, state).unwrap();
        state = Some(RawlogState::from_json(&end.to_json().unwrap()).unwrap());
        combined.l2.extend(batch.l2);
        combined.bbo.extend(batch.bbo);
        combined.ws_trades.extend(batch.ws_trades);
        combined.instruments.extend(batch.instruments);
        for (counter, count) in batch.counters {
            *combined.counters.entry(counter).or_default() += count;
        }
    }
    combined
}

fn assert_same_stream(mut actual: Batch, mut expected: Batch) {
    for batch in [&mut actual, &mut expected] {
        for row in &mut batch.l2 {
            row.ingest_id = 0;
        }
        for row in &mut batch.bbo {
            row.ingest_id = 0;
        }
        for row in &mut batch.ws_trades {
            row.ingest_id = 0;
        }
    }
    assert_eq!(actual.l2, expected.l2);
    assert_eq!(actual.bbo, expected.bbo);
    assert_eq!(actual.ws_trades, expected.ws_trades);
    assert_eq!(actual.instruments, expected.instruments);
    assert_eq!(actual.counters, expected.counters);
}

#[test]
fn numbered_and_legacy_rotations_preserve_every_decoded_row() {
    let records = fixture_records();
    let cuts = [records.len() / 3, records.len() * 2 / 3];
    assert!(records[cuts[0]..].iter().all(|record| {
        record.kind != RecordKind::Meta
            || meta_event(&record.payload).unwrap()["event"] != "connected"
    }));
    for numbered in [true, false] {
        let split = decode_segments(&records, &cuts, numbered);
        let whole = read_rawlog(Cursor::new(encode(&records)), 99).unwrap();
        assert_same_stream(split, whole);
    }

    let mut later = vec![header_with_sequence(&records[0], None)];
    later.extend_from_slice(&records[cuts[0]..cuts[1]]);
    let isolated = read_rawlog(Cursor::new(encode(&later)), 1).unwrap();
    assert!(isolated.l2.is_empty());
    assert!(isolated.counters["diffs_without_snapshot"] > 0);
    assert!(!isolated.ws_trades.is_empty());
    assert!(isolated.ws_trades.iter().all(|trade| trade.live == 1));
}

fn synthetic_record(kind: RecordKind, millis: i64, payload: Value) -> Record {
    Record {
        kind,
        recv_wall_ns: millis * 1_000_000,
        recv_mono_ns: 0,
        payload: serde_json::to_vec(&payload).unwrap(),
    }
}

fn synthetic_header(millis: i64) -> Record {
    synthetic_record(
        RecordKind::Meta,
        millis,
        serde_json::json!({
            "event": "file_open", "url": "wss://api.hyperliquid.xyz/ws",
            "source": "hl", "process_start_wall_ns": 7, "file_seq": 0
        }),
    )
}

fn synthetic_snapshot(millis: i64, bid: &str, ask: &str) -> Record {
    synthetic_record(
        RecordKind::RecvText,
        millis,
        serde_json::json!({"channel": "l2", "data": {"s": {
            "coin": "BTC", "time": millis,
            "levels": [
                [{"px": bid, "sz": "2", "n": 3}],
                [{"px": ask, "sz": "4", "n": 5}]
            ]
        }}}),
    )
}

fn synthetic_diff(millis: i64) -> Record {
    synthetic_record(
        RecordKind::RecvText,
        millis,
        serde_json::json!({"channel": "l2", "data": {"u": {
            "c": "BTC", "t": millis, "l": [[], []], "r": [[], []]
        }}}),
    )
}

fn synthetic_trade(received: i64, traded: i64) -> Record {
    synthetic_record(
        RecordKind::RecvText,
        received,
        serde_json::json!({"channel": "trades", "data": [{
            "coin": "BTC", "side": "B", "px": "100", "sz": "1",
            "time": traded, "tid": received, "hash": format!("0x{}", "00".repeat(32)),
            "users": [format!("0x{}", "01".repeat(20)), format!("0x{}", "02".repeat(20))]
        }]}),
    )
}

#[test]
fn known_connection_keeps_the_previous_trade_rows_and_counts() {
    let records = [
        synthetic_header(1_000),
        synthetic_record(
            RecordKind::Meta,
            1_000,
            serde_json::json!({"event": "connected"}),
        ),
        synthetic_snapshot(1_001, "100", "101"),
        synthetic_trade(1_002, 999),
        synthetic_trade(1_003, 1_000),
    ];
    let batch = read_rawlog(encode(&records).as_slice(), 7).unwrap();
    // Complete synthetic trade rows from the earlier loader.
    let snapshot_trade = WsTradeRow {
        instrument_id: 1_637_510_701,
        source: "ws_public".to_owned(),
        ts_exch: 999,
        ts_recv: 1_002_000_000,
        tid: 1_002,
        aggressor: 1,
        px: 10_000_000_000,
        sz: 100_000_000,
        buyer: [1; 20],
        seller: [2; 20],
        hash: [0; 32],
        live: 0,
        ingest_id: 7,
    };
    let live_trade = WsTradeRow {
        ts_exch: 1_000,
        ts_recv: 1_003_000_000,
        tid: 1_003,
        live: 1,
        ..snapshot_trade.clone()
    };
    assert_eq!(batch.ws_trades, [snapshot_trade, live_trade]);
    assert_eq!(
        batch.counters,
        BTreeMap::from([
            ("trades_live".to_owned(), 1),
            ("trades_replayed".to_owned(), 1),
        ])
    );
}

#[test]
fn unknown_start_trades_stay_live_across_serialized_rotations() {
    let records = [
        synthetic_header(1_000),
        synthetic_trade(1_001, 990),
        synthetic_record(
            RecordKind::Meta,
            1_002,
            serde_json::json!({"event": "dropped", "count": 1}),
        ),
        synthetic_trade(1_003, 995),
        synthetic_record(
            RecordKind::Meta,
            1_004,
            serde_json::json!({"event": "connected"}),
        ),
        synthetic_trade(1_005, 1_003),
        synthetic_trade(1_006, 1_004),
    ];
    let whole = read_rawlog(encode(&records).as_slice(), 7).unwrap();
    let flags: Vec<_> = whole
        .ws_trades
        .iter()
        .map(|row| (row.tid, row.live))
        .collect();
    assert_eq!(flags, [(1_001, 1), (1_003, 1), (1_005, 0), (1_006, 1)]);
    assert_eq!(whole.counters["trades_live"], 3);
    assert_eq!(whole.counters["trades_replayed"], 1);
    assert_eq!(whole.counters["frames_dropped_by_recorder"], 1);
    for numbered in [true, false] {
        let split = decode_segments(&records, &[2, 4, 6], numbered);
        let whole = read_rawlog(encode(&records).as_slice(), 7).unwrap();
        assert_same_stream(split, whole);
    }
}

#[test]
fn previous_loader_state_bytes_restore_the_cutoff_and_book() {
    // The earlier loader wrote these version 2 strings from synthetic records.
    let connected = concat!(
        r#"{"version":2,"network":"hyperliquid-mainnet","connected_at":1000000000,"books":{"#,
        r#""1637510701":{"synced":true,"snapshot_time_ns":1001000000,"#,
        r#""bids":[[10000000000,200000000,3]],"asks":[[10100000000,400000000,5]]}}}"#,
    );
    let disconnected =
        r#"{"version":2,"network":"hyperliquid-mainnet","connected_at":null,"books":{}}"#;
    let records = [
        synthetic_header(1_002),
        synthetic_trade(1_003, 999),
        synthetic_trade(1_004, 1_000),
        synthetic_diff(1_005),
    ];
    for (old_bytes, live, book_rows) in [(connected, 1, 1), (disconnected, 0, 0)] {
        let state = RawlogState::from_json(old_bytes).unwrap();
        assert_eq!(state.to_json().unwrap(), old_bytes);
        let (batch, _) =
            read_rawlog_with_state(encode(&records).as_slice(), 7, Some(state)).unwrap();
        assert_eq!(
            (batch.ws_trades[0].live, batch.ws_trades[1].live),
            (0, live)
        );
        assert_eq!(batch.l2.len(), book_rows);
    }
}

#[test]
fn truncated_tail_returns_the_cutoff_at_the_last_complete_record() {
    let mut bytes = encode(&[
        synthetic_header(1_000),
        synthetic_record(
            RecordKind::Meta,
            1_000,
            serde_json::json!({"event": "connected"}),
        ),
    ]);
    bytes.push(1);
    let (batch, state) = read_rawlog_with_state(bytes.as_slice(), 7, None).unwrap();
    assert_eq!(batch.counters["truncated"], 1);
    assert!(!state.cutoff.is_live(Timestamp::from_millis(999)));
    assert!(state.cutoff.is_live(Timestamp::from_millis(1_000)));
}

fn mon_subscription_records() -> Vec<Record> {
    let bids: Vec<_> = (81..=100)
        .rev()
        .map(|price| serde_json::json!({"px": price.to_string(), "sz": "2", "n": 3}))
        .collect();
    let asks: Vec<_> = (101..=120)
        .map(|price| serde_json::json!({"px": price.to_string(), "sz": "4", "n": 5}))
        .collect();
    vec![
        synthetic_header(1_000),
        synthetic_record(
            RecordKind::Meta,
            1_000,
            serde_json::json!({"event": "connected"}),
        ),
        synthetic_record(
            RecordKind::RecvText,
            1_001,
            serde_json::json!({"channel": "l2", "data": {"s": {
                "coin": "MON", "time": 1_001, "levels": [bids, asks]
            }}}),
        ),
        // This diff was included in the snapshot, even though it arrived later.
        synthetic_record(
            RecordKind::RecvText,
            1_002,
            serde_json::json!({"channel": "l2", "data": {"u": {
                "c": "MON", "t": 1_001, "l": [[], []], "r": [[0], [0]]
            }}}),
        ),
        // Position 19 only exists if the preceding stale diff was skipped.
        synthetic_record(
            RecordKind::RecvText,
            1_003,
            serde_json::json!({"channel": "l2", "data": {"u": {
                "c": "MON", "t": 1_003,
                "l": [[{"p": "80", "s": "6"}], [{"p": "121", "s": "8"}]],
                "r": [[19], [19]]
            }}}),
        ),
        synthetic_record(
            RecordKind::RecvText,
            1_004,
            serde_json::json!({"channel": "l2", "data": {"u": {
                "c": "MON", "t": 1_004,
                "l": [[{"p": "79", "s": "10"}], [{"p": "122", "s": "12"}]],
                "r": [[19], [19]]
            }}}),
        ),
    ]
}

#[test]
fn same_time_subscription_diff_is_skipped_before_later_positional_diffs() {
    let records = mon_subscription_records();
    let (batch, state) = read_rawlog_with_state(Cursor::new(encode(&records)), 7, None).unwrap();
    assert_eq!(batch.l2.len(), 3);
    assert_eq!(batch.counters["stale_diffs_skipped"], 1);
    for counter in ["book_errors", "diffs_without_snapshot", "decode_errors"] {
        assert_eq!(batch.counters.get(counter).copied().unwrap_or(0), 0);
    }
    assert!(batch.problems.is_empty());
    assert_eq!(batch.ts_range, Some((1_001, 1_004)));

    for (row, (time, last_bid, last_ask)) in batch.l2.iter().zip([
        (1_001, "81", "120"),
        (1_003, "80", "121"),
        (1_004, "79", "122"),
    ]) {
        assert_eq!(row.instrument_id, id("MON"));
        assert_eq!((row.ts_exch, row.ts_recv), (time, time * 1_000_000));
        assert_eq!((row.bid_px.len(), row.ask_px.len()), (20, 20));
        assert_eq!((row.bid_px[0], row.ask_px[0]), (e8("100"), e8("101")));
        assert_eq!(
            (row.bid_px[19], row.ask_px[19]),
            (e8(last_bid), e8(last_ask))
        );
    }
    assert!(state.books[&id("MON")].is_synced());
    assert_eq!(
        state.books[&id("MON")].snapshot_time(),
        Some(Timestamp::from_millis(1_001))
    );
}

#[test]
fn split_between_snapshot_and_stale_diff_preserves_rows_and_counters() {
    let records = mon_subscription_records();
    for numbered in [true, false] {
        let split = decode_segments(&records, &[3], numbered);
        let whole = read_rawlog(Cursor::new(encode(&records)), 99).unwrap();
        assert_same_stream(split, whole);
    }
}

#[test]
fn every_boundary_preserves_disconnect_reconnect_and_fresh_snapshots() {
    let meta = |millis, event| {
        synthetic_record(
            RecordKind::Meta,
            millis,
            serde_json::json!({"event": event}),
        )
    };
    let mut records = vec![
        synthetic_header(1_000),
        meta(1_000, "connected"),
        synthetic_snapshot(1_001, "100", "101"),
        synthetic_trade(1_002, 999),
        synthetic_trade(1_003, 1_000),
        synthetic_diff(1_004),
        meta(1_005, "disconnected"),
        synthetic_diff(1_006),
        synthetic_trade(1_007, 1_007),
        meta(2_000, "connected"),
        synthetic_trade(2_001, 1_999),
        synthetic_trade(2_002, 2_000),
        synthetic_diff(2_003),
        synthetic_snapshot(2_004, "200", "201"),
        synthetic_diff(2_005),
        synthetic_snapshot(2_006, "300", "301"),
        synthetic_diff(2_007),
    ];
    let whole = read_rawlog(Cursor::new(encode(&records)), 99).unwrap();
    assert_eq!(whole.counters["diffs_without_snapshot"], 2);
    assert_eq!(whole.counters["disconnects"], 1);
    assert_eq!(
        whole
            .ws_trades
            .iter()
            .map(|row| row.live)
            .collect::<Vec<_>>(),
        [0, 1, 0, 0, 1]
    );
    assert_eq!(whole.l2.last().unwrap().bid_px, [e8("300")]);
    for cut in 2..records.len() {
        let split = decode_segments(&records, &[cut], true);
        let whole = read_rawlog(Cursor::new(encode(&records)), 99).unwrap();
        assert_same_stream(split, whole);
    }
    records.push(meta(2_008, "shutdown"));
    let (_, end) = read_rawlog_with_state(Cursor::new(encode(&records)), 1, None).unwrap();
    assert_eq!(end.cutoff, TradeCutoff::Disconnected);
    assert!(end.books.is_empty());
}

#[test]
fn stored_state_preserves_exact_integer_levels_and_book_sync() {
    let level = hl_types::Level {
        px: hl_types::Px::from_e8(9_007_199_254_740_993),
        qty: hl_types::Qty::from_e8(i64::MAX),
        orders: u32::MAX,
    };
    let mut state = RawlogState::new(Network::Testnet);
    state.cutoff = TradeCutoff::ConnectedSince(Timestamp::from_nanos(i64::MIN + 1));
    let snapshot_time = Timestamp::from_nanos(9_007_199_254_740_993);
    let mut full = L2Book::new();
    full.apply_snapshot(
        &L2Snapshot {
            bids: vec![level],
            asks: vec![hl_types::Level {
                px: hl_types::Px::from_e8(i64::MAX),
                qty: hl_types::Qty::from_e8(9_007_199_254_740_995),
                orders: 1,
            }],
        },
        snapshot_time,
    )
    .unwrap();
    let mut empty = L2Book::new();
    empty
        .apply_snapshot(
            &L2Snapshot {
                bids: Vec::new(),
                asks: Vec::new(),
            },
            Timestamp::from_nanos(i64::MIN),
        )
        .unwrap();
    state.books.insert(1, full);
    state.books.insert(2, empty);
    state.books.insert(3, L2Book::new());
    let json = state.to_json().unwrap();
    let stored: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(stored["version"], 2);
    assert_eq!(
        stored["books"]["1"]["snapshot_time_ns"],
        snapshot_time.as_nanos()
    );
    assert_eq!(stored["books"]["2"]["snapshot_time_ns"], i64::MIN);
    assert_eq!(stored["books"]["3"]["snapshot_time_ns"], Value::Null);
    let restored = RawlogState::from_json(&json).unwrap();
    assert_eq!(restored.to_json().unwrap(), json);
    assert_eq!(restored.network, Network::Testnet);
    assert_eq!(restored.cutoff, state.cutoff);
    for (&instrument, book) in &state.books {
        assert_eq!(restored.books[&instrument].is_synced(), book.is_synced());
        assert_eq!(restored.books[&instrument].bids(), book.bids());
        assert_eq!(restored.books[&instrument].asks(), book.asks());
        assert_eq!(
            restored.books[&instrument].snapshot_time(),
            book.snapshot_time()
        );
    }
    assert!(
        RawlogState::from_json(&RawlogState::new(Network::Mainnet).to_json().unwrap())
            .unwrap()
            .cutoff
            .is_live(Timestamp::from_nanos(i64::MIN))
    );
}

#[test]
fn legacy_state_applies_diffs_across_restore_until_the_next_snapshot() {
    let stored = serde_json::json!({
        "version": 1, "network": "hyperliquid-mainnet", "connected_at": null,
        "books": {"1": {
            "synced": true, "bids": [[e8("100"), e8("2"), 3]],
            "asks": [[e8("101"), e8("4"), 5]]
        }}
    });
    let legacy = RawlogState::from_json(&stored.to_string()).unwrap();
    assert_eq!(legacy.books[&1].snapshot_time(), None);
    let carried = legacy.to_json().unwrap();
    let carried_json: Value = serde_json::from_str(&carried).unwrap();
    assert_eq!(carried_json["version"], 2);
    assert_eq!(carried_json["books"]["1"]["snapshot_time_ns"], Value::Null);
    let mut restored = RawlogState::from_json(&carried).unwrap();
    let book = restored.books.get_mut(&1).unwrap();
    assert_eq!(book.snapshot_time(), None);
    assert!(book.is_synced());
    let snapshot = book.to_snapshot().unwrap();
    let diff = hl_types::L2Diff {
        remove_bids: vec![0],
        ..Default::default()
    };
    let time = Timestamp::from_millis(1_001);
    assert_eq!(
        book.apply_diff(&diff, time),
        Ok(hl_book::DiffOutcome::Applied)
    );
    assert!(book.bids().is_empty());
    assert_eq!(book.snapshot_time(), None);
    book.apply_snapshot(&snapshot, time).unwrap();
    assert_eq!(
        book.apply_diff(&diff, time),
        Ok(hl_book::DiffOutcome::SkippedStale)
    );
    assert_eq!(book.to_snapshot(), Some(snapshot));
    assert!(book.is_synced());
}

#[test]
fn stored_state_rejects_invalid_versions_networks_and_books() {
    let state = RawlogState::new(Network::Mainnet).to_json().unwrap();
    for (field, value) in [
        ("version", serde_json::json!(0)),
        ("version", serde_json::json!(3)),
        ("network", serde_json::json!("unknown")),
        (
            "books",
            serde_json::json!({"1": {"synced": false, "bids": [[1, 1, 0]], "asks": []}}),
        ),
        (
            "books",
            serde_json::json!({"1": {"synced": true, "bids": [[0, 1, 0]], "asks": []}}),
        ),
        (
            "books",
            serde_json::json!({"1": {"synced": false, "bids": [], "asks": [], "snapshot_time_ns": 1}}),
        ),
        (
            "books",
            serde_json::json!({"1": {"synced": true, "bids": [], "asks": [], "snapshot_time_ns": "1"}}),
        ),
        (
            "books",
            serde_json::json!({"1": {"synced": true, "bids": [], "asks": [], "snapshot_time_ns": 1.5}}),
        ),
    ] {
        let mut json: Value = serde_json::from_str(&state).unwrap();
        json[field] = value;
        assert!(RawlogState::from_json(&json.to_string()).is_err());
    }
}

#[test]
fn header_metadata_supports_legacy_files_and_requires_numbered_chain_identity() {
    let original = fixture_records().remove(0);
    let legacy =
        read_rawlog_metadata(Cursor::new(encode(std::slice::from_ref(&original)))).unwrap();
    assert!(legacy.chain.is_some());
    assert_eq!(legacy.file_seq, None);
    assert_eq!(legacy.opened_wall_ns, original.recv_wall_ns);
    assert_eq!(legacy.opened_mono_ns, original.recv_mono_ns);
    let numbered = header_with_sequence(&original, Some(42));
    assert_eq!(
        read_rawlog_metadata(Cursor::new(encode(std::slice::from_ref(&numbered))))
            .unwrap()
            .file_seq,
        Some(42)
    );
    for field in ["source", "process_start_wall_ns"] {
        let mut json: Value = serde_json::from_slice(&numbered.payload).unwrap();
        json.as_object_mut().unwrap().remove(field);
        let incomplete = Record {
            payload: serde_json::to_vec(&json).unwrap(),
            ..numbered.clone()
        };
        assert!(read_rawlog_metadata(Cursor::new(encode(&[incomplete]))).is_err());
        json.as_object_mut().unwrap().remove("file_seq");
        let legacy = Record {
            payload: serde_json::to_vec(&json).unwrap(),
            ..numbered.clone()
        };
        assert_eq!(
            read_rawlog_metadata(Cursor::new(encode(&[legacy])))
                .unwrap()
                .chain,
            None
        );
    }
    for invalid in [serde_json::json!(-1), Value::Null, serde_json::json!("1")] {
        let mut json: Value = serde_json::from_slice(&numbered.payload).unwrap();
        json["file_seq"] = invalid;
        let bad = Record {
            payload: serde_json::to_vec(&json).unwrap(),
            ..numbered.clone()
        };
        assert!(read_rawlog_metadata(Cursor::new(encode(&[bad]))).is_err());
    }
}

#[test]
fn loader_checks_the_url_before_the_chain_and_ignores_the_format_field() {
    let read = |header| {
        let record = synthetic_record(RecordKind::Meta, 0, header);
        read_rawlog_metadata(encode(&[record]).as_slice())
    };
    let unsupported = read(serde_json::json!({
        "event": "file_open", "url": "ws://example.invalid", "file_seq": 1
    }));
    assert!(matches!(unsupported, Err(SourceError::Unsupported(_))));

    let known_feed = read(serde_json::json!({
        "event": "file_open", "url": "wss://api.hyperliquid.xyz/ws", "format": 2
    }))
    .expect("loader identifies this feed from its URL");
    assert_eq!(known_feed.network, Network::Mainnet);
}

#[test]
fn carried_state_requires_a_header_and_the_same_network() {
    let records = fixture_records();
    assert!(
        read_rawlog_with_state(
            Cursor::new(encode(&records)),
            1,
            Some(RawlogState::new(Network::Testnet))
        )
        .is_err()
    );
    assert!(
        read_rawlog_with_state(
            Cursor::new(encode(&records[1..])),
            1,
            Some(RawlogState::new(Network::Mainnet))
        )
        .is_err()
    );
}
