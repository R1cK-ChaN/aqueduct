//! The single time type used for both exchange time and local time.

use core::ops::Add;
use core::time::Duration;

const NANOS_PER_MILLI: i64 = 1_000_000;

/// A point in time: nanoseconds since the Unix epoch, UTC.
///
/// The same type serves two roles, told apart by field name:
///
/// - `ts_exch` is exchange time, the block time. WebSocket channels and archive
///   rows use milliseconds, while fills envelopes retain submillisecond digits (HL-95).
/// - `ts_local` is the time at which the strategy process sees an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Timestamp(i64);

impl Timestamp {
    /// 1970-01-01T00:00:00Z.
    pub const EPOCH: Self = Self(0);

    /// Builds a timestamp from nanoseconds since the Unix epoch.
    pub const fn from_nanos(nanos: i64) -> Self {
        Self(nanos)
    }

    /// Builds a timestamp from milliseconds since the Unix epoch.
    pub const fn from_millis(millis: i64) -> Self {
        Self(millis * NANOS_PER_MILLI)
    }

    /// Nanoseconds since the Unix epoch.
    pub const fn as_nanos(self) -> i64 {
        self.0
    }

    /// Whole milliseconds since the Unix epoch, rounded down.
    pub const fn as_millis(self) -> i64 {
        self.0.div_euclid(NANOS_PER_MILLI)
    }

    /// Signed nanoseconds from `earlier` to `self`. Negative when `self` is
    /// the earlier of the two, which happens for `ts_local - ts_exch` when
    /// the local clock runs behind.
    pub const fn nanos_since(self, earlier: Self) -> i64 {
        self.0 - earlier.0
    }
}

impl Add<Duration> for Timestamp {
    type Output = Self;

    /// # Panics
    ///
    /// Panics when the result is outside the representable range, about
    /// 292 years either side of the epoch.
    fn add(self, rhs: Duration) -> Self {
        let nanos = i64::try_from(rhs.as_nanos()).expect("duration fits in i64 nanoseconds");
        Self(self.0 + nanos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_between_millis_and_nanos() {
        let ts = Timestamp::from_millis(1_795_000_000_123);
        assert_eq!(ts.as_nanos(), 1_795_000_000_123_000_000);
        assert_eq!(ts.as_millis(), 1_795_000_000_123);
        assert_eq!(Timestamp::from_nanos(1_999_999).as_millis(), 1);
        assert_eq!(Timestamp::from_nanos(-1).as_millis(), -1);
    }

    #[test]
    fn orders_by_time_and_measures_latency() {
        let exch = Timestamp::from_millis(1_000);
        let local = exch + Duration::from_millis(254);
        assert!(local > exch);
        assert_eq!(local.nanos_since(exch), 254_000_000);
        assert_eq!(exch.nanos_since(local), -254_000_000);
    }
}
