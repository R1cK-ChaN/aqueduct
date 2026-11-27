//! Receive timestamps and the UTC hour a timestamp falls in.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

const NANOS_PER_HOUR: i64 = 3_600 * 1_000_000_000;

/// The two clocks read at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    /// Wall clock, nanoseconds since the Unix epoch. Can step when the system
    /// clock is corrected.
    pub wall_ns: i64,
    /// Monotonic clock, nanoseconds since the recorder process started.
    pub mono_ns: u64,
}

/// Reads both clocks. Created once at process start.
#[derive(Debug, Clone, Copy)]
pub struct Clock {
    started: Instant,
    start_wall_ns: i64,
}

fn wall_ns_now() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(since) => i64::try_from(since.as_nanos()).unwrap_or(i64::MAX),
        Err(before) => -i64::try_from(before.duration().as_nanos()).unwrap_or(i64::MAX),
    }
}

impl Clock {
    /// Starts the monotonic clock at zero.
    pub fn start() -> Self {
        Self {
            started: Instant::now(),
            start_wall_ns: wall_ns_now(),
        }
    }

    /// Reads both clocks now.
    pub fn now(&self) -> Stamp {
        let mono_ns = u64::try_from(self.started.elapsed().as_nanos()).unwrap_or(u64::MAX);
        Stamp {
            wall_ns: wall_ns_now(),
            mono_ns,
        }
    }

    /// Wall clock at the moment the monotonic clock was zero.
    pub fn start_wall_ns(&self) -> i64 {
        self.start_wall_ns
    }
}

/// A UTC hour, counted from the Unix epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct UtcHour(i64);

impl UtcHour {
    /// The hour that contains `wall_ns`.
    pub fn containing(wall_ns: i64) -> Self {
        Self(wall_ns.div_euclid(NANOS_PER_HOUR))
    }

    /// `(year, month, day, hour)` in UTC.
    fn civil(self) -> (i64, i64, i64, i64) {
        // Days-to-civil conversion for the proleptic Gregorian calendar,
        // after Howard Hinnant's `civil_from_days`.
        let days = self.0.div_euclid(24);
        let shifted = days + 719_468;
        let era = shifted.div_euclid(146_097);
        let day_of_era = shifted.rem_euclid(146_097);
        let year_of_era =
            (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let month_index = (5 * day_of_year + 2) / 153;
        let day = day_of_year - (153 * month_index + 2) / 5 + 1;
        let month = if month_index < 10 {
            month_index + 3
        } else {
            month_index - 9
        };
        let year = year_of_era + era * 400 + i64::from(month <= 2);
        (year, month, day, self.0.rem_euclid(24))
    }

    /// The date as `YYYYMMDD`.
    pub fn date(self) -> String {
        let (year, month, day, _) = self.civil();
        format!("{year:04}{month:02}{day:02}")
    }

    /// The hour as `YYYYMMDDTHH`.
    pub fn label(self) -> String {
        let (year, month, day, hour) = self.civil();
        format!("{year:04}{month:02}{day:02}T{hour:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NANOS_PER_SEC: i64 = 1_000_000_000;

    #[test]
    fn labels_known_instants() {
        let cases = [
            (0, "19700101T00"),
            (3_599, "19700101T00"),
            (3_600, "19700101T01"),
            (951_782_400, "20000229T00"), // leap day
            (951_868_799, "20000229T23"),
            (951_868_800, "20000301T00"),
            (1_795_515_300, "20261124T10"), // 2026-11-24 10:15:00 UTC
            (1_798_761_599, "20261231T23"),
            (1_798_761_600, "20270101T00"),
            (-1, "19691231T23"),
        ];
        for (unix_seconds, expected) in cases {
            let hour = UtcHour::containing(unix_seconds * NANOS_PER_SEC);
            assert_eq!(hour.label(), expected, "at {unix_seconds}");
            assert_eq!(hour.date(), &expected[..8]);
        }
    }

    #[test]
    fn hours_order_by_time() {
        let earlier = UtcHour::containing(3_599 * NANOS_PER_SEC);
        let later = UtcHour::containing(3_600 * NANOS_PER_SEC);
        assert!(earlier < later);
        assert_eq!(earlier, UtcHour::containing(0));
    }

    #[test]
    fn clock_reads_are_consistent() {
        let clock = Clock::start();
        let first = clock.now();
        let second = clock.now();
        assert!(second.mono_ns >= first.mono_ns);
        assert!(first.wall_ns >= clock.start_wall_ns());
        // Later than 2026-01-01, so the wall clock is plausibly set.
        assert!(first.wall_ns > 1_767_225_600 * NANOS_PER_SEC);
    }
}
