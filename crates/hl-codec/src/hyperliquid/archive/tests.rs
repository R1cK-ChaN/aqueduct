//! The fixtures are lines taken unchanged from the official archives for
//! 2026-06-01, hour 09 UTC: the first four lines of the JUP L2 file, and
//! eleven small blocks of `node_fills_by_block` chosen to cover every
//! optional field.

use super::super::{Network, instrument_id};
use super::*;

const L2_LINES: &str = include_str!("../../../tests/fixtures/archive_l2_jup_20260601.jsonl");
const FILL_LINES: &str = include_str!("../../../tests/fixtures/archive_fills_20260601.jsonl");

/// 2026-06-01T09:00:00Z in seconds since the epoch.
const NINE_OCLOCK: i64 = 1_780_304_400;

fn at(seconds_after_nine: i64, nanos: i64) -> Timestamp {
    Timestamp::from_nanos((NINE_OCLOCK + seconds_after_nine) * 1_000_000_000 + nanos)
}

fn blocks() -> Vec<FillBlock> {
    FILL_LINES
        .lines()
        .map(|line| decode_fill_block(line.as_bytes()).unwrap())
        .collect()
}

fn block(number: u64) -> FillBlock {
    blocks()
        .into_iter()
        .find(|block| block.block_number == number)
        .unwrap()
}

fn usd_of(text: &str) -> Usd {
    text.parse().unwrap()
}

#[test]
fn converts_civil_dates_to_days() {
    assert_eq!(days_from_civil(1970, 1, 1), 0);
    assert_eq!(days_from_civil(1969, 12, 31), -1);
    assert_eq!(days_from_civil(2000, 2, 29), 11_016);
    assert_eq!(days_from_civil(2000, 3, 1), 11_017);
    assert_eq!(days_from_civil(2026, 6, 1), NINE_OCLOCK / 86_400);
}

#[test]
fn parses_node_times_with_any_fraction_length() {
    let cases = [
        ("2026-06-01T09:00:03.669581779", at(3, 669_581_779)),
        ("2026-06-01T09:00:25.650866", at(25, 650_866_000)),
        ("2026-06-01T09:00:25.5", at(25, 500_000_000)),
        ("2026-06-01T09:00:25", at(25, 0)),
        ("2026-06-01T09:00:25.000000001Z", at(25, 1)),
        ("1970-01-01T00:00:00", Timestamp::EPOCH),
    ];
    for (text, expected) in cases {
        assert_eq!(node_time("time", text), Ok(expected), "{text}");
    }
}

#[test]
fn rejects_malformed_node_times() {
    let cases = [
        "",
        "2026-06-01",
        "2026-06-01 09:00:25",
        "2026/06/01T09:00:25",
        "2026-13-01T09:00:25",
        "2026-06-00T09:00:25",
        "2026-06-01T24:00:25",
        "2026-06-01T09:60:25",
        "2026-06-01T09:00:60",
        "2026-06-01T09:00:25.",
        "2026-06-01T09:00:25.0000000001",
        "2026-06-01T09:00:2x",
        "2026-06-01T09:00:25.12a",
        "2026-06-01T09:00:25+08:00",
        "+026-06-01T09:00:25",
    ];
    for text in cases {
        assert_eq!(
            node_time("time", text),
            Err(DecodeError::Invalid {
                field: "time",
                text: text.to_owned()
            }),
            "{text:?}"
        );
    }
}

#[test]
fn rounds_float_noise_to_eight_decimals() {
    let cases = [
        ("-2471856.2999999998", "-2471856.3"),
        ("-1081829.6000000001", "-1081829.6"),
        ("1195886.5183997401", "1195886.51839974"),
        ("0.000000015", "0.00000002"),
        ("-0.000000015", "-0.00000002"),
        ("0.000000014", "0.00000001"),
        ("0.999999995", "1"),
        ("38373.0", "38373"),
        ("-12.42", "-12.42"),
        ("5", "5"),
    ];
    for (text, expected) in cases {
        assert_eq!(
            rounded_qty("startPosition", text),
            Ok(expected.parse().unwrap()),
            "{text}"
        );
    }
    for text in ["1.12345678x", "abc.123456789", "1.123456789e3", "", "1..5"] {
        assert!(
            matches!(
                rounded_qty("startPosition", text),
                Err(DecodeError::Number {
                    field: "startPosition",
                    ..
                })
            ),
            "{text:?}"
        );
    }
}

#[test]
fn classifies_markets_by_coin_name() {
    for coin in ["BTC", "kPEPE", "xyz:INTC", "mkts:BVIV"] {
        assert_eq!(MarketKind::of(coin), MarketKind::Perp, "{coin}");
    }
    for coin in ["@72", "PURR/USDC"] {
        assert_eq!(MarketKind::of(coin), MarketKind::Spot, "{coin}");
    }
    for coin in ["#1360", "+1361"] {
        assert_eq!(MarketKind::of(coin), MarketKind::Outcome, "{coin}");
    }
}

#[test]
fn decodes_l2_archive_lines() {
    let books: Vec<ArchiveBook> = L2_LINES
        .lines()
        .map(|line| decode_archive_l2_line(line.as_bytes()).unwrap())
        .collect();
    assert_eq!(books.len(), 4);

    let first = &books[0];
    assert_eq!(first.node_time, at(3, 736_457_346));
    assert_eq!(first.book.coin, "JUP");
    assert_eq!(first.book.time, Timestamp::from_millis(1_780_304_398_828));
    assert_eq!((first.book.bids.len(), first.book.asks.len()), (20, 20));
    let level = |price: &str, size: &str, orders| hl_types::Level {
        px: price.parse().unwrap(),
        qty: size.parse().unwrap(),
        orders,
    };
    assert_eq!(
        first.book.bids[..2],
        [level("0.18926", "506", 1), level("0.18925", "1250", 1)]
    );
    assert_eq!(
        first.book.asks[..2],
        [level("0.1893", "1000", 1), level("0.18932", "3032", 2)]
    );
    assert_eq!(first.book.bids[19], level("0.18889", "15741", 1));
    assert_eq!(first.book.asks[19], level("0.18974", "15741", 1));

    let times: Vec<i64> = books
        .iter()
        .map(|book| book.book.time.as_millis())
        .collect();
    assert_eq!(
        times,
        [
            1_780_304_398_828,
            1_780_304_399_372,
            1_780_304_399_901,
            1_780_304_400_507
        ]
    );
    // The node writes lines in batches: here the block time is five seconds
    // older than the node's own clock (HL-99).
    assert!(first.node_time.nanos_since(first.book.time) > 4_000_000_000);
}

#[test]
fn refuses_l2_lines_of_an_unknown_format() {
    let line = L2_LINES.lines().next().unwrap();
    let version = line.replace(r#""ver_num":1"#, r#""ver_num":2"#);
    assert_eq!(
        decode_archive_l2_line(version.as_bytes()),
        Err(DecodeError::Invalid {
            field: "ver_num",
            text: "2".to_owned()
        })
    );
    let channel = line.replace(r#""channel":"l2Book""#, r#""channel":"l4Book""#);
    assert_eq!(
        decode_archive_l2_line(channel.as_bytes()),
        Err(DecodeError::Invalid {
            field: "channel",
            text: "l4Book".to_owned()
        })
    );
    assert!(matches!(
        decode_archive_l2_line(b"{}"),
        Err(DecodeError::Json(_))
    ));
    let time = line.replace("2026-06-01T09", "2026-06-01 09");
    assert!(matches!(
        decode_archive_l2_line(time.as_bytes()),
        Err(DecodeError::Invalid { field: "time", .. })
    ));
}

#[test]
fn decodes_an_empty_block() {
    assert_eq!(
        block(1_018_623_007),
        FillBlock {
            block_number: 1_018_623_007,
            block_time: at(25, 650_866_000),
            local_time: at(25, 790_352_990),
            fills: vec![],
            spot_rows: 0,
            outcome_rows: 0,
        }
    );
}

#[test]
fn decodes_both_rows_of_a_trade() {
    let block = block(1_018_632_945);
    assert_eq!(block.block_time, at(11 * 60 + 50, 625_702_062));
    assert_eq!(block.fills.len(), 2);

    let maker = &block.fills[0];
    assert_eq!(
        maker.user.to_string(),
        "0x6b13de56131bfa2256e2dcd64b67c38272c72318"
    );
    assert_eq!(maker.coin, "BTC");
    assert_eq!(maker.time, Timestamp::from_millis(1_780_305_110_625));
    assert_eq!(maker.tid, TradeId(773_594_879_104_157));
    assert_eq!(maker.oid, OrderId(451_146_983_748));
    assert_eq!((maker.side, maker.crossed), (Side::Buy, false));
    assert_eq!(
        (maker.px, maker.qty),
        ("72999".parse().unwrap(), "0.195".parse().unwrap())
    );
    assert_eq!(maker.start_position, "0.01345".parse().unwrap());
    assert_eq!(maker.dir, "Open Long");
    assert_eq!(
        (maker.closed_pnl, maker.fee),
        (Usd::ZERO, usd_of("2.13522"))
    );
    assert_eq!(maker.fee_token, "USDC");
    assert_eq!(maker.hash[..3], [0xb5, 0xe9, 0x4f]);
    assert_eq!(
        (
            maker.cloid,
            maker.builder_fee,
            maker.builder,
            maker.deployer_fee
        ),
        (None, None, None, None)
    );
    assert_eq!(
        (maker.priority_gas, maker.twap_id, &maker.liquidation),
        (None, None, &None)
    );

    let taker = &block.fills[1];
    assert_eq!((taker.side, taker.crossed), (Side::Sell, true));
    assert_eq!(taker.tid, maker.tid);
    assert_eq!(taker.start_position, "-12.42".parse().unwrap());
    assert_eq!(taker.fee, usd_of("2.989309"));
    // The block's time in milliseconds is the fill's time.
    assert_eq!(block.block_time.as_millis(), taker.time.as_millis());
}

#[test]
fn decodes_optional_fields() {
    let priority = &block(1_018_629_047).fills[1];
    assert_eq!(priority.priority_gas, Some("0.00013325".parse().unwrap()));
    assert_eq!(
        priority.cloid,
        Some(ClientOrderId::from_u128(
            0xac09_e828_387b_8815_721d_a725_fd3b_24ad
        ))
    );

    let twap = &block(1_018_643_292).fills[0];
    assert_eq!(twap.twap_id, Some(1_879_319));
    assert_eq!(twap.hash, [0; 32], "TWAP slices carry a zero hash");

    let built = &block(1_018_635_508).fills[0];
    assert_eq!(built.coin, "xyz:INTC");
    assert_eq!(built.builder_fee, Some(usd_of("0.0547")));
    assert_eq!(
        built.builder.unwrap().to_string(),
        "0x22047776933bc123d0602ed17aaf0d2f5647df0c"
    );
    assert_eq!(built.deployer_fee, Some(usd_of("0.001417")));

    let flip = &block(1_018_649_868).fills[1];
    assert_eq!(flip.dir, "Long > Short");
    assert_eq!(flip.closed_pnl, usd_of("0.062166"));
}

#[test]
fn decodes_a_liquidation_and_negative_fees() {
    let block = block(1_018_635_033);
    let (maker, liquidated) = (&block.fills[0], &block.fills[1]);

    let expected = Liquidation {
        liquidated_user: liquidated.user,
        mark_px: "109.07".parse().unwrap(),
        method: "market".to_owned(),
    };
    assert_eq!(maker.liquidation, Some(expected.clone()));
    assert_eq!(liquidated.liquidation, Some(expected));
    assert_eq!(
        liquidated.user.to_string(),
        "0x01b742badc86efc9713c256c1b5431865e883f26"
    );

    assert_eq!(
        (maker.fee, maker.deployer_fee),
        (usd_of("-0.000297"), Some(usd_of("-0.000148")))
    );
    assert_eq!(liquidated.closed_pnl, usd_of("-5.5237"));
}

#[test]
fn start_position_noise_is_rounded_away() {
    let noisy = &block(1_018_634_449).fills[1];
    assert_eq!(noisy.coin, "CRV");
    assert_eq!(noisy.start_position, "-1081829.6".parse().unwrap());
    assert_eq!(noisy.fee, usd_of("-0.000001"), "a maker rebate");
}

#[test]
fn spot_and_outcome_rows_are_counted_not_decoded() {
    let outcome = block(1_018_646_669);
    assert_eq!(
        (outcome.fills.len(), outcome.spot_rows, outcome.outcome_rows),
        (0, 0, 2)
    );
    let spot = block(1_018_648_332);
    assert_eq!(
        (spot.fills.len(), spot.spot_rows, spot.outcome_rows),
        (0, 2, 0)
    );

    let all = blocks();
    assert_eq!(all.len(), 11);
    assert_eq!(all.iter().map(|block| block.fills.len()).sum::<usize>(), 16);
}

#[test]
fn malformed_fill_blocks_are_errors() {
    let line = FILL_LINES.lines().nth(2).unwrap();
    assert!(line.contains(r#""coin":"BTC""#));

    type Check = fn(&DecodeError) -> bool;
    let cases: [(String, Check); 6] = [
        (line.replace(r#""side":"A""#, r#""side":"S""#), |error| {
            matches!(error, DecodeError::Invalid { field: "side", .. })
        }),
        (
            line.replace(r#""px":"72999.0""#, r#""px":"72999.000000001""#),
            |error| matches!(error, DecodeError::Number { field: "px", .. }),
        ),
        (
            line.replace(r#""fee":"2.13522""#, r#""fee":"two""#),
            |error| matches!(error, DecodeError::Number { field: "fee", .. }),
        ),
        (line.replace("0x6b13de56131b", "0x6b13"), |error| {
            matches!(error, DecodeError::Invalid { field: "user", .. })
        }),
        (
            line.replace(r#""crossed":false"#, r#""crossed":"no""#),
            |error| matches!(error, DecodeError::Json(_)),
        ),
        (
            line.replace("2026-06-01T09:11:50.625702062", "yesterday"),
            |error| {
                matches!(
                    error,
                    DecodeError::Invalid {
                        field: "block_time",
                        ..
                    }
                )
            },
        ),
    ];
    for (text, is_expected) in cases {
        let error = decode_fill_block(text.as_bytes()).unwrap_err();
        assert!(is_expected(&error), "{error:?}");
    }
    assert!(matches!(
        decode_fill_block(b"[]"),
        Err(DecodeError::Json(_))
    ));
}

#[test]
fn a_trade_becomes_one_event_from_the_taker_row() {
    let selected = |network| {
        move |coin: &str| {
            (network == Network::Mainnet && matches!(coin, "BTC" | "TRX"))
                .then(|| instrument_id(network, coin))
        }
    };
    let ts_local = at(3_600, 0);

    let mut events = Vec::new();
    for block in blocks() {
        trade_events(&block, selected(Network::Mainnet), ts_local, &mut events);
    }
    assert_eq!(
        events.len(),
        2,
        "one event per trade, only for listed instruments"
    );

    assert_eq!(events[0].instrument, instrument_id(Network::Mainnet, "BTC"));
    assert_eq!(events[0].ts_exch, Timestamp::from_millis(1_780_305_110_625));
    assert_eq!(events[0].ts_local, ts_local);
    assert_eq!(
        events[0].kind,
        MarketEventKind::Trade(Trade {
            tid: TradeId(773_594_879_104_157),
            px: "72999".parse().unwrap(),
            qty: "0.195".parse().unwrap(),
            aggressor: Side::Sell,
        })
    );
    let MarketEventKind::Trade(trx) = &events[1].kind else {
        panic!("expected a trade");
    };
    assert_eq!(events[1].ts_exch, Timestamp::from_millis(1_780_306_276_565));
    assert_eq!(events[1].ts_local, ts_local);
    assert_eq!(
        (events[1].instrument, trx.aggressor),
        (instrument_id(Network::Mainnet, "TRX"), Side::Sell)
    );

    let mut testnet = Vec::new();
    trade_events(
        &blocks()[2],
        selected(Network::Testnet),
        ts_local,
        &mut testnet,
    );
    assert!(testnet.is_empty());
}
