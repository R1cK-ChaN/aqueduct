use std::fmt;

use hl_rules::RuleError;
use hl_types::{InstrumentId, Timestamp};

/// A simulated action could not be checked against its dated rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimError {
    /// Instrument whose rules were requested.
    pub instrument: InstrumentId,
    /// Modeled effect time of the action.
    pub effective_at: Timestamp,
    /// Reason the rule lookup failed.
    pub source: RuleError,
}

impl fmt::Display for SimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "instrument {} at effect time {} ns: ",
            self.instrument,
            self.effective_at.as_nanos()
        )?;
        match self.source {
            RuleError::MissingCoverage { first_start, .. } => {
                write!(f, "rule coverage starts at {} ns", first_start.as_nanos())
            }
            RuleError::UnknownInstrument(_) => f.write_str("no rule history"),
            source => source.fmt(f),
        }
    }
}

impl std::error::Error for SimError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}
