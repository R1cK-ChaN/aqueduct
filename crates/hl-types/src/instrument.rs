//! Instrument identity.

use core::fmt;

/// The stable instrument number carried by events, orders and database rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstrumentId(u32);

impl InstrumentId {
    /// Wraps a raw instrument number.
    pub const fn new(raw: u32) -> Self {
        Self(raw)
    }

    /// The raw instrument number.
    pub const fn get(self) -> u32 {
        self.0
    }

    /// The id of the instrument that `venue` calls `symbol`.
    ///
    /// The id is the 32-bit FNV-1a hash of the UTF-8 text `"{venue}:{symbol}"`,
    /// so it needs no stored state and is the same on every machine and after
    /// every rebuild. Two names can collide; whoever assigns ids must check
    /// for that and refuse to continue (see `docs/rfc/0004`).
    pub fn derive(venue: &str, symbol: &str) -> Self {
        const OFFSET_BASIS: u32 = 0x811c_9dc5;
        const PRIME: u32 = 0x0100_0193;
        let bytes = venue
            .bytes()
            .chain(core::iter::once(b':'))
            .chain(symbol.bytes());
        Self(bytes.fold(OFFSET_BASIS, |hash, byte| {
            (hash ^ u32::from(byte)).wrapping_mul(PRIME)
        }))
    }
}

impl fmt::Display for InstrumentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_matches_the_published_fnv1a_test_vectors() {
        // FNV-1a 32-bit of "a" and "foobar", with the separator accounted
        // for by hashing the whole text through `derive`'s fold.
        let hash = |text: &str| {
            text.bytes().fold(0x811c_9dc5_u32, |hash, byte| {
                (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
            })
        };
        assert_eq!(hash(""), 0x811c_9dc5);
        assert_eq!(hash("a"), 0xe40c_292c);
        assert_eq!(hash("foobar"), 0xbf9c_f968);
        assert_eq!(InstrumentId::derive("foo", "bar").get(), hash("foo:bar"));
    }
}
