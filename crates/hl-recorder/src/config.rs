//! The recorder's configuration file.

use std::collections::BTreeSet;
use std::fmt;
use std::path::PathBuf;

use serde::Deserialize;

use crate::public_v1;

/// The text in a `subscribe_each` template that is replaced by each symbol.
pub const SYMBOL_PLACEHOLDER: &str = "{symbol}";

/// The whole configuration file.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Directory the recordings are written under.
    pub out_dir: PathBuf,
    /// How many frames may wait for the writer before new ones are dropped.
    #[serde(default = "default_queue_capacity")]
    pub queue_capacity: usize,
    /// zstd compression level, 1 to 19.
    #[serde(default = "default_zstd_level")]
    pub zstd_level: i32,
    /// The connections to record. Written as `[[source]]` tables.
    #[serde(rename = "source", default)]
    pub sources: Vec<SourceConfig>,
}

/// One WebSocket connection to record.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceConfig {
    /// Short name used in directory and file names: lower-case letters,
    /// digits, `-` and `_`.
    pub name: String,
    /// `wss://` or `ws://` URL.
    pub url: String,
    #[serde(default)]
    send_policy: Option<String>,
    /// Reconnect when no frame has arrived for this long.
    #[serde(default = "default_stale_after_ms")]
    pub stale_after_ms: u64,
    /// Pause between two subscription messages.
    #[serde(default = "default_subscribe_pace_ms")]
    pub subscribe_pace_ms: u64,
    /// Text frame sent as an application-level ping, if the venue needs one.
    #[serde(default)]
    pub ping_text: Option<String>,
    /// How often `ping_text` is sent.
    #[serde(default = "default_ping_interval_secs")]
    pub ping_interval_secs: u64,
    /// Symbols substituted into every `subscribe_each` template.
    #[serde(default)]
    pub symbols: Vec<String>,
    /// Messages sent once per symbol. Each must contain `{symbol}`.
    #[serde(default)]
    pub subscribe_each: Vec<String>,
    /// Messages sent once per connection.
    #[serde(default)]
    pub subscribe_once: Vec<String>,
}

fn default_queue_capacity() -> usize {
    65_536
}

fn default_zstd_level() -> i32 {
    3
}

fn default_stale_after_ms() -> u64 {
    3_000
}

fn default_subscribe_pace_ms() -> u64 {
    50
}

fn default_ping_interval_secs() -> u64 {
    20
}

/// Why a configuration file was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    message: String,
}

impl ConfigError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ConfigError {}

impl SourceConfig {
    /// The send policy selected in the configuration file, if any.
    pub fn send_policy(&self) -> Option<&str> {
        self.send_policy.as_deref()
    }

    /// Every message to send after connecting, in order: the `subscribe_once`
    /// messages, then for each symbol every `subscribe_each` template.
    pub fn subscriptions(&self) -> Vec<String> {
        self.expanded_subscriptions().collect()
    }

    fn expanded_subscriptions(&self) -> impl Iterator<Item = String> {
        let per_symbol = self.symbols.iter().flat_map(|symbol| {
            self.subscribe_each
                .iter()
                .map(move |template| template.replace(SYMBOL_PLACEHOLDER, symbol))
        });
        self.subscribe_once.iter().cloned().chain(per_symbol)
    }

    fn validate_public_v1(&self) -> Result<(), ConfigError> {
        let fail = |problem| Err(ConfigError::new(format!("{}: {problem}", public_v1::NAME)));
        if self.name != public_v1::SOURCE_NAME {
            return fail("source name must be hl-public");
        }
        if self.url != public_v1::URL {
            return fail("url must equal the public WebSocket URL exactly");
        }
        if self.ping_text.as_deref() != Some(public_v1::PING) {
            return fail("ping_text must equal the policy ping");
        }
        if self
            .symbols
            .iter()
            .any(|symbol| !public_v1::COINS.contains(&symbol.as_str()))
        {
            return fail("symbols include a coin outside the v1 list");
        }
        if self
            .subscribe_each
            .iter()
            .any(|template| !template.contains(SYMBOL_PLACEHOLDER))
        {
            return fail("subscribe_each entries must contain {symbol}");
        }

        let mut subscriptions = BTreeSet::new();
        for text in self.expanded_subscriptions() {
            if text == public_v1::PING || !public_v1::allows(&text) {
                return fail("expanded subscription is outside the subscription allowlist");
            }
            if !subscriptions.insert(text) {
                return fail("duplicate expanded subscription");
            }
        }
        if self.ping_interval_secs == 0 {
            return fail("ping_interval_secs must be positive");
        }
        let pings = public_v1::WINDOW
            .as_secs()
            .div_ceil(self.ping_interval_secs);
        let round_messages = subscriptions.len() * public_v1::MAX_ATTEMPTS;
        if round_messages as u64 + pings > public_v1::MAX_MESSAGES as u64 {
            return fail("subscription rounds and pings exceed the outbound message limit");
        }
        Ok(())
    }

    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        if let Some(policy) = self.send_policy() {
            if policy != public_v1::NAME {
                return Err(ConfigError::new(format!(
                    "unknown send_policy (expected {})",
                    public_v1::NAME
                )));
            }
            // The ordinary syntax diagnostics below may quote a template or URL.
            self.validate_public_v1()?;
        }
        let name = &self.name;
        let name_ok = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "-_".contains(c));
        if !name_ok {
            return Err(ConfigError::new(format!(
                "source name {name:?} must be lower-case letters, digits, '-' or '_'"
            )));
        }
        let fail = |problem: String| Err(ConfigError::new(format!("source {name}: {problem}")));

        if !(self.url.starts_with("wss://") || self.url.starts_with("ws://")) {
            return fail(format!(
                "url {:?} must start with wss:// or ws://",
                self.url
            ));
        }
        if self.stale_after_ms == 0 {
            return fail("stale_after_ms must be positive".to_owned());
        }
        if self.ping_interval_secs == 0 {
            return fail("ping_interval_secs must be positive".to_owned());
        }
        // A symbol is pasted into JSON text, so it must not be able to end a
        // JSON string.
        if let Some(symbol) = self.symbols.iter().find(|symbol| {
            symbol.is_empty() || symbol.chars().any(|c| c.is_control() || "\"\\".contains(c))
        }) {
            return fail(format!(
                "symbol {symbol:?} is empty or has a quote, backslash or control character"
            ));
        }
        if let Some(template) = self
            .subscribe_each
            .iter()
            .find(|template| !template.contains(SYMBOL_PLACEHOLDER))
        {
            return fail(format!(
                "subscribe_each entry {template:?} has no {SYMBOL_PLACEHOLDER}"
            ));
        }
        if let Some(message) = self
            .subscribe_once
            .iter()
            .find(|message| message.contains(SYMBOL_PLACEHOLDER))
        {
            return fail(format!(
                "subscribe_once entry {message:?} contains {SYMBOL_PLACEHOLDER}"
            ));
        }
        if self.symbols.is_empty() != self.subscribe_each.is_empty() {
            return fail("symbols and subscribe_each must be given together".to_owned());
        }
        if self.subscribe_once.is_empty() && self.subscribe_each.is_empty() {
            return fail("nothing to subscribe to".to_owned());
        }
        Ok(())
    }
}

impl Config {
    /// Parses and validates a configuration file.
    pub fn from_toml(text: &str) -> Result<Self, ConfigError> {
        let config: Self =
            toml::from_str(text).map_err(|error| ConfigError::new(error.to_string()))?;
        config.validate()?;
        Ok(config)
    }

    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        if self.out_dir.as_os_str().is_empty() {
            return Err(ConfigError::new("out_dir must not be empty"));
        }
        if self.queue_capacity == 0 {
            return Err(ConfigError::new("queue_capacity must be positive"));
        }
        if !(1..=19).contains(&self.zstd_level) {
            return Err(ConfigError::new("zstd_level must be between 1 and 19"));
        }
        if self.sources.is_empty() {
            return Err(ConfigError::new("at least one [[source]] is required"));
        }
        if self.sources.len() != 1
            && self
                .sources
                .iter()
                .any(|source| source.send_policy().is_some())
        {
            return Err(ConfigError::new(
                "send_policy requires exactly one [[source]]",
            ));
        }
        let mut names = BTreeSet::new();
        for source in &self.sources {
            source.validate()?;
            if !names.insert(source.name.as_str()) {
                return Err(ConfigError::new(format!(
                    "source name {} is used twice",
                    source.name
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
        out_dir = "/tmp/rec"

        [[source]]
        name = "hl-main"
        url = "wss://example.test/ws"
        symbols = ["BTC", "xyz:NVDA"]
        subscribe_each = [
            '{"method":"subscribe","subscription":{"type":"l2","c":"{symbol}"}}',
            '{"method":"subscribe","subscription":{"type":"bbo","coin":"{symbol}"}}',
        ]
        subscribe_once = ['{"method":"subscribe","subscription":{"type":"fastAssetCtxs"}}']
    "#;

    fn error_of(text: &str) -> String {
        Config::from_toml(text).unwrap_err().to_string()
    }

    #[test]
    fn example_sources_and_subscriptions() {
        let config = Config::from_toml(include_str!("../example.toml")).expect("valid example");
        assert!(config.out_dir.is_relative());
        let [hl, books, trades] = config.sources.as_slice() else {
            panic!("expected three example sources");
        };

        assert_eq!(hl.name, "hl");
        assert_eq!(
            hl.subscriptions(),
            [
                r#"{"method":"subscribe","subscription":{"type":"fastAssetCtxs"}}"#,
                r#"{"method":"subscribe","subscription":{"type":"l2","c":"BTC"}}"#,
                r#"{"method":"subscribe","subscription":{"type":"bbo","coin":"BTC"}}"#,
                r#"{"method":"subscribe","subscription":{"type":"trades","coin":"BTC"}}"#,
                r#"{"method":"subscribe","subscription":{"type":"activeAssetCtx","coin":"BTC"}}"#,
                r#"{"method":"subscribe","subscription":{"type":"l2","c":"ETH"}}"#,
                r#"{"method":"subscribe","subscription":{"type":"bbo","coin":"ETH"}}"#,
                r#"{"method":"subscribe","subscription":{"type":"trades","coin":"ETH"}}"#,
                r#"{"method":"subscribe","subscription":{"type":"activeAssetCtx","coin":"ETH"}}"#,
            ]
        );
        assert_eq!(books.name, "binance-public");
        assert_eq!(
            books.subscriptions(),
            [
                r#"{"method":"SUBSCRIBE","params":["btcusdt@depth5@100ms","ethusdt@depth5@100ms"],"id":2}"#,
            ]
        );
        assert_eq!(trades.name, "binance-market");
        assert_eq!(
            trades.subscriptions(),
            [
                r#"{"method":"SUBSCRIBE","params":["btcusdt@aggTrade"],"id":1}"#,
                r#"{"method":"SUBSCRIBE","params":["ethusdt@aggTrade"],"id":1}"#,
            ]
        );
    }

    #[test]
    fn parses_a_minimal_file_with_defaults() {
        let config = Config::from_toml(MINIMAL).unwrap();
        assert_eq!(config.out_dir, PathBuf::from("/tmp/rec"));
        assert_eq!((config.queue_capacity, config.zstd_level), (65_536, 3));

        let source = &config.sources[0];
        assert_eq!(source.name, "hl-main");
        assert_eq!(
            (source.stale_after_ms, source.subscribe_pace_ms),
            (3_000, 50)
        );
        assert_eq!(
            (source.ping_text.as_deref(), source.ping_interval_secs),
            (None, 20)
        );
    }

    #[test]
    fn expands_templates_once_then_per_symbol() {
        let config = Config::from_toml(MINIMAL).unwrap();
        assert_eq!(
            config.sources[0].subscriptions(),
            [
                r#"{"method":"subscribe","subscription":{"type":"fastAssetCtxs"}}"#,
                r#"{"method":"subscribe","subscription":{"type":"l2","c":"BTC"}}"#,
                r#"{"method":"subscribe","subscription":{"type":"bbo","coin":"BTC"}}"#,
                r#"{"method":"subscribe","subscription":{"type":"l2","c":"xyz:NVDA"}}"#,
                r#"{"method":"subscribe","subscription":{"type":"bbo","coin":"xyz:NVDA"}}"#,
            ]
        );
    }

    #[test]
    fn rejects_unknown_fields() {
        let text = MINIMAL.replace("out_dir", "queue_capacty = 5\nout_dir");
        assert!(error_of(&text).contains("queue_capacty"));
    }

    #[test]
    fn rejects_a_file_without_sources() {
        assert!(error_of(r#"out_dir = "/tmp/rec""#).contains("at least one [[source]]"));
    }

    #[test]
    fn rejects_bad_source_names() {
        for name in ["", "HL", "a/b", "a b", ".."] {
            let text = MINIMAL.replace("hl-main", name);
            assert!(error_of(&text).contains("source name"), "name {name:?}");
        }
    }

    #[test]
    fn rejects_duplicate_source_names() {
        let second = MINIMAL.split_once("[[source]]").unwrap().1;
        let text = format!("{MINIMAL}\n[[source]]{second}");
        assert!(error_of(&text).contains("used twice"));
    }

    #[test]
    fn rejects_urls_that_are_not_websocket_urls() {
        let text = MINIMAL.replace("wss://", "https://");
        assert!(error_of(&text).contains("must start with wss:// or ws://"));
    }

    #[test]
    fn rejects_symbols_that_could_break_out_of_a_json_string() {
        for symbol in [r#"B\"TC"#, r"B\\TC", ""] {
            let text = MINIMAL.replace("\"BTC\"", &format!("\"{symbol}\""));
            assert!(error_of(&text).contains("symbol"), "symbol {symbol:?}");
        }
    }

    #[test]
    fn rejects_templates_and_symbols_that_do_not_match_up() {
        let no_placeholder = MINIMAL.replace(r#""c":"{symbol}""#, r#""c":"BTC""#);
        assert!(error_of(&no_placeholder).contains("has no {symbol}"));

        let placeholder_in_once =
            MINIMAL.replace(r#"{"type":"fastAssetCtxs"}"#, r#"{"c":"{symbol}"}"#);
        assert!(error_of(&placeholder_in_once).contains("contains {symbol}"));

        let no_symbols = MINIMAL.replace(r#"symbols = ["BTC", "xyz:NVDA"]"#, "symbols = []");
        assert!(error_of(&no_symbols).contains("given together"));
    }

    #[test]
    fn rejects_a_source_with_nothing_to_subscribe_to() {
        let text = r#"
            out_dir = "/tmp/rec"
            [[source]]
            name = "quiet"
            url = "wss://example.test/ws"
        "#;
        assert!(error_of(text).contains("nothing to subscribe to"));
    }

    #[test]
    fn rejects_out_of_range_settings() {
        let zstd = MINIMAL.replace("out_dir", "zstd_level = 40\nout_dir");
        assert!(error_of(&zstd).contains("zstd_level"));
        let queue = MINIMAL.replace("out_dir", "queue_capacity = 0\nout_dir");
        assert!(error_of(&queue).contains("queue_capacity"));
        let stale = MINIMAL.replace(
            "name = \"hl-main\"",
            "name = \"hl-main\"\nstale_after_ms = 0",
        );
        assert!(error_of(&stale).contains("stale_after_ms"));
    }

    const PUBLIC_CONFIG: &str = include_str!("../public-v1.toml");

    fn parse_public_with(edit: impl FnOnce(&mut toml::Table)) -> Result<Config, ConfigError> {
        let mut file: toml::Table = toml::from_str(PUBLIC_CONFIG).unwrap();
        let sources = file["source"].as_array_mut().unwrap();
        edit(sources[0].as_table_mut().unwrap());
        Config::from_toml(&toml::to_string(&file).unwrap())
    }

    #[test]
    fn public_policy_accepts_a_delisted_coin_subset() {
        let config = parse_public_with(|source| {
            source["symbols"].as_array_mut().unwrap().remove(0);
        })
        .unwrap();
        assert_eq!(config.sources[0].symbols.len(), 13);
        assert_eq!(config.sources[0].subscriptions().len(), 53);
    }

    #[test]
    fn public_policy_rejects_coins_outside_v1() {
        for coin in ["OTHER", "kpepe", "xyz:NVDA"] {
            let error = parse_public_with(|source| source["symbols"] = vec![coin].into())
                .unwrap_err()
                .to_string();
            assert!(error.contains("coin outside the v1 list"), "{error}");
            assert!(!error.contains(coin));
        }
    }

    #[test]
    fn public_policy_rejects_other_messages_and_noncanonical_subscriptions() {
        for text in [
            r#"{"method":"subscribe","subscription":{"type":"allMids"}}"#,
            r#"{"method":"subscribe","subscription":{"type":"userFills","user":"example"}}"#,
            r#"{"method":"post","id":1}"#,
            r#"{"method":"subscribe","subscription":{"type":"bbo","coin":"BTC","extra":1}}"#,
            r#"{"method":"subscribe","subscription":{"coin":"BTC","type":"bbo"}}"#,
            r#"{"method":"subscribe", "subscription":{"type":"bbo","coin":"BTC"}}"#,
            public_v1::PING,
        ] {
            let error = parse_public_with(|source| source["subscribe_once"] = vec![text].into())
                .unwrap_err()
                .to_string();
            assert!(error.contains("subscription allowlist"), "{error}");
            assert!(!error.contains(text));
        }

        let template = "synthetic-rejected-template";
        let error = parse_public_with(|source| source["subscribe_each"] = vec![template].into())
            .unwrap_err()
            .to_string();
        assert!(error.contains("subscribe_each"), "{error}");
        assert!(!error.contains(template));
    }

    #[test]
    fn public_policy_rejects_a_duplicate_across_expanded_subscriptions() {
        let error = parse_public_with(|source| {
            source["subscribe_once"] =
                vec![r#"{"method":"subscribe","subscription":{"type":"bbo","coin":"ETH"}}"#].into();
        })
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("duplicate expanded subscription")
        );
    }

    #[test]
    fn public_policy_requires_the_exact_url() {
        for url in [
            "wss://other.test/ws",
            "wss://api.hyperliquid.xyz:443/ws",
            "wss://user@api.hyperliquid.xyz/ws",
            "wss://api.hyperliquid.xyz/ws?extra=1",
            "wss://api.hyperliquid.xyz/ws/",
            "ws://api.hyperliquid.xyz/ws",
        ] {
            let error = parse_public_with(|source| source["url"] = url.into())
                .unwrap_err()
                .to_string();
            assert!(error.contains("url must equal"), "{error}");
            assert!(!error.contains(url));
        }
    }

    #[test]
    fn public_policy_requires_a_single_source_named_hl_public() {
        let error = parse_public_with(|source| source["name"] = "alternate".into()).unwrap_err();
        assert!(error.to_string().contains("source name must be hl-public"));

        let multi = format!(
            "{PUBLIC_CONFIG}\n[[source]]\nname = 'extra'\nurl = 'ws://example.test/ws'\nsubscribe_once = ['test']\n"
        );
        assert!(error_of(&multi).contains("exactly one [[source]]"));
    }

    #[test]
    fn public_policy_requires_the_exact_ping() {
        let missing = parse_public_with(|source| {
            source.remove("ping_text");
        })
        .unwrap_err();
        assert!(missing.to_string().contains("ping_text"));
        let other = r#"{"method": "ping"}"#;
        let error = parse_public_with(|source| source["ping_text"] = other.into())
            .unwrap_err()
            .to_string();
        assert!(error.contains("ping_text"));
        assert!(!error.contains(other));
    }

    #[test]
    fn public_policy_checks_the_ping_interval() {
        let error =
            parse_public_with(|source| source["ping_interval_secs"] = 0.into()).unwrap_err();
        assert!(error.to_string().contains("ping_interval_secs"));
        for interval in [1, i64::MAX] {
            parse_public_with(|source| source["ping_interval_secs"] = interval.into()).unwrap();
        }
    }

    #[test]
    fn config_rejects_an_unknown_send_policy() {
        let error =
            parse_public_with(|source| source["send_policy"] = "unknown".into()).unwrap_err();
        assert!(error.to_string().contains("unknown send_policy"));
    }
}
