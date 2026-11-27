//! Hyperliquid payloads.
//!
//! Decoding has two steps, described in
//! `docs/rfc/0003-hyperliquid-market-data-decoding.md`:
//!
//! 1. [`decode_ws_frame`] turns the text of one WebSocket frame into a
//!    [`WsFrame`], a typed and faithful record of the message.
//! 2. [`market_events`] turns a frame into [`hl_types::MarketEvent`]s, given
//!    a coin resolver and the local receive time.
//!
//! Decoded so far: the book channels (`l2`, `l2Book`), `bbo` and `trades`.
//! Other channels come back as [`WsFrame::Other`].
//!
//! The official archives have their own entry points:
//! [`decode_archive_l2_line`] and [`decode_fill_block`].

mod archive;
mod cutoff;
mod events;
mod identity;
mod ws;

use core::fmt;

use hl_types::ParseFixedError;

pub use archive::{
    ArchiveBook, FillBlock, Liquidation, MarketKind, NodeFill, decode_archive_l2_line,
    decode_fill_block, trade_events,
};
pub use cutoff::TradeCutoff;
pub use events::{UnknownCoin, market_events};
pub use identity::{Network, dex, instrument_id};
pub use ws::{WsBbo, WsBook, WsBookDiff, WsFrame, WsTrade, decode_ws_frame};

/// Why a payload could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// The payload is not JSON, or not JSON of the expected shape. The text
    /// is the parser's message.
    Json(String),
    /// A decimal field could not be parsed exactly.
    Number {
        /// Name of the field in the venue's JSON.
        field: &'static str,
        /// The text that was found.
        text: String,
        /// What is wrong with it.
        error: ParseFixedError,
    },
    /// A field holds a value the format does not allow.
    Invalid {
        /// Name of the field in the venue's JSON.
        field: &'static str,
        /// The text that was found.
        text: String,
    },
    /// A compressed payload could not be unpacked. The text says which step
    /// failed.
    Compressed(&'static str),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(message) => write!(f, "unexpected JSON: {message}"),
            Self::Number { field, text, error } => {
                write!(
                    f,
                    "field {field}: {text:?} is not a usable number ({error})"
                )
            }
            Self::Invalid { field, text } => write!(f, "field {field}: unexpected value {text:?}"),
            Self::Compressed(step) => write!(f, "compressed payload: {step}"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// A 20-byte account address (HL-52).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Address(pub [u8; 20]);

impl fmt::Display for Address {
    /// Formats as `0x` followed by 40 lower-case hex digits (HL-52).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("0x")?;
        self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
    }
}

/// Parses `0x` followed by exactly `2 * N` hex digits.
fn hex_bytes<const N: usize>(field: &'static str, text: &str) -> Result<[u8; N], DecodeError> {
    let invalid = || DecodeError::Invalid {
        field,
        text: text.to_owned(),
    };
    let digits = text.strip_prefix("0x").ok_or_else(invalid)?.as_bytes();
    if digits.len() != 2 * N {
        return Err(invalid());
    }
    let nibble = |digit: u8| {
        char::from(digit)
            .to_digit(16)
            .and_then(|value| u8::try_from(value).ok())
    };
    let mut bytes = [0_u8; N];
    let (pairs, _) = digits.as_chunks::<2>();
    for (byte, pair) in bytes.iter_mut().zip(pairs) {
        let (high, low) = nibble(pair[0]).zip(nibble(pair[1])).ok_or_else(invalid)?;
        *byte = (high << 4) | low;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_prints_addresses() {
        let text = "0x31dea2516beee92135b96f464eeec3cf292a13f2";
        let address = Address(hex_bytes("users", text).unwrap());
        assert_eq!(address.0[0], 0x31);
        assert_eq!(address.0[19], 0xf2);
        assert_eq!(address.to_string(), text);
        let upper: [u8; 20] = hex_bytes("users", &text.to_uppercase().replace("0X", "0x")).unwrap();
        assert_eq!(upper, address.0, "hex digits may be upper case");
    }

    #[test]
    fn rejects_malformed_hex() {
        let cases = [
            "31dea2516beee92135b96f464eeec3cf292a13f2", // no prefix
            "0x31dea2516beee92135b96f464eeec3cf292a13", // too short
            "0x31dea2516beee92135b96f464eeec3cf292a13f200", // too long
            "0x31dea2516beee92135b96f464eeec3cf292a13zz", // not hex
            "0x+1dea2516beee92135b96f464eeec3cf292a13f2", // sign is not a digit
            "",
        ];
        for text in cases {
            let result: Result<[u8; 20], _> = hex_bytes("users", text);
            assert_eq!(
                result,
                Err(DecodeError::Invalid {
                    field: "users",
                    text: text.to_owned()
                }),
                "{text:?}"
            );
        }
    }
}
