//! One paper strategy, simulator and recorder source.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use hl_codec::hyperliquid::{Network, SelectedRules, instrument_id};
use hl_recorder::{Config as RecorderConfig, SourceConfig};
use hl_run::select_rules;
use hl_sim::SimConfig;
use hl_strategy::FixedSpreadConfig;
use hl_types::{MarketEvent, MarketEventKind};
use serde::Deserialize;

/// Validated run settings. Each session records all component parameters.
#[derive(Debug)]
pub struct PaperConfig {
    pub(crate) out_dir: PathBuf,
    pub(crate) network: Network,
    pub(crate) coin: String,
    rules_file: Option<PathBuf>,
    pub(crate) queue_capacity: usize,
    pub(crate) zstd_level: i32,
    pub(crate) source: SourceConfig,
    pub(crate) strategy: FixedSpreadConfig,
    pub(crate) sim: SimConfig,
}

#[derive(Deserialize)]
struct Settings {
    network: String,
    coin: String,
    rules_file: Option<PathBuf>,
    strategy_name: String,
    strategy: BTreeMap<String, String>,
    sim: BTreeMap<String, String>,
    #[serde(flatten)]
    capture: toml::Table,
}

impl PaperConfig {
    /// Reads the paper settings and resolves a relative rules path beside the TOML file.
    pub fn from_file(path: &Path) -> io::Result<Self> {
        let mut config = Self::from_toml(&fs::read_to_string(path)?).map_err(io::Error::other)?;
        if let (Some(directory), Some(rules)) = (path.parent(), &mut config.rules_file) {
            *rules = directory.join(&*rules);
        }
        Ok(config)
    }

    /// Loads the configured rules for this run's exact network and coin.
    pub fn load_rules(&self) -> Result<Option<SelectedRules>, String> {
        self.rules_file
            .as_deref()
            .map(|path| {
                let text = fs::read_to_string(path);
                select_rules(path, text, self.network, &self.coin)
            })
            .transpose()
    }

    /// Apply the offline quoter and report range checks as each input arrives.
    pub(crate) fn validate_event(&self, event: &MarketEvent) -> Result<(), &'static str> {
        let latency = (self.sim.order_latency + self.sim.report_latency).as_nanos();
        if i128::from(event.ts_local.as_nanos()) + latency as i128 > i128::from(i64::MAX) {
            return Err("report time with these latencies exceeds the timestamp range");
        }
        if let MarketEventKind::Bbo(bbo) = &event.kind
            && let (Some(bid), Some(ask)) = (bbo.bid, bbo.ask)
        {
            let mid = bid.px.midpoint(ask.px).as_e8();
            let spread = self.strategy.half_spread.as_e8();
            if mid.checked_add(spread).is_none() || mid.checked_sub(spread).is_none() {
                return Err("quote price with this half spread exceeds the price range");
            }
        }
        Ok(())
    }

    /// Parses the TOML configuration and validates the source and both components.
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let Settings {
            network,
            coin,
            rules_file,
            strategy_name,
            strategy,
            sim,
            mut capture,
        } = toml::from_str(text).map_err(|error| error.to_string())?;
        let network = Network::from_short_name(&network)
            .ok_or_else(|| format!("unknown network {network:?}"))?;
        if coin.trim().is_empty() {
            return Err("coin must not be empty".into());
        }
        if strategy_name != "fixed_spread" {
            return Err(format!("unknown strategy_name {strategy_name:?}"));
        }
        let instrument = instrument_id(network, &coin);
        let strategy = rebuild(strategy, |pairs| {
            FixedSpreadConfig::from_parameters(instrument, pairs)
        })?;
        let sim = rebuild(sim, SimConfig::from_parameters)?;

        // Use the recorder's source schema, defaults and validation, including
        // the path-safe source name. Paper accepts exactly one [source] table.
        let source = capture
            .remove("source")
            .filter(toml::Value::is_table)
            .ok_or("expected one [source] table")?;
        capture.insert("source".into(), toml::Value::Array(vec![source]));
        let capture = toml::to_string(&capture).map_err(|error| error.to_string())?;
        let mut capture = RecorderConfig::from_toml(&capture).map_err(|error| error.to_string())?;
        Ok(Self {
            out_dir: capture.out_dir,
            network,
            coin,
            rules_file,
            queue_capacity: capture.queue_capacity,
            zstd_level: capture.zstd_level,
            source: capture.sources.remove(0),
            strategy,
            sim,
        })
    }
}

fn rebuild<T>(
    values: BTreeMap<String, String>,
    from_parameters: impl FnOnce(&[(String, String)]) -> Result<T, String>,
) -> Result<T, String> {
    let pairs: Vec<_> = values.into_iter().collect();
    from_parameters(&pairs).map_err(|error| {
        let names: Vec<_> = pairs.iter().map(|(key, _)| key.as_str()).collect();
        format!("parameters {}: {error}", names.join(", "))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_network_selects_the_instrument() {
        let example = include_str!("../example.toml");
        let testnet = PaperConfig::from_toml(&example.replace("mainnet", "testnet")).unwrap();
        assert_eq!(testnet.network, Network::Testnet);
        assert_eq!(
            testnet.strategy.instrument,
            instrument_id(Network::Testnet, "BTC")
        );
        assert_eq!(
            PaperConfig::from_toml(&example.replace("mainnet", "unknown")).unwrap_err(),
            "unknown network \"unknown\""
        );
    }

    #[test]
    fn unrepresentable_deadline_fails_even_before_first_quote() {
        let text = include_str!("../example.toml")
            .replace("400000000", "0")
            .replace("250000000", &i64::MAX.to_string());
        let config = PaperConfig::from_toml(&text).expect("representable durations");
        let gap = MarketEvent {
            instrument: config.strategy.instrument,
            ts_exch: hl_types::Timestamp::EPOCH,
            ts_local: hl_types::Timestamp::from_nanos(1),
            kind: MarketEventKind::FeedGap,
        };
        assert!(config.validate_event(&gap).is_err());
    }

    #[test]
    fn configuration_names_rejected_keys_components_and_parameters() {
        let example = include_str!("../example.toml");
        PaperConfig::from_toml(example).expect("valid example");
        for (from, to, error) in [
            ("out_dir =", "unknown = 3\nout_dir =", "unknown"),
            ("url =", "subscrib_once = []\nurl =", "subscrib_once"),
            ("fixed_spread", "future_strategy", "future_strategy"),
            ("qty_base", "wrong_qty", "wrong_qty"),
            ("6.0", "bad", "half_spread_quote_per_base"),
            ("400000000", "-1", "order_latency_ns"),
        ] {
            let message = PaperConfig::from_toml(&example.replace(from, to))
                .expect_err("invalid settings")
                .to_string();
            assert!(message.contains(error), "{message}");
        }
    }
}
