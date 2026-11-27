//! Payloads here are copied from frames recorded on 2026-11-24, shortened
//! where the full frame adds nothing.

use hl_types::{Bbo, InstrumentId, L2Snapshot, MarketEventKind, Trade};

use super::super::{Network, UnknownCoin, instrument_id, market_events};
use super::*;

fn level(price: &str, size: &str, orders: u32) -> Level {
    Level {
        px: price.parse().unwrap(),
        qty: size.parse().unwrap(),
        orders,
    }
}

fn ms(millis: i64) -> Timestamp {
    Timestamp::from_millis(millis)
}

fn decode(text: &str) -> WsFrame {
    decode_ws_frame(text.as_bytes()).unwrap()
}

fn error(text: &str) -> DecodeError {
    decode_ws_frame(text.as_bytes()).unwrap_err()
}

const SNAPSHOT: &str = r#"{"channel":"l2","data":{"s":{"coin":"BTC","time":1795489760107,"levels":[[{"px":"59707.0","sz":"8.3975","n":38},{"px":"59706.0","sz":"0.91479","n":3}],[{"px":"59708.0","sz":"0.0015","n":1},{"px":"59716.0","sz":"0.08786","n":1}]]}}}"#;

const PLAIN_DIFF: &str = r#"{"channel":"l2","data":{"u":{"c":"PONS","t":1795489765059,"l":[[{"p":"0.089981","s":"3000.0"}],[{"p":"0.090205","s":"3000.0"}]],"r":[[12],[19]]}}}"#;

/// Inflates to
/// `{"c":"PONS","t":1795489768769,"l":[[{"p":"0.090033","s":"2632.0"},{"p":"0.090012","s":"492.0"}],[{"p":"0.09037","s":"12609.0"}]],"r":[[1],[15]]}`.
const PACKED_DIFF: &str = r#"{"channel":"l2","data":{"c":"TY0xCoAwEAT/svUhd4kmXh6hgqVcZWshaif+3SgpUi4zw95YkTCNwwzChSRRu7bXGPoYlLAhLcuNPTvcsDJ7D8KZpwveNYyHaiqu0FZ/aFTHPhYqLrB+sRnh+C4km9KZPS8="}}"#;

const BBO: &str = r#"{"channel":"bbo","data":{"coin":"BTC","time":1795489760723,"bbo":[{"px":"59722.0","sz":"6.0555","n":13},{"px":"59723.0","sz":"0.74529","n":4}]}}"#;

const TRADES: &str = r#"{"channel":"trades","data":[{"coin":"BTC","side":"A","px":"59752.0","sz":"0.0015","time":1795489751720,"hash":"0x1f489599b58b8a0920c20449778dd90205f3007f508ea8dbc31140ec748f63f3","tid":795421459876667,"users":["0x31dea2516beee92135b96f464eeec3cf292a13f2","0xf5d81a135f756ca16544e53c20fc20643ec3ad53"]},{"coin":"BTC","side":"B","px":"59745.0","sz":"0.01548","time":1795489751850,"hash":"0x0000000000000000000000000000000000000000000000000000000000000000","tid":1001688279816760,"users":["0xfb13690e2a728d2dad21c84bf6d7d1ca5569e68e","0xabfae5ef417fec83a63dcdbff5a13f71d09045c5"]}]}"#;

#[test]
fn decodes_an_l2_snapshot() {
    assert_eq!(
        decode(SNAPSHOT),
        WsFrame::Book(WsBook {
            coin: "BTC".to_owned(),
            time: ms(1_795_489_760_107),
            bids: vec![level("59707", "8.3975", 38), level("59706", "0.91479", 3)],
            asks: vec![level("59708", "0.0015", 1), level("59716", "0.08786", 1)],
        })
    );
}

#[test]
fn decodes_the_l2book_channel_as_a_book_too() {
    let text = SNAPSHOT.replace(
        r#""channel":"l2","data":{"s":"#,
        r#""channel":"l2Book","data":"#,
    );
    let text = text.strip_suffix('}').unwrap();
    assert_eq!(decode(text), decode(SNAPSHOT));
}

#[test]
fn decodes_a_plain_l2_diff() {
    assert_eq!(
        decode(PLAIN_DIFF),
        WsFrame::BookDiff(WsBookDiff {
            coin: "PONS".to_owned(),
            time: ms(1_795_489_765_059),
            diff: L2Diff {
                remove_bids: vec![12],
                remove_asks: vec![19],
                set_bids: vec![level("0.089981", "3000", 0)],
                set_asks: vec![level("0.090205", "3000", 0)],
            },
        })
    );
}

#[test]
fn decodes_a_compressed_l2_diff() {
    assert_eq!(
        decode(PACKED_DIFF),
        WsFrame::BookDiff(WsBookDiff {
            coin: "PONS".to_owned(),
            time: ms(1_795_489_768_769),
            diff: L2Diff {
                remove_bids: vec![1],
                remove_asks: vec![15],
                set_bids: vec![level("0.090033", "2632", 0), level("0.090012", "492", 0)],
                set_asks: vec![level("0.09037", "12609", 0)],
            },
        })
    );
}

#[test]
fn inflates_the_example_from_the_venue_documentation() {
    let packed =
        "q1ZyCnFWsqpWyk0syg6oULJSsjQ3NTDQM1Wq1VFyDfFAkTI2MzXQMwJLVVRWWfmFuTiiyBuamOoZKdXWAgA=";
    let value: serde_json::Value = serde_json::from_str(&inflate(packed).unwrap()).unwrap();
    assert_eq!(
        value,
        serde_json::json!({
            "BTC": { "markPx": "97500.5" },
            "ETH": { "markPx": "3650.25" },
            "xyz:NVDA": { "markPx": "145.2" },
        })
    );
}

#[test]
fn a_diff_without_removals_or_levels_is_empty() {
    let frame = decode(r#"{"channel":"l2","data":{"u":{"c":"PONS","t":5}}}"#);
    let WsFrame::BookDiff(change) = frame else {
        panic!("expected a diff, got {frame:?}");
    };
    assert_eq!(change.diff, L2Diff::default());
}

#[test]
fn decodes_a_bbo_update() {
    assert_eq!(
        decode(BBO),
        WsFrame::Bbo(WsBbo {
            coin: "BTC".to_owned(),
            time: ms(1_795_489_760_723),
            bid: Some(level("59722", "6.0555", 13)),
            ask: Some(level("59723", "0.74529", 4)),
        })
    );
}

#[test]
fn decodes_a_one_sided_bbo() {
    let text = r#"{"channel":"bbo","data":{"coin":"NEW","time":7,"bbo":[null,{"px":"2","sz":"1","n":1}]}}"#;
    let WsFrame::Bbo(bbo) = decode(text) else {
        panic!("expected a bbo");
    };
    assert_eq!((bbo.bid, bbo.ask), (None, Some(level("2", "1", 1))));
}

#[test]
fn decodes_trades_with_both_parties() {
    let WsFrame::Trades(trades) = decode(TRADES) else {
        panic!("expected trades");
    };
    assert_eq!(trades.len(), 2);

    let first = &trades[0];
    assert_eq!(first.coin, "BTC");
    assert_eq!(first.time, ms(1_795_489_751_720));
    assert_eq!(first.tid, TradeId(795_421_459_876_667));
    assert_eq!(
        (first.px, first.qty),
        ("59752".parse().unwrap(), "0.0015".parse().unwrap())
    );
    assert_eq!(
        first.aggressor,
        Side::Sell,
        "side A is a sell that crossed the spread"
    );
    assert_eq!(first.hash[..4], [0x1f, 0x48, 0x95, 0x99]);
    assert_eq!(
        first.buyer.to_string(),
        "0x31dea2516beee92135b96f464eeec3cf292a13f2"
    );
    assert_eq!(
        first.seller.to_string(),
        "0xf5d81a135f756ca16544e53c20fc20643ec3ad53"
    );

    assert_eq!(trades[1].aggressor, Side::Buy);
    assert_eq!(trades[1].hash, [0; 32]);
}

#[test]
fn decodes_control_frames() {
    let ack = r#"{"channel":"subscriptionResponse","data":{"method":"subscribe","subscription":{"type":"l2","c":"BTC","s":null,"m":null}}}"#;
    assert_eq!(decode(ack), WsFrame::SubscriptionResponse);
    assert_eq!(decode(r#"{"channel":"pong"}"#), WsFrame::Pong);
    assert_eq!(
        decode(r#"{"channel":"error","data":"Invalid subscription"}"#),
        WsFrame::Error("Invalid subscription".to_owned())
    );
}

#[test]
fn channels_not_decoded_yet_are_reported_by_name() {
    let ctx =
        r#"{"channel":"activeAssetCtx","data":{"coin":"BTC","ctx":{"funding":"-0.0000027259"}}}"#;
    assert_eq!(
        decode(ctx),
        WsFrame::Other {
            channel: "activeAssetCtx".to_owned()
        }
    );
    assert_eq!(
        decode(r#"{"channel":"fastAssetCtxs","data":"q1Y="}"#),
        WsFrame::Other {
            channel: "fastAssetCtxs".to_owned()
        }
    );
}

#[test]
fn unknown_fields_are_ignored() {
    let text = BBO.replace(
        r#""coin":"BTC","#,
        r#""coin":"BTC","added":{"later":[1,2]},"#,
    );
    assert_eq!(decode(&text), decode(BBO));
}

#[test]
fn malformed_frames_are_json_errors() {
    let cases = [
        "",
        "not json",
        "[]",
        r#"{"data":{}}"#,                                      // no channel
        r#"{"channel":"bbo"}"#,                                // no data
        r#"{"channel":"bbo","data":{"coin":"BTC","time":1}}"#, // missing field
        r#"{"channel":"bbo","data":{"coin":"BTC","time":"1","bbo":[null,null]}}"#,
        r#"{"channel":"bbo","data":{"coin":"BTC","time":1,"bbo":[null]}}"#,
        r#"{"channel":"l2","data":{}}"#,
        r#"{"channel":"error","data":{"not":"a string"}}"#,
    ];
    for text in cases {
        assert!(matches!(error(text), DecodeError::Json(_)), "{text:?}");
    }
    assert!(matches!(
        decode_ws_frame(&[0xff, 0xfe]),
        Err(DecodeError::Json(_))
    ));
}

#[test]
fn an_l2_frame_with_two_payloads_is_refused() {
    let text = PLAIN_DIFF.replace(r#""data":{"u""#, r#""data":{"c":"AAAA","u""#);
    assert!(matches!(error(&text), DecodeError::Json(_)));
}

#[test]
fn prices_and_sizes_must_parse_exactly() {
    let too_fine = BBO.replace("59722.0", "59722.000000001");
    assert_eq!(
        error(&too_fine),
        DecodeError::Number {
            field: "px",
            text: "59722.000000001".to_owned(),
            error: hl_types::ParseFixedError::TooManyDecimals,
        }
    );
    let not_a_number = BBO.replace("6.0555", "1e3");
    assert!(matches!(
        error(&not_a_number),
        DecodeError::Number { field: "sz", .. }
    ));
    let diff = PLAIN_DIFF.replace("0.089981", "abc");
    assert!(matches!(
        error(&diff),
        DecodeError::Number { field: "p", .. }
    ));
}

#[test]
fn invalid_values_name_their_field() {
    let side = TRADES.replace(r#""side":"A""#, r#""side":"S""#);
    assert_eq!(
        error(&side),
        DecodeError::Invalid {
            field: "side",
            text: "S".to_owned()
        }
    );

    let hash = TRADES.replace("0x1f489599", "0x1f48");
    assert!(matches!(
        error(&hash),
        DecodeError::Invalid { field: "hash", .. }
    ));

    let user = TRADES.replace("0x31dea2516b", "31dea2516b");
    assert!(matches!(
        error(&user),
        DecodeError::Invalid { field: "users", .. }
    ));

    let time = BBO.replace("1795489760723", "18446744073709551615");
    assert!(matches!(
        error(&time),
        DecodeError::Invalid { field: "time", .. }
    ));
}

#[test]
fn broken_compressed_payloads_are_reported_by_step() {
    let frame = |packed: &str| format!(r#"{{"channel":"l2","data":{{"c":"{packed}"}}}}"#);
    assert_eq!(
        error(&frame("not base64!")),
        DecodeError::Compressed("base64")
    );
    // Valid base64 of bytes that are not a DEFLATE stream.
    assert_eq!(
        error(&frame("/////w==")),
        DecodeError::Compressed("inflate")
    );
    // A valid stream holding JSON of the wrong shape.
    let packed_bbo =
        "q1ZyCnFWsqpWyk0syg6oULJSsjQ3NTDQM1Wq1VFyDfFAkTI2MzXQMwJLVVRWWfmFuTiiyBuamOoZKdXWAgA=";
    assert!(matches!(error(&frame(packed_bbo)), DecodeError::Json(_)));
}

fn strict(network: Network) -> impl Fn(&str) -> Result<Option<InstrumentId>, UnknownCoin> {
    move |coin| {
        if network == Network::Mainnet && matches!(coin, "BTC" | "PONS") {
            Ok(Some(instrument_id(network, coin)))
        } else {
            Err(UnknownCoin {
                coin: coin.to_owned(),
            })
        }
    }
}

fn events(text: &str) -> Vec<hl_types::MarketEvent> {
    let mut out = Vec::new();
    market_events(
        &decode(text),
        strict(Network::Mainnet),
        ms(9_000_000_000_000),
        &mut out,
    )
    .unwrap();
    out
}

#[test]
fn book_frames_become_single_events_with_both_times() {
    let snapshot = events(SNAPSHOT);
    assert_eq!(snapshot.len(), 1);
    assert_eq!(
        snapshot[0].instrument,
        instrument_id(Network::Mainnet, "BTC")
    );
    assert_eq!(snapshot[0].ts_exch, ms(1_795_489_760_107));
    assert_eq!(snapshot[0].ts_local, ms(9_000_000_000_000));
    assert_eq!(
        snapshot[0].kind,
        MarketEventKind::L2Snapshot(L2Snapshot {
            bids: vec![level("59707", "8.3975", 38), level("59706", "0.91479", 3)],
            asks: vec![level("59708", "0.0015", 1), level("59716", "0.08786", 1)],
        })
    );

    let diff = events(PLAIN_DIFF);
    assert_eq!(diff[0].instrument, instrument_id(Network::Mainnet, "PONS"));
    assert!(matches!(&diff[0].kind, MarketEventKind::L2Diff(diff) if diff.remove_asks == [19]));

    let bbo = events(BBO);
    assert_eq!(
        bbo[0].kind,
        MarketEventKind::Bbo(Bbo {
            bid: Some(level("59722", "6.0555", 13)),
            ask: Some(level("59723", "0.74529", 4)),
        })
    );
}

#[test]
fn a_trades_frame_becomes_one_event_per_trade() {
    let trades = events(TRADES);
    assert_eq!(trades.len(), 2);
    assert_eq!(trades[0].ts_exch, ms(1_795_489_751_720));
    assert_eq!(
        trades[0].kind,
        MarketEventKind::Trade(Trade {
            tid: TradeId(795_421_459_876_667),
            px: "59752".parse().unwrap(),
            qty: "0.0015".parse().unwrap(),
            aggressor: Side::Sell,
        })
    );
    assert_eq!(trades[1].ts_exch, ms(1_795_489_751_850));
}

#[test]
fn frames_without_market_content_give_no_events() {
    assert!(events(r#"{"channel":"pong"}"#).is_empty());
    assert!(events(r#"{"channel":"error","data":"x"}"#).is_empty());
    assert!(events(r#"{"channel":"fastAssetCtxs","data":"q1Y="}"#).is_empty());
}

#[test]
fn resolver_can_skip_one_trade_and_keep_the_next() {
    let mixed = TRADES.replacen(r#""coin":"BTC""#, r#""coin":"btc""#, 1);
    let mut out = events(BBO);
    market_events(
        &decode(&mixed),
        |coin| Ok((coin == "BTC").then(|| instrument_id(Network::Mainnet, coin))),
        ms(9_000_000_000_000),
        &mut out,
    )
    .unwrap();
    assert_eq!(&out[..1], events(BBO));
    assert_eq!(&out[1..], &events(TRADES)[1..]);
}

#[test]
fn an_unlisted_coin_is_an_error_and_leaves_the_output_untouched() {
    let unlisted = TRADES.replacen(
        r#""coin":"BTC","side":"B""#,
        r#""coin":"DOGE","side":"B""#,
        1,
    );
    let mut out = events(BBO);
    let result = market_events(
        &decode(&unlisted),
        strict(Network::Mainnet),
        ms(1),
        &mut out,
    );
    assert_eq!(
        result,
        Err(UnknownCoin {
            coin: "DOGE".to_owned()
        })
    );
    assert_eq!(
        out,
        events(BBO),
        "the first trade of the frame is not left behind"
    );

    let testnet = market_events(&decode(BBO), strict(Network::Testnet), ms(1), &mut out);
    assert_eq!(
        testnet,
        Err(UnknownCoin {
            coin: "BTC".to_owned()
        })
    );
}
