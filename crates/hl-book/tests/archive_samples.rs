//! Runs the archive decoders over whole sample files and checks the two
//! archives against each other. Ignored by default because the samples are
//! not in the repository (the buckets are requester-pays, HL-96).
//!
//! To run it, decompress one hour of `node_fills_by_block` and the L2 files
//! of some coins for the same hour, then:
//!
//! ```text
//! HLMM_ARCHIVE_FILLS=fills.jsonl HLMM_ARCHIVE_L2=BTC.jsonl,JUP.jsonl \
//!     cargo test --release -p hl-book --test archive_samples -- --ignored --nocapture
//! ```

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader};

use hl_book::L2Book;
use hl_codec::hyperliquid::{ArchiveBook, decode_archive_l2_line, decode_fill_block};
use hl_types::{L2Snapshot, Px, Side, Timestamp};

fn lines(path: &str) -> impl Iterator<Item = Vec<u8>> {
    let file = File::open(path).unwrap_or_else(|error| panic!("{path}: {error}"));
    BufReader::new(file)
        .split(b'\n')
        .map(|line| line.expect("readable file"))
        .filter(|line| !line.is_empty())
}

struct Taker {
    time: Timestamp,
    px: Px,
    aggressor: Side,
}

#[test]
#[ignore = "needs sample files: set HLMM_ARCHIVE_FILLS and HLMM_ARCHIVE_L2"]
fn archives_decode_and_agree_with_each_other() {
    let fills_path = std::env::var("HLMM_ARCHIVE_FILLS").expect("HLMM_ARCHIVE_FILLS is set");
    let l2_paths = std::env::var("HLMM_ARCHIVE_L2").expect("HLMM_ARCHIVE_L2 is set");

    // Books: every line decodes, is a valid book, and times strictly increase.
    let mut books: BTreeMap<String, Vec<ArchiveBook>> = BTreeMap::new();
    for path in l2_paths.split(',') {
        let mut validator = L2Book::new();
        for line in lines(path) {
            let decoded = decode_archive_l2_line(&line).expect("L2 line decodes");
            let snapshot = L2Snapshot {
                bids: decoded.book.bids.clone(),
                asks: decoded.book.asks.clone(),
            };
            validator
                .apply_snapshot(&snapshot, decoded.book.time)
                .expect("archived book is valid");
            let series = books.entry(decoded.book.coin.clone()).or_default();
            if let Some(previous) = series.last() {
                assert!(
                    previous.book.time < decoded.book.time,
                    "{path}: time does not increase"
                );
            }
            series.push(decoded);
        }
    }
    for (coin, series) in &books {
        let full = series
            .iter()
            .filter(|b| b.book.bids.len() == 20 && b.book.asks.len() == 20);
        println!(
            "L2 {coin}: {} books, {} with 20 levels a side",
            series.len(),
            full.count()
        );
    }

    // Fills: every block decodes, heights are contiguous, and every perp
    // trade has exactly one row that crossed the spread.
    let (mut blocks, mut perp_rows, mut spot_rows, mut outcome_rows) = (0_u64, 0_u64, 0_u64, 0_u64);
    let mut previous_height = None;
    let mut crossed_rows: BTreeMap<(String, u64, Timestamp), (u8, u8)> = BTreeMap::new();
    let mut takers: BTreeMap<String, Vec<Taker>> = BTreeMap::new();
    for line in lines(&fills_path) {
        let block = decode_fill_block(&line).expect("fill block decodes");
        if let Some(previous) = previous_height {
            assert_eq!(
                block.block_number,
                previous + 1,
                "block heights are contiguous"
            );
        }
        previous_height = Some(block.block_number);
        blocks += 1;
        spot_rows += u64::from(block.spot_rows);
        outcome_rows += u64::from(block.outcome_rows);
        for fill in &block.fills {
            perp_rows += 1;
            assert_eq!(fill.time.as_millis(), block.block_time.as_millis());
            assert!(fill.px.is_positive() && fill.qty.is_positive());
            let rows = crossed_rows
                .entry((fill.coin.clone(), fill.tid.0, fill.time))
                .or_default();
            if fill.crossed {
                rows.0 += 1;
                if books.contains_key(&fill.coin) {
                    let taker = Taker {
                        time: fill.time,
                        px: fill.px,
                        aggressor: fill.side,
                    };
                    takers.entry(fill.coin.clone()).or_default().push(taker);
                }
            } else {
                rows.1 += 1;
            }
        }
    }
    let trades = crossed_rows.len();
    let paired = crossed_rows
        .values()
        .filter(|rows| **rows == (1, 1))
        .count();
    println!(
        "fills: {blocks} blocks, {perp_rows} perp rows, {spot_rows} spot rows, \
         {outcome_rows} outcome rows, {trades} perp trades, {paired} with one taker and one maker row"
    );
    assert_eq!(
        paired, trades,
        "every perp trade has one taker row and one maker row"
    );

    // Cross-check: a taker fill must be priced consistently with the books
    // archived around it. A book stamped T shows the state after block T, so
    // the book before a fill is the last one with an earlier time, and the
    // book after it is the first one with the same or a later time (HL-75).
    //
    // Books are about half a second apart (HL-138) and the market moves in between,
    // so a fill can sit on the wrong side of the book before it. It should
    // not be on the wrong side of both neighbours.
    let wrong_side = |taker: &Taker, book: &ArchiveBook| -> bool {
        match (
            taker.aggressor,
            book.book.bids.first(),
            book.book.asks.first(),
        ) {
            // A seller cannot trade at or above the ask.
            (Side::Sell, _, Some(best_ask)) => taker.px >= best_ask.px,
            // A buyer cannot trade at or below the bid.
            (Side::Buy, Some(best_bid), _) => taker.px <= best_bid.px,
            _ => false,
        }
    };
    for (coin, takers) in &takers {
        let series = &books[coin];
        let (mut compared, mut wrong_before, mut wrong_after, mut wrong_both) = (0_u32, 0, 0, 0);
        for taker in takers {
            let after = series.partition_point(|book| book.book.time < taker.time);
            let (Some(previous), Some(next)) = (
                after.checked_sub(1).map(|index| &series[index]),
                series.get(after),
            ) else {
                continue;
            };
            compared += 1;
            let (before, later) = (wrong_side(taker, previous), wrong_side(taker, next));
            wrong_before += u32::from(before);
            wrong_after += u32::from(later);
            wrong_both += u32::from(before && later);
        }
        let percent = |count: u32| f64::from(count) * 100.0 / f64::from(compared.max(1));
        println!(
            "{coin}: {compared} taker fills; wrong side of the book before: {wrong_before} \
             ({:.2}%), of the book after: {wrong_after} ({:.2}%), of both: {wrong_both}",
            percent(wrong_before),
            percent(wrong_after)
        );
        assert!(compared > 100, "{coin}: only {compared} trades compared");
        assert!(
            percent(wrong_both) < 0.1,
            "{coin}: {wrong_both} fills fit neither neighbour"
        );
    }
}
