//! Dated Hyperliquid perpetual rules for submitted prices and sizes.
//!
//! [`RuleBook::at`] selects a [`RuleVersion`] by instrument and exchange time.
//! The price and size checks cite entries HL-23 through HL-26 in
//! `docs/venues/hyperliquid.md`. RFC 0009 D4 defines the version history.

#![deny(clippy::float_arithmetic)]

mod error;
mod history;

pub use error::RuleError;
pub use history::{RuleBook, RuleHistory};

use hl_types::{DECIMALS, Px, Qty, Timestamp};

const MAX_SIGNIFICANT_FIGURES: u32 = 5; // HL-23
const PERP_MAX_DECIMALS: u8 = 6; // HL-24

/// One instrument's rules and opaque asset id from an inclusive exchange time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuleVersion {
    effective_from: Timestamp,
    sz_decimals: u8,
    asset_id: u64,
}

impl RuleVersion {
    /// Creates a version. Returns [`RuleError::UnsupportedSizeDecimals`] for
    /// `sz_decimals` above six, where HL-24 leaves the price grid undefined.
    pub const fn new(
        effective_from: Timestamp,
        sz_decimals: u8,
        asset_id: u64,
    ) -> Result<Self, RuleError> {
        if sz_decimals > PERP_MAX_DECIMALS {
            return Err(RuleError::UnsupportedSizeDecimals(sz_decimals));
        }
        Ok(Self {
            effective_from,
            sz_decimals,
            asset_id,
        })
    }

    /// First exchange time covered by this version.
    pub const fn effective_from(&self) -> Timestamp {
        self.effective_from
    }

    /// Size precision supplied for this version (HL-26).
    pub const fn sz_decimals(&self) -> u8 {
        self.sz_decimals
    }

    /// Asset id supplied for this version (HL-53, HL-54).
    pub const fn asset_id(&self) -> u64 {
        self.asset_id
    }

    fn max_price_decimals(&self) -> u32 {
        u32::from(PERP_MAX_DECIMALS - self.sz_decimals)
    }

    /// Checks positivity (RFC 0007 D10) and the price grid (HL-23, HL-24, HL-25).
    /// If both fractional limits fail, reports the significant-figure error first.
    pub fn check_price(&self, price: Px) -> Result<(), RuleError> {
        if !price.is_positive() {
            return Err(RuleError::NonPositivePrice);
        }
        let (coefficient, decimals) = decimal_parts(price.as_e8());
        if decimals == 0 {
            return Ok(()); // HL-25
        }
        if digits(coefficient) > MAX_SIGNIFICANT_FIGURES {
            return Err(RuleError::PriceSignificantFigures);
        }
        if decimals > self.max_price_decimals() {
            return Err(RuleError::PriceDecimals);
        }
        Ok(())
    }

    /// Checks positivity (RFC 0007 D10) and size precision (HL-26).
    pub fn check_size(&self, size: Qty) -> Result<(), RuleError> {
        if !size.is_positive() {
            return Err(RuleError::NonPositiveSize);
        }
        let (_, decimals) = decimal_parts(size.as_e8());
        if decimals > u32::from(self.sz_decimals) {
            return Err(RuleError::SizeDecimals);
        }
        Ok(())
    }

    /// Finds the smallest valid price strictly above a positive input, on or
    /// off the grid (RFC 0007 D9, HL-23 to HL-25). Returns `Ok(None)` if that
    /// price exceeds the [`Px`] range, or [`RuleError::NonPositivePrice`] for
    /// zero or a negative input.
    pub fn next_price_above(&self, price: Px) -> Result<Option<Px>, RuleError> {
        if !price.is_positive() {
            return Err(RuleError::NonPositivePrice);
        }
        let raw = price.as_e8();
        // In each decade, the limits require trailing zero digits in raw
        // units. HL-25 caps the step at one whole price. A power-of-ten
        // boundary belongs to both the finer and the coarser grid.
        let decimal_zeros = DECIMALS - self.max_price_decimals();
        let significant_zeros = digits(raw).saturating_sub(MAX_SIGNIFICANT_FIGURES);
        let exponent = decimal_zeros.max(significant_zeros).min(DECIMALS);
        let next = 10_i64
            .checked_pow(exponent)
            .and_then(|step| raw.checked_add(step - raw % step));
        Ok(next.map(Px::from_e8))
    }
}

/// Coefficient and fractional places after stripping fractional trailing zeros.
fn decimal_parts(mut raw: i64) -> (i64, u32) {
    let mut decimals = DECIMALS;
    while decimals > 0 && raw % 10 == 0 {
        raw /= 10;
        decimals -= 1;
    }
    (raw, decimals)
}

/// Number of decimal digits in a positive integer.
fn digits(positive: i64) -> u32 {
    positive.ilog10() + 1
}
