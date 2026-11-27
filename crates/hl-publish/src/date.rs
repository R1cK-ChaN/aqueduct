//! UTC calendar arguments. Gap bounds use Unix nanoseconds even at a day boundary.

#[derive(Debug, Clone)]
pub(crate) struct Day {
    iso: String,
    year: u32,
    month: u32,
    day: u32,
}

const fn month_length(year: u32, month: u32) -> u32 {
    match month {
        4 | 6 | 9 | 11 => 30,
        2 if year.is_multiple_of(400) || (year.is_multiple_of(4) && !year.is_multiple_of(100)) => {
            29
        }
        2 => 28,
        _ => 31,
    }
}

impl Day {
    pub(crate) fn parse(iso: &str) -> Option<Self> {
        if iso.len() != 10 || !iso.is_ascii() || &iso[4..5] != "-" || &iso[7..8] != "-" {
            return None;
        }
        let number = |s: &str| {
            s.bytes()
                .all(|b| b.is_ascii_digit())
                .then(|| s.parse().ok())
                .flatten()
        };
        let (year, month, day) = (number(&iso[..4])?, number(&iso[5..7])?, number(&iso[8..])?);
        if year == 0
            || !(1..=12).contains(&month)
            || !(1..=month_length(year, month)).contains(&day)
        {
            return None;
        }
        Some(Self {
            iso: iso.to_owned(),
            year,
            month,
            day,
        })
    }

    pub(crate) fn iso(&self) -> &str {
        &self.iso
    }

    pub(crate) fn compact(&self) -> String {
        self.iso.replace('-', "")
    }

    pub(crate) fn previous(&self) -> Option<String> {
        let (mut y, mut m, mut d) = (self.year, self.month, self.day);
        if d == 1 {
            if m == 1 {
                if y == 1 {
                    return None;
                }
                y -= 1;
                m = 12;
            } else {
                m -= 1;
            }
            d = month_length(y, m);
        } else {
            d -= 1;
        }
        Some(format!("{y:04}-{m:02}-{d:02}"))
    }

    pub(crate) fn start_ns(&self) -> i128 {
        // Completed Gregorian years and months since 0001-01-01.
        let y = self.year - 1;
        let elapsed = y * 365 + y / 4 - y / 100
            + y / 400
            + (1..self.month)
                .map(|m| month_length(self.year, m))
                .sum::<u32>()
            + self.day
            - 1;
        (i128::from(elapsed) - 719_162) * 86_400_000_000_000
    }

    pub(crate) fn contains_utc_time(&self, value: &str) -> bool {
        let Some(time) = value
            .strip_prefix(self.iso())
            .and_then(|s| s.strip_prefix('T'))
            .and_then(|s| s.strip_suffix('Z'))
        else {
            return false;
        };
        let (hms, fraction) = time
            .split_once('.')
            .map_or((time, None), |(hms, f)| (hms, Some(f)));
        let [h1, h2, b':', m1, m2, b':', s1, s2] = *hms.as_bytes() else {
            return false;
        };
        let valid = |a: u8, b: u8, max: u8| {
            a.is_ascii_digit() && b.is_ascii_digit() && (a - b'0') * 10 + b - b'0' <= max
        };
        valid(h1, h2, 23)
            && valid(m1, m2, 59)
            && valid(s1, s2, 59)
            && fraction
                .is_none_or(|f| (1..=9).contains(&f.len()) && f.bytes().all(|b| b.is_ascii_digit()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_dates_and_coverage_instants() {
        assert_eq!(Day::parse("1970-01-01").expect("epoch").start_ns(), 0);
        assert_eq!(
            Day::parse("2000-03-01")
                .expect("leap year")
                .previous()
                .as_deref(),
            Some("2000-02-29")
        );
        assert_eq!(
            Day::parse("1900-03-01")
                .expect("century")
                .previous()
                .as_deref(),
            Some("1900-02-28")
        );
        assert!(Day::parse("2026-02-29").is_none());
        assert!(Day::parse("+026-02-01").is_none());
        let day = Day::parse("2026-01-01").expect("day");
        assert_eq!(day.previous().as_deref(), Some("2025-12-31"));
        assert!(day.contains_utc_time("2026-01-01T23:59:59.123456789Z"));
        for bad in [
            "2025-12-31T23:59:59Z",
            "2026-01-01T24:00:00Z",
            "2026-01-01T12:00:00.Z",
        ] {
            assert!(!day.contains_utc_time(bad));
        }
    }
}
