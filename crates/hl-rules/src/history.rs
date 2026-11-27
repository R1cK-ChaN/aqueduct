use std::collections::{BTreeMap, btree_map::Entry};

use hl_types::{InstrumentId, Timestamp};

use crate::{RuleError, RuleVersion};

/// One instrument's rule versions in increasing exchange-time order (RFC 0009 D4).
#[derive(Debug, Clone)]
pub struct RuleHistory {
    versions: Vec<RuleVersion>,
}

impl RuleHistory {
    /// Builds a history from a nonempty sequence with strictly increasing starts.
    pub fn new(versions: Vec<RuleVersion>) -> Result<Self, RuleError> {
        if versions.is_empty() {
            return Err(RuleError::EmptyHistory);
        }
        for pair in versions.windows(2) {
            let previous = pair[0].effective_from();
            let next = pair[1].effective_from();
            if next <= previous {
                return Err(RuleError::NonIncreasingStart { previous, next });
            }
        }
        Ok(Self { versions })
    }

    /// Returns the last version starting at or before `ts_exch`.
    /// A time before the first start gives [`RuleError::MissingCoverage`].
    pub fn at(&self, ts_exch: Timestamp) -> Result<&RuleVersion, RuleError> {
        let end = self
            .versions
            .partition_point(|version| version.effective_from() <= ts_exch);
        self.versions[..end]
            .last()
            .ok_or_else(|| RuleError::MissingCoverage {
                ts_exch,
                first_start: self.versions[0].effective_from(),
            })
    }
}

/// The rule histories keyed by stable instrument id (RFC 0009 D4).
#[derive(Debug, Clone, Default)]
pub struct RuleBook {
    histories: BTreeMap<InstrumentId, RuleHistory>,
}

impl RuleBook {
    /// Creates an empty rule book.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an instrument's history. A duplicate id keeps its original history
    /// and returns [`RuleError::DuplicateInstrument`].
    pub fn insert(&mut self, id: InstrumentId, history: RuleHistory) -> Result<(), RuleError> {
        match self.histories.entry(id) {
            Entry::Vacant(entry) => {
                entry.insert(history);
                Ok(())
            }
            Entry::Occupied(_) => Err(RuleError::DuplicateInstrument(id)),
        }
    }

    /// Looks up an instrument's version at the given exchange time.
    /// An absent id gives [`RuleError::UnknownInstrument`]. A time before its
    /// history starts gives [`RuleError::MissingCoverage`].
    pub fn at(&self, id: InstrumentId, ts_exch: Timestamp) -> Result<&RuleVersion, RuleError> {
        self.histories
            .get(&id)
            .ok_or(RuleError::UnknownInstrument(id))?
            .at(ts_exch)
    }
}
