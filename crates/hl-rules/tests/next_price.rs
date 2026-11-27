//! Successor checks against a unit-by-unit scan of the fixed-point prices.

use hl_rules::{RuleError, RuleVersion};
use hl_types::{Px, Timestamp};

fn rules(sz_decimals: u8) -> RuleVersion {
    RuleVersion::new(Timestamp::EPOCH, sz_decimals, 0).expect("supported precision")
}

fn px(text: &str) -> Px {
    text.parse().expect("exact test price")
}

fn compare_at_boundary(rules: &RuleVersion, boundary: &str, lower_gap: &str, upper_gap: &str) {
    let boundary = px(boundary);
    let before = boundary - px(lower_gap);
    let after = boundary + px(upper_gap);
    for anchor in [before, boundary, after] {
        assert_eq!(rules.check_price(anchor), Ok(()), "{rules:?} at {anchor}");
    }
    assert_eq!(rules.next_price_above(before), Ok(Some(boundary)));
    assert_eq!(rules.next_price_above(boundary), Ok(Some(after)));

    let first = before.as_e8() - 1;
    let last = boundary.as_e8() + 1_000;
    compare_range(rules, first, last, last + px(upper_gap).as_e8());
}

fn compare_range(rules: &RuleVersion, first: i64, last: i64, limit: i64) {
    // Between two accepted prices, all inputs have the same successor. Keep
    // the first accepted candidate until the input reaches it, then resume
    // scanning one raw unit at a time. The limit bounds a failing search.
    let mut candidate = first;
    for raw in first..=last {
        if raw == candidate {
            candidate += 1;
            while rules.check_price(Px::from_e8(candidate)).is_err() {
                assert!(candidate < limit, "no price above {raw} for {rules:?}");
                candidate += 1;
            }
        }
        let price = Px::from_e8(raw);
        assert_eq!(
            rules.next_price_above(price),
            Ok(Some(Px::from_e8(candidate))),
            "{rules:?} at {price}"
        );
    }
}

#[test]
fn next_price_matches_the_first_accepted_price_across_both_boundaries() {
    // The first power crosses a decade on the decimal grid. At the second
    // power the five-significant-figure limit first widens the gap (HL-23,
    // HL-24). The neighboring prices also pin where that change occurs.
    for (sz_decimals, power, crossover, decimal_gap, wider_gap) in [
        (0, "0.01", "0.1", "0.000001", "0.00001"),
        (1, "0.1", "1", "0.00001", "0.0001"),
        (2, "1", "10", "0.0001", "0.001"),
        (3, "10", "100", "0.001", "0.01"),
        (4, "100", "1000", "0.01", "0.1"),
        (5, "1000", "10000", "0.1", "1"),
    ] {
        let rules = rules(sz_decimals);
        compare_at_boundary(&rules, power, decimal_gap, decimal_gap);
        compare_at_boundary(&rules, crossover, decimal_gap, wider_gap);
    }
}

#[test]
fn whole_price_precision_crosses_a_power_of_ten() {
    // With zero price decimals the gap is one on either side of ten.
    compare_range(
        &rules(6),
        px("9.99999999").as_e8(),
        px("10.00000001").as_e8(),
        px("11").as_e8(),
    );
}

#[test]
fn next_price_requires_a_positive_input_and_handles_the_first_grid_point() {
    for (sz_decimals, first_price) in [
        (0, "0.000001"),
        (1, "0.00001"),
        (2, "0.0001"),
        (3, "0.001"),
        (4, "0.01"),
        (5, "0.1"),
        (6, "1"),
    ] {
        let rules = rules(sz_decimals);
        for raw in [i64::MIN, -1, 0] {
            assert_eq!(
                rules.next_price_above(Px::from_e8(raw)),
                Err(RuleError::NonPositivePrice)
            );
        }
        let first = px(first_price);
        assert_eq!(rules.check_price(first), Ok(()));
        assert_eq!(rules.next_price_above(Px::from_e8(1)), Ok(Some(first)));
        assert_eq!(rules.next_price_above(first), Ok(Some(first + first)));
    }
}

#[test]
fn next_price_preserves_integer_neighbors_through_the_fixed_point_ceiling() {
    let largest_integer = px("92233720368");
    for sz_decimals in 0..=6 {
        let rules = rules(sz_decimals);
        for (bid, ask) in [
            ("9999.99999999", "10000"),
            ("10000", "10001"),
            ("10001", "10002"),
            ("99999", "100000"),
            ("100000", "100001"),
            ("92233720367.99999999", "92233720368"),
        ] {
            assert_eq!(rules.check_price(px(ask)), Ok(()));
            assert_eq!(rules.next_price_above(px(bid)), Ok(Some(px(ask))));
        }
        for price in [
            largest_integer,
            Px::from_e8(largest_integer.as_e8() + 1),
            Px::from_e8(i64::MAX),
        ] {
            assert_eq!(rules.next_price_above(price), Ok(None));
        }
    }
}
