//! Send rules for public dataset v1 (RFC 0010 D2, D3 and D5).
//!
//! The recorder and publisher can both use [`allows`] to check a text frame.
//! Message shapes are registered in HL-43, HL-45, HL-47, HL-91 and HL-92
//! in `docs/venues/hyperliquid.md`.

use std::collections::BTreeSet;
use std::sync::LazyLock;
use std::time::Duration;

use crate::SYMBOL_PLACEHOLDER;

/// Name selecting this version of the send policy in a source config.
pub const NAME: &str = "hl-public-v1";
/// Source name required by RFC 0010 D3.
pub const SOURCE_NAME: &str = "hl-public";
/// Exact URL required by this policy (HL-40, HL-41).
pub const URL: &str = "wss://api.hyperliquid.xyz/ws";
/// Fixed coin list in RFC 0010 D2 order.
pub const COINS: [&str; 14] = [
    "BTC", "ETH", "SOL", "HYPE", "XRP", "ZEC", "DOGE", "SUI", "AAVE", "NEAR", "TAO", "PUMP", "XPL",
    "kPEPE",
];
/// Exact ping text permitted by this policy (HL-43).
pub const PING: &str = r#"{"method":"ping"}"#;
/// Rolling window for this source's D2 limits.
pub const WINDOW: Duration = Duration::from_secs(60);
/// Maximum connection attempts in one [`WINDOW`].
pub const MAX_ATTEMPTS: usize = 2;
/// Maximum outbound texts in one [`WINDOW`], including pings.
pub const MAX_MESSAGES: usize = 300;

const SUBSCRIBE_EACH: [&str; 4] = [
    r#"{"method":"subscribe","subscription":{"type":"l2","c":"{symbol}"}}"#,
    r#"{"method":"subscribe","subscription":{"type":"bbo","coin":"{symbol}"}}"#,
    r#"{"method":"subscribe","subscription":{"type":"trades","coin":"{symbol}"}}"#,
    r#"{"method":"subscribe","subscription":{"type":"activeAssetCtx","coin":"{symbol}"}}"#,
];
const SHARED_SUBSCRIPTION: &str =
    r#"{"method":"subscribe","subscription":{"type":"fastAssetCtxs"}}"#;

static ALLOWLIST: LazyLock<BTreeSet<String>> = LazyLock::new(|| {
    let mut texts = BTreeSet::from([SHARED_SUBSCRIPTION.to_owned(), PING.to_owned()]);
    for coin in COINS {
        for template in SUBSCRIBE_EACH {
            texts.insert(template.replace(SYMBOL_PLACEHOLDER, coin));
        }
    }
    texts
});

/// Returns true exactly when the text matches an allowlist member byte for byte.
pub fn allows(text: &str) -> bool {
    ALLOWLIST.contains(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;

    #[test]
    fn public_config_expands_to_all_57_subscriptions_and_the_rules_allow_all_58_texts() {
        let config = Config::from_toml(include_str!("../public-v1.toml")).unwrap();
        assert!(config.out_dir.is_relative());
        assert_eq!(config.zstd_level, 3);
        let [source] = config.sources.as_slice() else {
            panic!("expected one public source");
        };
        assert_eq!(source.name, SOURCE_NAME);
        assert_eq!(source.send_policy(), Some(NAME));
        assert_eq!(source.url, URL);
        assert_eq!(source.symbols, COINS);
        assert_eq!(source.ping_text.as_deref(), Some(PING));

        let subscriptions = source.subscriptions();
        assert_eq!(subscriptions.len(), 57);
        let mut texts = BTreeSet::from_iter(subscriptions);
        assert!(texts.insert(PING.to_owned()));
        assert_eq!(texts.len(), 58);
        assert_eq!(texts, *ALLOWLIST);
        assert!(texts.iter().all(|text| allows(text)));
    }

    #[test]
    fn rejects_signing_fields_unsubscribe_duplicate_keys_and_empty_text() {
        for (case, text) in [
            ("signature", r#"{"method":"ping","signature":"synthetic"}"#),
            ("nonce", r#"{"method":"ping","nonce":1}"#),
            (
                "vault address",
                r#"{"method":"ping","vaultAddress":"synthetic"}"#,
            ),
            (
                "private key",
                r#"{"method":"ping","privateKey":"synthetic"}"#,
            ),
            (
                "unsubscribe",
                r#"{"method":"unsubscribe","subscription":{"type":"l2","c":"BTC"}}"#,
            ),
            ("duplicate key", r#"{"method":"ping","method":"ping"}"#),
            ("empty", ""),
        ] {
            assert!(!allows(text), "{case}");
        }
    }
}
