use core::fmt;
use hl_types::{InstrumentId, Timestamp};

/// Why rule construction, lookup or validation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleError {
    /// HL-24 leaves the price grid undefined at this size precision.
    UnsupportedSizeDecimals(u8),
    /// The submitted price fails the positivity check (RFC 0007 D10).
    NonPositivePrice,
    /// The submitted non-integer price exceeds the HL-23 precision.
    PriceSignificantFigures,
    /// The submitted price exceeds the HL-24 decimal limit for this version.
    PriceDecimals,
    /// The submitted size fails the positivity check (RFC 0007 D10).
    NonPositiveSize,
    /// The submitted size exceeds the HL-26 precision for this version.
    SizeDecimals,
    /// A history was supplied without any versions.
    EmptyHistory,
    /// Two successive versions have equal or decreasing start times.
    NonIncreasingStart {
        /// Start of the preceding version in the supplied order.
        previous: Timestamp,
        /// Start of the version that broke the ordering.
        next: Timestamp,
    },
    /// The book already contains a history for this instrument.
    DuplicateInstrument(InstrumentId),
    /// The book has no history for this instrument.
    UnknownInstrument(InstrumentId),
    /// The requested exchange time precedes the first version.
    MissingCoverage {
        /// Requested exchange time.
        ts_exch: Timestamp,
        /// Earliest covered exchange time for this history.
        first_start: Timestamp,
    },
}

impl fmt::Display for RuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSizeDecimals(precision) => {
                write!(
                    f,
                    "undefined price precision for sz_decimals {precision} (HL-24)"
                )
            }
            Self::NonPositivePrice => f.write_str("price must be positive"),
            Self::PriceSignificantFigures => {
                f.write_str("price has too many significant figures (HL-23)")
            }
            Self::PriceDecimals => f.write_str("price has too many decimal places (HL-24)"),
            Self::NonPositiveSize => f.write_str("size must be positive"),
            Self::SizeDecimals => f.write_str("size has too many decimal places (HL-26)"),
            Self::EmptyHistory => f.write_str("rule history is empty"),
            Self::NonIncreasingStart { previous, next } => write!(
                f,
                "rule starts must increase (previous {} ns, next {} ns)",
                previous.as_nanos(),
                next.as_nanos()
            ),
            Self::DuplicateInstrument(id) => {
                write!(f, "duplicate rule history for instrument {id}")
            }
            Self::UnknownInstrument(id) => write!(f, "unknown instrument {id}"),
            Self::MissingCoverage {
                ts_exch,
                first_start,
            } => write!(
                f,
                "no rule coverage at {} ns (first start {} ns)",
                ts_exch.as_nanos(),
                first_start.as_nanos()
            ),
        }
    }
}

impl std::error::Error for RuleError {}
