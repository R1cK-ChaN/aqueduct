//! The dated perpetual rules file from RFC 0009 D7.

use core::{fmt, marker::PhantomData, str::FromStr};
use std::collections::BTreeMap;

use hl_rules::{RuleBook, RuleError, RuleHistory, RuleVersion};
use hl_types::{InstrumentId, Timestamp};
use serde::de::{MapAccess, Visitor, value::MapAccessDeserializer};
use serde::{Deserialize, Deserializer};

use super::{Network, instrument_id};

/// Validated RFC 0009 D7 histories for exact coin names (HL-55), with
/// perpetual rules (HL-23 to HL-26) and dated asset ids (HL-53, HL-54).
#[derive(Debug)]
pub struct RulesFile {
    histories: BTreeMap<InstrumentId, NamedHistory>,
}

#[derive(Debug)]
struct NamedHistory {
    network: Network,
    coin: String,
    versions: Vec<RuleVersion>,
    history: RuleHistory,
}

/// The RFC 0009 D7 rules selected for one run (HL-23 to HL-26, HL-53 to HL-55).
#[derive(Debug)]
pub struct SelectedRules {
    /// Id derived from the RFC 0009 D7 network and exact coin name (HL-55).
    pub instrument_id: InstrumentId,
    /// Versions in RFC 0009 D7 start order, including each asset id (HL-53, HL-54).
    pub versions: Vec<RuleVersion>,
    /// This instrument's RFC 0009 D7 history for exchange-time lookups (HL-23 to HL-26).
    pub book: RuleBook,
}

impl RulesFile {
    /// Selects one exact RFC 0009 D7 pair (HL-55) and returns its versions
    /// and rule book (HL-23 to HL-26, HL-53, HL-54).
    pub fn select(&self, network: Network, coin: &str) -> Result<SelectedRules, RulesFileError> {
        let id = instrument_id(network, coin);
        let entry = self
            .histories
            .get(&id)
            .filter(|entry| entry.network == network && entry.coin == coin)
            .ok_or_else(|| RulesFileError::MissingPair {
                network,
                coin: coin.to_owned(),
            })?;
        let mut book = RuleBook::new();
        book.insert(id, entry.history.clone())
            .map_err(RulesFileError::Rule)?;
        Ok(SelectedRules {
            instrument_id: id,
            versions: entry.versions.clone(),
            book,
        })
    }
}

/// Parses the entire RFC 0009 D7 JSON file, validating instrument identities
/// (HL-55), rule versions (HL-23 to HL-26) and their asset ids (HL-53, HL-54).
pub fn parse_rules_file(text: &str) -> Result<RulesFile, RulesFileError> {
    let Object(file): Object<FileFields> =
        serde_json::from_str(text).map_err(|error| RulesFileError::Json(error.to_string()))?;
    if file.instruments.is_empty() {
        return Err(RulesFileError::Json("instruments must be nonempty".into()));
    }

    let mut histories: BTreeMap<InstrumentId, NamedHistory> = BTreeMap::new();
    for Object(fields) in file.instruments {
        let network = Network::from_short_name(&fields.network).ok_or_else(|| {
            RulesFileError::Json(format!("unknown rules file network {:?}", fields.network))
        })?;
        let coin = fields.coin;
        if coin.trim().is_empty() {
            return Err(RulesFileError::Json(
                "coin must contain a non-whitespace character".into(),
            ));
        }
        let id = instrument_id(network, &coin);
        if let Some(first) = histories.get(&id) {
            if first.network == network && first.coin == coin {
                return Err(RulesFileError::DuplicatePair { network, coin });
            }
            return Err(RulesFileError::IdCollision {
                id,
                first: (first.network, first.coin.clone()),
                second: (network, coin),
            });
        }

        let versions = fields
            .versions
            .into_iter()
            .map(rule_version)
            .collect::<Result<Vec<_>, _>>()?;
        // The session needs the ordered versions and the simulator needs time lookups.
        let history = RuleHistory::new(versions.clone()).map_err(RulesFileError::Rule)?;
        histories.insert(
            id,
            NamedHistory {
                network,
                coin,
                versions,
                history,
            },
        );
    }
    Ok(RulesFile { histories })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileFields {
    instruments: Vec<Object<InstrumentFields>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InstrumentFields {
    network: String,
    coin: String,
    versions: Vec<Object<VersionFields>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VersionFields {
    effective_from: String,
    sz_decimals: u8,
    asset_id: String,
}

// A derived struct also accepts a positional array. Require an object and
// pass its keys directly to the derived visitor so it sees every occurrence.
struct Object<T>(T);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Object<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ObjectVisitor<T>(PhantomData<T>);

        impl<'de, T: Deserialize<'de>> Visitor<'de> for ObjectVisitor<T> {
            type Value = Object<T>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a rules file object")
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Object<T>, A::Error> {
                T::deserialize(MapAccessDeserializer::new(map)).map(Object)
            }
        }

        deserializer.deserialize_map(ObjectVisitor(PhantomData))
    }
}

fn rule_version(Object(fields): Object<VersionFields>) -> Result<RuleVersion, RulesFileError> {
    let start = integer("effective_from", fields.effective_from)?;
    let asset_id = integer("asset_id", fields.asset_id)?;
    RuleVersion::new(Timestamp::from_nanos(start), fields.sz_decimals, asset_id)
        .map_err(RulesFileError::Rule)
}

fn integer<T: FromStr + fmt::Display>(
    field: &'static str,
    value: String,
) -> Result<T, RulesFileError> {
    value
        .parse::<T>()
        .ok()
        .filter(|number| number.to_string() == value)
        .ok_or(RulesFileError::Integer { field, value })
}

/// Why an RFC 0009 D7 rules file or selection failed (HL-23 to HL-26, HL-53 to HL-55).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RulesFileError {
    /// Invalid JSON or a value outside the RFC 0009 D7 file layout.
    Json(String),
    /// A string outside the RFC 0009 D7 integer grammar or range (HL-53, HL-54 for asset ids).
    Integer {
        /// RFC 0009 D7 field containing the integer string.
        field: &'static str,
        /// Decoded string supplied for the RFC 0009 D7 field.
        value: String,
    },
    /// An RFC 0009 D7 rule version or history failed validation (HL-23 to HL-26).
    Rule(RuleError),
    /// The same RFC 0009 D7 pair occurs twice (HL-55).
    DuplicatePair {
        /// Network of the repeated RFC 0009 D7 pair.
        network: Network,
        /// Exact RFC 0009 D7 coin name (HL-55).
        coin: String,
    },
    /// Two distinct RFC 0009 D7 pairs derive the same instrument id (HL-55).
    IdCollision {
        /// Id derived for both RFC 0009 D7 pairs (HL-55).
        id: InstrumentId,
        /// Network and coin of the first RFC 0009 D7 pair (HL-55).
        first: (Network, String),
        /// Network and coin of the conflicting RFC 0009 D7 pair (HL-55).
        second: (Network, String),
    },
    /// The file has no history for the requested RFC 0009 D7 pair (HL-55).
    MissingPair {
        /// Network requested for the RFC 0009 D7 run.
        network: Network,
        /// Exact coin requested for the RFC 0009 D7 run (HL-55).
        coin: String,
    },
}

impl fmt::Display for RulesFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(message) => write!(f, "invalid rules file ({message})"),
            Self::Integer { field, value } => {
                write!(f, "invalid integer string {value:?} for {field}")
            }
            Self::Rule(error) => error.fmt(f),
            Self::DuplicatePair { network, coin } => {
                write!(f, "duplicate rules for {} {coin:?}", network.short_name())
            }
            Self::IdCollision { id, first, second } => write!(
                f,
                "instrument id {id} collides for {} {:?} and {} {:?}",
                first.0.short_name(),
                first.1,
                second.0.short_name(),
                second.1
            ),
            Self::MissingPair { network, coin } => {
                write!(f, "missing rules for {} {coin}", network.short_name())
            }
        }
    }
}

impl std::error::Error for RulesFileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Rule(error) => Some(error),
            _ => None,
        }
    }
}
