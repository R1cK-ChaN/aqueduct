//! Fixed-point decimals with eight fractional digits.
//!
//! Every value is an `i64` count of 1e-8 units, the same representation as a
//! ClickHouse `Decimal64(8)`. Parsing is exact and formatting prints the
//! shortest exact decimal, so a value survives a round trip through text.

use core::fmt;
use core::ops::{Add, AddAssign, Neg, Sub, SubAssign};
use core::str::FromStr;

/// Number of fractional decimal digits carried by every fixed-point type.
pub const DECIMALS: u32 = 8;

/// Raw units per whole unit: 10^8.
pub const SCALE: i64 = 100_000_000;

const SCALE_U64: u64 = 100_000_000;
const SCALE_I128: i128 = 100_000_000;

/// Why a decimal string could not be parsed into a fixed-point value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseFixedError {
    /// The string was empty.
    Empty,
    /// The string is not of the form `-?digits(.digits)?`.
    InvalidCharacter,
    /// The string has a non-zero digit beyond the eighth decimal place.
    TooManyDecimals,
    /// The value does not fit in the underlying `i64`, or is outside the
    /// range the target type allows.
    OutOfRange,
}

impl fmt::Display for ParseFixedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Empty => "empty string",
            Self::InvalidCharacter => "not a plain decimal number",
            Self::TooManyDecimals => "more than eight decimal places",
            Self::OutOfRange => "value out of range",
        })
    }
}

impl std::error::Error for ParseFixedError {}

fn decimal_digit(byte: u8) -> Result<i128, ParseFixedError> {
    if byte.is_ascii_digit() {
        Ok(i128::from(byte - b'0'))
    } else {
        Err(ParseFixedError::InvalidCharacter)
    }
}

fn parse_e8(text: &str) -> Result<i64, ParseFixedError> {
    if text.is_empty() {
        return Err(ParseFixedError::Empty);
    }
    let (negative, unsigned) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (whole, fraction) = match unsigned.split_once('.') {
        Some((_, "")) => return Err(ParseFixedError::InvalidCharacter),
        Some(parts) => parts,
        None => (unsigned, ""),
    };
    if whole.is_empty() {
        return Err(ParseFixedError::InvalidCharacter);
    }

    let mut raw: i128 = 0;
    for byte in whole.bytes() {
        raw = raw * 10 + decimal_digit(byte)?;
        if raw > i128::from(i64::MAX) {
            return Err(ParseFixedError::OutOfRange);
        }
    }
    let mut places_left = DECIMALS;
    for byte in fraction.bytes() {
        let digit = decimal_digit(byte)?;
        if places_left == 0 {
            if digit != 0 {
                return Err(ParseFixedError::TooManyDecimals);
            }
            continue;
        }
        raw = raw * 10 + digit;
        places_left -= 1;
    }
    raw *= 10_i128.pow(places_left);
    if negative {
        raw = -raw;
    }
    i64::try_from(raw).map_err(|_| ParseFixedError::OutOfRange)
}

fn fmt_e8(raw: i64, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let magnitude = raw.unsigned_abs();
    let whole = magnitude / SCALE_U64;
    let mut fraction = magnitude % SCALE_U64;
    if raw < 0 {
        f.write_str("-")?;
    }
    if fraction == 0 {
        return write!(f, "{whole}");
    }
    let mut width = DECIMALS as usize;
    while fraction.is_multiple_of(10) {
        fraction /= 10;
        width -= 1;
    }
    write!(f, "{whole}.{fraction:0width$}")
}

/// Multiplies two 1e-8 values and returns the 1e-8 product, truncated toward
/// zero. `None` when the result does not fit in an `i64`.
fn mul_e8(a: i64, b: i64) -> Option<i64> {
    i64::try_from(i128::from(a) * i128::from(b) / SCALE_I128).ok()
}

macro_rules! fixed_e8 {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
        pub struct $name(i64);

        impl $name {
            /// The value zero.
            pub const ZERO: Self = Self(0);

            /// Builds a value from raw units of 1e-8.
            pub const fn from_e8(raw: i64) -> Self {
                Self(raw)
            }

            /// Returns the value in raw units of 1e-8.
            pub const fn as_e8(self) -> i64 {
                self.0
            }

            /// True when the value is greater than zero.
            pub const fn is_positive(self) -> bool {
                self.0 > 0
            }

            /// True when the value is less than zero.
            pub const fn is_negative(self) -> bool {
                self.0 < 0
            }

            /// True when the value is exactly zero.
            pub const fn is_zero(self) -> bool {
                self.0 == 0
            }
        }

        impl FromStr for $name {
            type Err = ParseFixedError;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                parse_e8(text).map(Self)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt_e8(self.0, f)
            }
        }

        impl Add for $name {
            type Output = Self;

            fn add(self, rhs: Self) -> Self {
                Self(self.0 + rhs.0)
            }
        }

        impl Sub for $name {
            type Output = Self;

            fn sub(self, rhs: Self) -> Self {
                Self(self.0 - rhs.0)
            }
        }

        impl Neg for $name {
            type Output = Self;

            fn neg(self) -> Self {
                Self(-self.0)
            }
        }

        impl AddAssign for $name {
            fn add_assign(&mut self, rhs: Self) {
                self.0 += rhs.0;
            }
        }

        impl SubAssign for $name {
            fn sub_assign(&mut self, rhs: Self) {
                self.0 -= rhs.0;
            }
        }
    };
}

fixed_e8! {
    /// A price, or a price difference, in quote currency per unit of the
    /// instrument.
    ///
    /// Arithmetic panics on overflow in every build profile.
    Px
}

fixed_e8! {
    /// A size in units of the instrument.
    ///
    /// Order and fill sizes are positive. A position is a signed `Qty`:
    /// positive when long, negative when short.
    Qty
}

fixed_e8! {
    /// An amount used for notional value, fee or profit, with its currency
    /// supplied by the instrument or fill context (HL-28).
    Usd
}

impl Px {
    /// The value of `qty` at this price, truncated toward zero.
    ///
    /// `None` when the result does not fit in the underlying `i64`
    /// (above roughly 92 billion).
    pub fn notional(self, qty: Qty) -> Option<Usd> {
        mul_e8(self.0, qty.0).map(Usd)
    }

    /// The price halfway between `self` and `other`, rounded down to the
    /// nearest 1e-8.
    pub fn midpoint(self, other: Self) -> Self {
        let sum = i128::from(self.0) + i128::from(other.0);
        // The floor of the mean of two i64 values always fits in an i64.
        Self(sum.div_euclid(2) as i64)
    }
}

/// A fee as a fraction of notional: `0.00015` is 1.5 basis points.
///
/// The magnitude is at most 1. A negative rate is a rebate (HL-29).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct FeeRate(i64);

impl FeeRate {
    /// A rate of zero.
    pub const ZERO: Self = Self(0);

    /// Builds a rate from raw units of 1e-8. `None` when the magnitude
    /// exceeds 1.
    pub const fn from_e8(raw: i64) -> Option<Self> {
        if raw.unsigned_abs() <= SCALE_U64 {
            Some(Self(raw))
        } else {
            None
        }
    }

    /// Returns the rate in raw units of 1e-8.
    pub const fn as_e8(self) -> i64 {
        self.0
    }

    /// The fee charged on `notional`, truncated toward zero.
    ///
    /// This truncation is the simulator's fee-model assumption (HL-30).
    /// Fee tiers, discounts and rounding belong to that model. `hl-ledger`
    /// books the fee carried by each fill. Live trading uses the
    /// venue-reported fee.
    pub fn fee_on(self, notional: Usd) -> Usd {
        let product = i128::from(self.0) * i128::from(notional.0) / SCALE_I128;
        // |rate| <= 1, so |product| <= |notional| and the cast is lossless.
        Usd(product as i64)
    }
}

impl FromStr for FeeRate {
    type Err = ParseFixedError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::from_e8(parse_e8(text)?).ok_or(ParseFixedError::OutOfRange)
    }
}

impl fmt::Display for FeeRate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_e8(self.0, f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(text: &str) -> Px {
        text.parse().unwrap()
    }

    fn qty(text: &str) -> Qty {
        text.parse().unwrap()
    }

    fn usd(text: &str) -> Usd {
        text.parse().unwrap()
    }

    #[test]
    fn parses_exact_decimals() {
        assert_eq!(px("97123.5").as_e8(), 9_712_350_000_000);
        assert_eq!(px("0.00000001").as_e8(), 1);
        assert_eq!(px("10").as_e8(), 10 * SCALE);
        assert_eq!(qty("-1.25").as_e8(), -125_000_000);
        assert_eq!(px("0").as_e8(), 0);
        assert_eq!(px("-0.0").as_e8(), 0);
    }

    #[test]
    fn accepts_trailing_zeros_beyond_eight_places() {
        assert_eq!(px("1.500000000").as_e8(), 150_000_000);
    }

    #[test]
    fn rejects_malformed_input() {
        let cases = [
            ("", ParseFixedError::Empty),
            ("-", ParseFixedError::InvalidCharacter),
            (".5", ParseFixedError::InvalidCharacter),
            ("5.", ParseFixedError::InvalidCharacter),
            ("1e5", ParseFixedError::InvalidCharacter),
            ("+1", ParseFixedError::InvalidCharacter),
            (" 1", ParseFixedError::InvalidCharacter),
            ("1.2.3", ParseFixedError::InvalidCharacter),
            ("0.000000001", ParseFixedError::TooManyDecimals),
            ("92233720369", ParseFixedError::OutOfRange),
            (
                "99999999999999999999999999999999999999999",
                ParseFixedError::OutOfRange,
            ),
        ];
        for (text, expected) in cases {
            assert_eq!(text.parse::<Px>(), Err(expected), "input {text:?}");
        }
    }

    #[test]
    fn parses_the_extremes_of_the_range() {
        assert_eq!(px("92233720368.54775807").as_e8(), i64::MAX);
        assert_eq!(px("-92233720368.54775808").as_e8(), i64::MIN);
    }

    #[test]
    fn displays_the_shortest_exact_form() {
        for text in [
            "97123.5",
            "0.00000001",
            "10",
            "-1.25",
            "0",
            "0.00015",
            "-0.5",
        ] {
            assert_eq!(px(text).to_string(), text);
        }
        assert_eq!(px("1.50").to_string(), "1.5");
        assert_eq!(Px::from_e8(i64::MIN).to_string(), "-92233720368.54775808");
    }

    #[test]
    fn adds_and_subtracts_within_a_type() {
        assert_eq!(px("1.5") + px("0.25"), px("1.75"));
        assert_eq!(px("1.5") - px("0.25"), px("1.25"));
        assert_eq!(-qty("2"), qty("-2"));
        let mut total = usd("1");
        total += usd("0.5");
        total -= usd("2");
        assert_eq!(total, usd("-0.5"));
    }

    #[test]
    fn notional_is_price_times_size() {
        assert_eq!(px("97123.5").notional(qty("0.001")), Some(usd("97.1235")));
        assert_eq!(px("0.02512").notional(qty("400")), Some(usd("10.048")));
        assert_eq!(px("2").notional(qty("-3")), Some(usd("-6")));
    }

    #[test]
    fn notional_truncates_toward_zero() {
        assert_eq!(px("0.00000001").notional(qty("0.5")), Some(Usd::ZERO));
        assert_eq!(
            px("0.00000003").notional(qty("-0.5")),
            Some(usd("-0.00000001"))
        );
    }

    #[test]
    fn notional_reports_overflow() {
        assert_eq!(px("90000000000").notional(qty("2")), None);
    }

    #[test]
    fn midpoint_rounds_down() {
        assert_eq!(px("100").midpoint(px("101")), px("100.5"));
        assert_eq!(Px::from_e8(1).midpoint(Px::from_e8(2)), Px::from_e8(1));
        assert_eq!(Px::from_e8(-1).midpoint(Px::from_e8(-2)), Px::from_e8(-2));
        assert_eq!(
            Px::from_e8(i64::MAX).midpoint(Px::from_e8(i64::MAX)),
            Px::from_e8(i64::MAX)
        );
    }

    #[test]
    fn fee_rate_is_bounded_by_one() {
        assert!(FeeRate::from_e8(SCALE).is_some());
        assert!(FeeRate::from_e8(-SCALE).is_some());
        assert!(FeeRate::from_e8(SCALE + 1).is_none());
        assert_eq!("1.5".parse::<FeeRate>(), Err(ParseFixedError::OutOfRange));
        assert_eq!("0.00015".parse::<FeeRate>().unwrap().to_string(), "0.00015");
    }

    #[test]
    fn fee_is_rate_times_notional() {
        let maker: FeeRate = "0.00015".parse().unwrap();
        let taker: FeeRate = "0.00045".parse().unwrap();
        assert_eq!(maker.fee_on(usd("1000")), usd("0.15"));
        assert_eq!(taker.fee_on(usd("1000")), usd("0.45"));
        assert_eq!(maker.fee_on(usd("0.00001")), Usd::ZERO);
        let rebate: FeeRate = "-0.00001".parse().unwrap();
        assert_eq!(rebate.fee_on(usd("1000")), usd("-0.01"));
    }
}
