//! Accounting for one instrument from fills and mark prices.
//!
//! [`Ledger`] starts with zero position and cash, books the fee carried by each
//! fill, and reports gross and net profit at the last supplied mark. Monetary
//! amounts use the instrument's settlement token (HL-28 in `docs/venues/hyperliquid.md`).
//! The simulator models fee rounding (HL-30). Other callers supply their own
//! fills and marks through the same interface.
//!
//! Products must be exact at eight decimal places and all arithmetic is checked.
//! HL-24 and HL-26 provide the grid rationale for exact fill and mid-price
//! products. Every failed update preserves the previous state.
//!
//! V1 excludes funding, margin, transfers, and the split between realized and
//! unrealized profit. Callers choose mark prices and the order of events.

#![deny(clippy::float_arithmetic)]

use core::fmt;

use hl_types::{Fill, InstrumentId, Px, Qty, SCALE, Side, Usd};

/// Why an update or profit calculation could not be represented exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedgerError {
    /// The fill belongs to another instrument.
    WrongInstrument {
        /// Instrument accounted for by this ledger.
        expected: InstrumentId,
        /// Instrument supplied with the fill.
        actual: InstrumentId,
    },
    /// A price-size product has a nonzero digit beyond eight decimal places.
    InexactProduct {
        /// Quantity whose calculation failed.
        operation: &'static str,
        /// Price used in the product.
        px: Px,
        /// Size used in the product, signed when valuing a position.
        qty: Qty,
    },
    /// A result is outside the underlying signed 64-bit fixed-point range.
    Overflow {
        /// Quantity whose calculation failed.
        operation: &'static str,
    },
    /// Profit for an open position needs a mark price.
    MissingMark {
        /// Position that could not be valued.
        position: Qty,
    },
}

impl fmt::Display for LedgerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongInstrument { expected, actual } => write!(
                f,
                "fill instrument {actual} does not match ledger instrument {expected}"
            ),
            Self::InexactProduct { operation, px, qty } => {
                write!(
                    f,
                    "inexact product for {operation}, price {px} times size {qty}"
                )
            }
            Self::Overflow { operation } => write!(f, "fixed-point overflow for {operation}"),
            Self::MissingMark { position } => {
                write!(f, "profit for position {position} requires a mark price")
            }
        }
    }
}

impl std::error::Error for LedgerError {}

/// Total profit in the instrument's settlement token at the current mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Profit {
    /// Trading cash plus the marked value of the signed position.
    pub gross: Usd,
    /// Gross profit minus reported fees, including negative fees as rebates.
    pub net: Usd,
}

/// Position, trading cash, reported fees and traded notional of one instrument.
///
/// Fill and mark updates are atomic. A fill can open a position before the first
/// mark, in which case [`Self::profit`] returns [`LedgerError::MissingMark`].
/// Profit for a flat position is defined even without a mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ledger {
    instrument: InstrumentId,
    position: Qty,
    cash: Usd,
    fees: Usd,
    notional: Usd,
    mark: Option<Px>,
}

impl Ledger {
    /// Starts an instrument account with zero balances and no mark.
    pub const fn new(instrument: InstrumentId) -> Self {
        Self {
            instrument,
            position: Qty::ZERO,
            cash: Usd::ZERO,
            fees: Usd::ZERO,
            notional: Usd::ZERO,
            mark: None,
        }
    }

    /// Instrument accounted for by this ledger.
    pub const fn instrument(&self) -> InstrumentId {
        self.instrument
    }

    /// Signed position, positive for long and negative for short.
    pub const fn position(&self) -> Qty {
        self.position
    }

    /// Trading cash from executions, before fees.
    pub const fn cash(&self) -> Usd {
        self.cash
    }

    /// Sum of reported fees, positive for costs and negative for rebates (HL-29).
    pub const fn fees(&self) -> Usd {
        self.fees
    }

    /// Sum of execution price times executed size on both sides.
    pub const fn notional(&self) -> Usd {
        self.notional
    }

    /// Last mark price supplied by the caller.
    pub const fn mark(&self) -> Option<Px> {
        self.mark
    }

    /// Applies one fill from this instrument, booking its reported fee exactly.
    ///
    /// The caller supplies the instrument because [`Fill`] carries none. Fill
    /// size follows the positive-size contract on [`Fill::qty`]. The caller
    /// determines event order and ensures each fill is applied once.
    ///
    /// Returns an error without changing any state if the instrument differs,
    /// a product is inexact, or an account or defined profit amount overflows.
    pub fn apply_fill(&mut self, instrument: InstrumentId, fill: &Fill) -> Result<(), LedgerError> {
        if instrument != self.instrument {
            return Err(LedgerError::WrongInstrument {
                expected: self.instrument,
                actual: instrument,
            });
        }

        let notional = exact_product(fill.px, fill.qty, "fill notional")?;
        let (position, cash) = match fill.side {
            Side::Buy => (
                self.position.as_e8().checked_add(fill.qty.as_e8()),
                self.cash.as_e8().checked_sub(notional),
            ),
            Side::Sell => (
                self.position.as_e8().checked_sub(fill.qty.as_e8()),
                self.cash.as_e8().checked_add(notional),
            ),
        };
        let mut next = *self;
        next.position = Qty::from_e8(position.ok_or(LedgerError::Overflow {
            operation: "position",
        })?);
        next.cash = Usd::from_e8(cash.ok_or(LedgerError::Overflow {
            operation: "trading cash",
        })?);
        next.fees = Usd::from_e8(
            self.fees
                .as_e8()
                .checked_add(fill.fee.as_e8())
                .ok_or(LedgerError::Overflow { operation: "fees" })?,
        );
        next.notional = Usd::from_e8(self.notional.as_e8().checked_add(notional).ok_or(
            LedgerError::Overflow {
                operation: "traded notional",
            },
        )?);
        if next.position.is_zero() || next.mark.is_some() {
            next.profit()?;
        }
        *self = next;
        Ok(())
    }

    /// Replaces the mark, checking the resulting profit before committing it.
    ///
    /// Returns an error without changing any state if the marked product is
    /// inexact or any profit amount overflows.
    pub fn apply_mark(&mut self, mark: Px) -> Result<(), LedgerError> {
        let mut next = *self;
        next.mark = Some(mark);
        next.profit()?;
        *self = next;
        Ok(())
    }

    /// Returns `cash + position * mark` and that gross profit minus fees.
    ///
    /// A flat position has zero marked value with any mark or with no mark.
    /// An open position without a mark returns [`LedgerError::MissingMark`].
    pub fn profit(&self) -> Result<Profit, LedgerError> {
        let marked_value = if self.position.is_zero() {
            0
        } else {
            let mark = self.mark.ok_or(LedgerError::MissingMark {
                position: self.position,
            })?;
            exact_product(mark, self.position, "marked position value")?
        };
        let gross = self
            .cash
            .as_e8()
            .checked_add(marked_value)
            .ok_or(LedgerError::Overflow {
                operation: "gross profit",
            })?;
        let net = gross
            .checked_sub(self.fees.as_e8())
            .ok_or(LedgerError::Overflow {
                operation: "net profit",
            })?;
        Ok(Profit {
            gross: Usd::from_e8(gross),
            net: Usd::from_e8(net),
        })
    }
}

fn exact_product(px: Px, qty: Qty, operation: &'static str) -> Result<i64, LedgerError> {
    // Two i64 factors fit in i128, including i64::MIN * i64::MIN = 2^126.
    let product = i128::from(px.as_e8()) * i128::from(qty.as_e8());
    let scale = i128::from(SCALE);
    if product % scale != 0 {
        return Err(LedgerError::InexactProduct { operation, px, qty });
    }
    i64::try_from(product / scale).map_err(|_| LedgerError::Overflow { operation })
}

#[cfg(test)]
mod tests;
