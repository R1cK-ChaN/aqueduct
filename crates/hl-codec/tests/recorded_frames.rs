//! Decodes every frame of a real recording excerpt.
//!
//! The fixture is the PONS traffic, plus control frames, cut from a recording
//! made by `hl-recorder` on 2026-11-24. It holds one `l2` snapshot, diffs in
//! both encodings, `bbo` updates and trades (HL-68, HL-69).

use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use hl_codec::hyperliquid::{WsFrame, decode_ws_frame};
use hl_codec::rawlog::{RecordKind, RecordReader};

#[test]
fn every_recorded_frame_decodes() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hl_ws_pons_20261124.hlrec");
    let reader = RecordReader::new(BufReader::new(File::open(path).expect("fixture exists")));

    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut trades = 0;
    for record in reader {
        let record = record.expect("fixture is intact");
        if record.kind != RecordKind::RecvText {
            continue;
        }
        let frame = decode_ws_frame(&record.payload).unwrap_or_else(|error| {
            panic!("{error}: {}", String::from_utf8_lossy(&record.payload))
        });
        let name = match &frame {
            WsFrame::Book(book) => {
                assert_eq!(book.coin, "PONS");
                assert_eq!((book.bids.len(), book.asks.len()), (20, 20));
                "book".to_owned()
            }
            WsFrame::BookDiff(change) => {
                assert_eq!(change.coin, "PONS");
                "diff".to_owned()
            }
            WsFrame::Bbo(bbo) => {
                assert!(bbo.bid.is_some() && bbo.ask.is_some());
                "bbo".to_owned()
            }
            WsFrame::Trades(batch) => {
                assert!(
                    batch
                        .iter()
                        .all(|trade| trade.coin == "PONS" && trade.qty.is_positive())
                );
                trades += batch.len();
                "trades".to_owned()
            }
            WsFrame::SubscriptionResponse => "ack".to_owned(),
            WsFrame::Pong => "pong".to_owned(),
            WsFrame::Error(message) => panic!("venue error in fixture: {message}"),
            WsFrame::Other { channel } => format!("other:{channel}"),
        };
        *counts.entry(name).or_default() += 1;
    }

    let expected = [
        ("ack", 5),
        ("bbo", 259),
        ("book", 1),
        ("diff", 107),
        ("other:activeAssetCtx", 2),
        ("other:fastAssetCtxs", 1),
        ("pong", 2),
        ("trades", 7),
    ];
    let expected: BTreeMap<String, usize> = expected
        .into_iter()
        .map(|(name, count)| (name.to_owned(), count))
        .collect();
    assert_eq!(counts, expected);
    assert!(trades >= 7);
}
