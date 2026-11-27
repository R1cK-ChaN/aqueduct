//! Hyperliquid instrument naming (RFC 0009 D1–D2).

use hl_types::InstrumentId;

/// The network used to identify a Hyperliquid instrument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Network {
    /// The production network.
    Mainnet,
    /// The public test network.
    Testnet,
}

impl Network {
    const ALL: [Self; 2] = [Self::Mainnet, Self::Testnet];

    /// The venue name used in instrument ids and stored rows.
    pub const fn venue(self) -> &'static str {
        match self {
            Self::Mainnet => "hyperliquid-mainnet",
            Self::Testnet => "hyperliquid-testnet",
        }
    }

    /// Looks up the exact venue name written by [`Self::venue`].
    pub fn from_venue(venue: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|network| network.venue() == venue)
    }

    /// The spelling used in configuration, CLI options and session files.
    pub const fn short_name(self) -> &'static str {
        match self {
            Self::Mainnet => "mainnet",
            Self::Testnet => "testnet",
        }
    }

    /// Looks up the exact spelling written by [`Self::short_name`].
    pub fn from_short_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|network| network.short_name() == name)
    }

    /// Identifies the network from the host in a recording header (HL-40).
    pub fn from_url(url: &str) -> Option<Self> {
        let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
        match rest.split(['/', ':', '?']).next()? {
            "api.hyperliquid.xyz" => Some(Self::Mainnet),
            "api.hyperliquid-testnet.xyz" => Some(Self::Testnet),
            _ => None,
        }
    }
}

/// Derives the stable id from the network's venue name and the exact coin text.
pub fn instrument_id(network: Network, coin: &str) -> InstrumentId {
    InstrumentId::derive(network.venue(), coin)
}

/// Extracts the dex prefix from a coin name, when present (HL-55).
pub fn dex(coin: &str) -> Option<&str> {
    coin.split_once(':').map(|(prefix, _)| prefix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_names_round_trip_in_their_own_format() {
        for (network, venue, name) in [
            (Network::Mainnet, "hyperliquid-mainnet", "mainnet"),
            (Network::Testnet, "hyperliquid-testnet", "testnet"),
        ] {
            assert_eq!(network.venue(), venue);
            assert_eq!(Network::from_venue(venue), Some(network));
            assert_eq!(network.short_name(), name);
            assert_eq!(Network::from_short_name(name), Some(network));
            assert_eq!(Network::from_venue(name), None);
            assert_eq!(Network::from_short_name(venue), None);
        }
        assert_eq!(Network::from_venue("unknown"), None);
        assert_eq!(Network::from_short_name("unknown"), None);
    }

    #[test]
    fn recognises_recording_hosts() {
        for (url, expected) in [
            ("wss://api.hyperliquid.xyz/ws", Some(Network::Mainnet)),
            ("wss://api.hyperliquid.xyz:443/ws", Some(Network::Mainnet)),
            (
                "wss://api.hyperliquid-testnet.xyz/ws",
                Some(Network::Testnet),
            ),
            ("api.hyperliquid-testnet.xyz?query", Some(Network::Testnet)),
            ("wss://fstream.binance.com/public/ws", None),
            ("wss://api.hyperliquid.xyz.example.com/ws", None),
            ("", None),
        ] {
            assert_eq!(Network::from_url(url), expected, "{url}");
        }
    }

    #[test]
    fn ids_match_rfc_0009() {
        assert_eq!(instrument_id(Network::Mainnet, "BTC").get(), 0x619a_6e2d);
        assert_eq!(
            instrument_id(Network::Mainnet, "xyz:NVDA").get(),
            0x0b98_63d6
        );
        assert_eq!(instrument_id(Network::Testnet, "BTC").get(), 0x6704_e12a);
    }

    #[test]
    fn dex_is_the_prefix_before_the_first_colon() {
        assert_eq!(dex("BTC"), None);
        assert_eq!(dex("xyz:NVDA"), Some("xyz"));
        assert_eq!(dex("one:two:three"), Some("one"));
    }
}
