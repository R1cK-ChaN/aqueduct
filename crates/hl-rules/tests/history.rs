//! Rule selection by instrument and exchange time.

use hl_rules::{RuleBook, RuleError, RuleHistory, RuleVersion};
use hl_types::{InstrumentId, Timestamp};

fn version(start_ns: i64, sz_decimals: u8, asset_id: u64) -> RuleVersion {
    RuleVersion::new(Timestamp::from_nanos(start_ns), sz_decimals, asset_id)
        .expect("valid rule version")
}

#[test]
fn a_single_version_covers_its_start_and_every_later_time() {
    let first = version(-5, 3, 110_001);
    let history = RuleHistory::new(vec![first]).expect("one version");
    let before = Timestamp::from_nanos(-6);
    assert_eq!(
        history.at(before),
        Err(RuleError::MissingCoverage {
            ts_exch: before,
            first_start: first.effective_from(),
        })
    );
    assert_eq!(history.at(first.effective_from()), Ok(&first));
    assert_eq!(history.at(Timestamp::from_nanos(i64::MAX)), Ok(&first));
}

#[test]
fn a_change_takes_effect_at_its_exchange_nanosecond() {
    let first = version(10, 0, 3);
    let second = version(20, 5, 110_007);
    let history = RuleHistory::new(vec![first, second]).expect("increasing starts");
    assert_eq!(history.at(Timestamp::from_nanos(19)), Ok(&first));
    assert_eq!(history.at(Timestamp::from_nanos(20)), Ok(&second));
    assert_eq!(history.at(Timestamp::from_nanos(21)), Ok(&second));
}

#[test]
fn history_rejects_empty_equal_and_decreasing_starts() {
    assert_eq!(
        RuleHistory::new(vec![]).expect_err("empty history"),
        RuleError::EmptyHistory
    );

    let first = version(10, 0, 0);
    let middle = version(20, 1, 1);
    for last in [version(20, 2, 2), version(19, 2, 2)] {
        assert_eq!(
            RuleHistory::new(vec![first, middle, last]).expect_err("invalid start"),
            RuleError::NonIncreasingStart {
                previous: middle.effective_from(),
                next: last.effective_from(),
            }
        );
    }
}

#[test]
fn book_distinguishes_unknown_instruments_from_missing_coverage() {
    let mut book = RuleBook::new();
    let first_id = InstrumentId::new(32);
    let second_id = InstrumentId::new(7);
    let first = version(10, 2, 1);
    let second = version(20, 4, 1);
    for (id, rules) in [(first_id, first), (second_id, second)] {
        book.insert(id, RuleHistory::new(vec![rules]).expect("one version"))
            .expect("distinct id");
    }

    let time = first.effective_from();
    let unknown = InstrumentId::new(8);
    assert_eq!(
        book.at(unknown, time),
        Err(RuleError::UnknownInstrument(unknown))
    );
    assert_eq!(book.at(first_id, time), Ok(&first));
    assert_eq!(
        book.at(second_id, time),
        Err(RuleError::MissingCoverage {
            ts_exch: time,
            first_start: second.effective_from(),
        })
    );
    assert_eq!(book.at(second_id, second.effective_from()), Ok(&second));
}

#[test]
fn a_duplicate_instrument_keeps_its_original_history() {
    let mut book = RuleBook::new();
    let id = InstrumentId::new(2);
    let first = version(10, 2, 5);
    let second = version(20, 4, 110_001);
    let history = RuleHistory::new(vec![first, second]).expect("increasing starts");
    book.insert(id, history).expect("first insertion");

    let replacement = RuleHistory::new(vec![version(0, 6, 0)]).expect("one version");
    assert_eq!(
        book.insert(id, replacement),
        Err(RuleError::DuplicateInstrument(id))
    );
    assert_eq!(book.at(id, first.effective_from()), Ok(&first));
    assert_eq!(book.at(id, second.effective_from()), Ok(&second));
}
