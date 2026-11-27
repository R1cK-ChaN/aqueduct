//! Rule-version construction and validation through the public API.

use hl_rules::{RuleError, RuleVersion};
use hl_types::{Px, Qty, Timestamp};

fn version(sz_decimals: u8) -> RuleVersion {
    RuleVersion::new(Timestamp::EPOCH, sz_decimals, 0).expect("supported precision")
}

fn price(text: &str) -> Px {
    text.parse().expect("exact price")
}

fn size(text: &str) -> Qty {
    text.parse().expect("exact size")
}

#[test]
fn version_preserves_its_inputs_and_rejects_undefined_precision() {
    let start = Timestamp::from_nanos(123);
    for sz_decimals in 0..=6 {
        let rules = RuleVersion::new(start, sz_decimals, 110_007).expect("supported precision");
        assert_eq!(rules.effective_from(), start);
        assert_eq!(rules.sz_decimals(), sz_decimals);
        assert_eq!(rules.asset_id(), 110_007);
    }
    for sz_decimals in [7, u8::MAX] {
        assert_eq!(
            RuleVersion::new(start, sz_decimals, 110_007),
            Err(RuleError::UnsupportedSizeDecimals(sz_decimals))
        );
    }
    let opaque = RuleVersion::new(start, 2, u64::MAX).expect("opaque asset id");
    assert_eq!(opaque.asset_id(), u64::MAX);
}

#[test]
fn non_integer_prices_with_five_and_six_significant_figures() {
    for (sz_decimals, five, six) in [
        (0, "0.012345", "0.123456"),
        (2, "1.0001", "10.0001"),
        (5, "1234.5", "12345.6"),
    ] {
        let rules = version(sz_decimals);
        assert_eq!(rules.check_price(price(five)), Ok(()), "{five}");
        assert_eq!(
            rules.check_price(price(six)),
            Err(RuleError::PriceSignificantFigures),
            "{six}"
        );
    }
}

#[test]
fn price_decimal_limit_depends_on_size_precision() {
    for (sz_decimals, within_limit, one_extra) in [
        (0, "0.000001", "0.0000001"),
        (2, "0.0001", "0.00001"),
        (5, "0.1", "0.01"),
        (6, "1", "0.1"),
    ] {
        let rules = version(sz_decimals);
        assert_eq!(
            rules.check_price(price(within_limit)),
            Ok(()),
            "{within_limit}"
        );
        assert_eq!(
            rules.check_price(price(one_extra)),
            Err(RuleError::PriceDecimals),
            "{one_extra}"
        );
    }
}

#[test]
fn trailing_zeros_leave_price_unchanged_and_integers_are_exempt() {
    for sz_decimals in [0, 2, 5] {
        let rules = version(sz_decimals);
        for (padded, shortest) in [("1.50", "1.5"), ("1234.50000000", "1234.5")] {
            assert_eq!(price(padded), price(shortest));
            assert_eq!(rules.check_price(price(padded)), Ok(()), "{padded}");
        }
        for integer in ["123456", "92233720368"] {
            assert_eq!(rules.check_price(price(integer)), Ok(()), "{integer}");
        }
    }
}

#[test]
fn prices_and_sizes_must_be_positive() {
    for sz_decimals in [0, 2, 3, 5, 6] {
        let rules = version(sz_decimals);
        for raw in [0, -1, i64::MIN] {
            assert_eq!(
                rules.check_price(Px::from_e8(raw)),
                Err(RuleError::NonPositivePrice)
            );
            assert_eq!(
                rules.check_size(Qty::from_e8(raw)),
                Err(RuleError::NonPositiveSize)
            );
        }
    }
}

#[test]
fn sizes_at_and_beyond_the_decimal_limit() {
    for (sz_decimals, exact, one_extra) in [
        (0, "12", "1.2"),
        (2, "1.23", "1.234"),
        (3, "1.234", "1.2345"),
        (5, "0.01234", "0.012345"),
        (6, "0.123456", "0.1234567"),
    ] {
        let rules = version(sz_decimals);
        assert_eq!(rules.check_size(size(exact)), Ok(()), "{exact}");
        assert_eq!(
            rules.check_size(size(one_extra)),
            Err(RuleError::SizeDecimals),
            "{one_extra}"
        );
        assert_eq!(rules.check_size(size("123456.00000000")), Ok(()));
    }
    assert_eq!(version(2).check_size(size("92233720368.54000000")), Ok(()));
}

#[test]
fn price_breaking_both_limits_reports_significant_figures_first() {
    assert_eq!(
        version(5).check_price(price("1234.56")),
        Err(RuleError::PriceSignificantFigures)
    );
}
