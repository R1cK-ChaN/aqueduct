//! Check the loader's live flags against the shared recording.

use hl_loader::sources::read_rawlog;

#[path = "../../hl-codec/tests/fixtures/trade_cutoff.rs"]
mod fixture;

#[test]
fn loader_marks_live_trades_in_recording_order() {
    let batch = read_rawlog(fixture::recording().as_slice(), 1).expect("load synthetic recording");
    let rows: Vec<_> = batch
        .ws_trades
        .iter()
        .map(|row| (row.tid, row.live))
        .collect();
    let expected = fixture::EXPECTED.map(|(tid, live)| (tid, u8::from(live)));
    assert_eq!(rows, expected);
}
