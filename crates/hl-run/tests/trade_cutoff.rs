//! Replay the same synthetic recording that the loader classifies by trade id.

use hl_codec::hyperliquid::Network;
use hl_codec::rawlog::{FileOpen, RecordReader};
use hl_run::recording::RecordConverter;
use hl_types::MarketEventKind;

#[path = "../../hl-codec/tests/fixtures/trade_cutoff.rs"]
mod fixture;

#[test]
fn replay_admits_the_live_trade_ids() {
    let bytes = fixture::recording();
    let mut records = RecordReader::new(bytes.as_slice());
    let record = records
        .next()
        .expect("fixture has a header")
        .expect("read header");
    let header = FileOpen::parse(&record).expect("parse synthetic header");
    let mut converter = RecordConverter::new(Network::Mainnet, "BTC");
    let mut events = Vec::new();
    converter
        .file_start(
            header.opened_wall_ns,
            header.opened_mono_ns,
            header
                .chain_position()
                .expect("numbered file")
                .process_start_wall_ns,
            false,
            &mut events,
        )
        .expect("start recording");
    for record in records {
        converter
            .record(&record.expect("read synthetic record"), &mut events)
            .expect("convert synthetic record");
    }
    let admitted: Vec<u64> = events
        .iter()
        .filter_map(|event| match &event.kind {
            MarketEventKind::Trade(trade) => Some(trade.tid.0),
            _ => None,
        })
        .collect();
    let expected_live: Vec<_> = fixture::EXPECTED
        .into_iter()
        .filter_map(|(tid, live)| live.then_some(tid))
        .collect();
    let expected_replayed = fixture::EXPECTED.iter().filter(|(_, live)| !live).count();
    assert_eq!(admitted, expected_live);
    assert_eq!(converter.stats().trades_replayed, expected_replayed as u64);
}
