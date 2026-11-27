//! Replays a real recording through the codec and the book, and checks the
//! book against an independent channel.
//!
//! The `l2` channel sends a snapshot and then diffs that remove levels by
//! position (HL-66, HL-71). The `bbo` channel separately reports the best bid and ask of a
//! block (HL-78). If the diffs are decoded and applied correctly, then for every
//! block time at which both a book update and a `bbo` message exist, the top
//! of the rebuilt book equals the `bbo` message (HL-137).
//!
//! By default the test reads the fixture shipped with `hl-codec`. Set
//! `HLMM_RAWLOG` to the path of any decompressed recording of the Hyperliquid
//! feed to run the same check on it.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;

use hl_book::{DiffOutcome, L2Book};
use hl_codec::hyperliquid::{Network, decode_ws_frame, instrument_id, market_events};
use hl_codec::rawlog::{RecordKind, RecordReader};
use hl_types::{InstrumentId, Level, MarketEventKind, Px, Qty, Timestamp};

type Top = (Option<(Px, Qty)>, Option<(Px, Qty)>);

fn top(bid: Option<&Level>, ask: Option<&Level>) -> Top {
    (
        bid.map(|level| (level.px, level.qty)),
        ask.map(|level| (level.px, level.qty)),
    )
}

fn recording() -> PathBuf {
    std::env::var_os("HLMM_RAWLOG").map_or_else(
        || {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../hl-codec/tests/fixtures/hl_ws_pons_20261124.hlrec")
        },
        PathBuf::from,
    )
}

#[test]
fn rebuilt_book_matches_the_bbo_channel() {
    let path = recording();
    let file = File::open(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let frames = RecordReader::new(BufReader::new(file))
        .map(|record| record.expect("recording is intact"))
        .filter(|record| record.kind == RecordKind::RecvText)
        .map(|record| {
            let frame = decode_ws_frame(&record.payload).expect("frame decodes");
            (Timestamp::from_nanos(record.recv_wall_ns), frame)
        });

    let mut instruments = BTreeSet::new();
    let mut books: BTreeMap<InstrumentId, L2Book> = BTreeMap::new();
    let mut book_tops: BTreeMap<(InstrumentId, Timestamp), Top> = BTreeMap::new();
    let mut bbo_tops: BTreeMap<(InstrumentId, Timestamp), Top> = BTreeMap::new();
    let (mut snapshots, mut diffs) = (0, 0);
    let mut events = Vec::new();
    for (ts_local, frame) in frames {
        events.clear();
        market_events(
            &frame,
            |coin| Ok(Some(instrument_id(Network::Mainnet, coin))),
            ts_local,
            &mut events,
        )
        .expect("accept every coin in the recording");
        for event in &events {
            instruments.insert(event.instrument);
            let key = (event.instrument, event.ts_exch);
            match &event.kind {
                MarketEventKind::L2Snapshot(_) | MarketEventKind::L2Diff(_) => {
                    let book = books.entry(event.instrument).or_default();
                    let outcome = book.apply(event).unwrap_or_else(|error| {
                        panic!(
                            "{error} at {:?} for instrument {}",
                            event.ts_exch, event.instrument
                        )
                    });
                    assert!(book.is_synced());
                    if outcome == Some(DiffOutcome::SkippedStale) {
                        continue;
                    }
                    if matches!(event.kind, MarketEventKind::L2Snapshot(_)) {
                        snapshots += 1;
                    } else {
                        diffs += 1;
                    }
                    book_tops.insert(key, top(book.best_bid(), book.best_ask()));
                }
                MarketEventKind::Bbo(bbo) => {
                    bbo_tops.insert(key, top(bbo.bid.as_ref(), bbo.ask.as_ref()));
                }
                MarketEventKind::Trade(_) | MarketEventKind::FeedGap => {}
            }
        }
    }

    let mut compared = 0;
    for (key, bbo_top) in &bbo_tops {
        if let Some(book_top) = book_tops.get(key) {
            assert_eq!(book_top, bbo_top, "instrument {} at {:?}", key.0, key.1);
            compared += 1;
        }
    }
    println!(
        "{}: {} instruments, {snapshots} snapshots, {diffs} diffs, {} bbo updates, {compared} compared",
        path.display(),
        instruments.len(),
        bbo_tops.len()
    );
    assert_eq!(snapshots, instruments.len(), "one snapshot per instrument");
    assert!(diffs >= 100, "only {diffs} diffs");
    assert!(
        compared >= 20,
        "only {compared} block times had both a book update and a bbo"
    );
}
