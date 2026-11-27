//! Data-quality statistics and daily checks.
//!
//! `run_day` recomputes `hl.dq_hourly` for one UTC day, runs every check in
//! [`CHECKS`] and stores the results in `hl.dq_checks`. The checks and their
//! thresholds are explained in `docs/rfc/0005-data-quality-checks.md`.
//!
//! Every query takes the day as the server-side parameter `{day:Date}`.

use clickhouse::Row;
use serde::{Deserialize, Serialize};

use crate::db::{Db, DbError};

/// The day bounds every query below starts from.
macro_rules! day_bounds {
    () => {
        "WITH toDateTime64({day:Date}, 3, 'UTC') AS day_start, day_start + INTERVAL 1 DAY AS day_end "
    };
}

/// Gaps between consecutive rows of a group, in milliseconds, counting from
/// the start of the hour and to its end. `ts` is the group's time column.
macro_rules! hour_gaps {
    ($ts:literal) => {
        concat!(
            "arrayDifference(arrayConcat([toUnixTimestamp64Milli(toDateTime64(hour, 3))], ",
            "arraySort(groupArray(toUnixTimestamp64Milli(",
            $ts,
            "))), [toUnixTimestamp64Milli(toDateTime64(hour, 3)) + 3600000]))"
        )
    };
}

const DQ_COLUMNS: &str = "INSERT INTO hl.dq_hourly (hour, table_name, source, instrument_id, rows, \
     max_gap_ms, gaps_over_5s, invalid, short_books, computed_at) ";

/// A book is invalid when it is crossed, has a non-positive price or size,
/// or is not strictly ordered best price first.
const INVALID_BOOK: &str = "(length(bid_px) > 0 AND length(ask_px) > 0 AND bid_px[1] >= ask_px[1]) \
     OR NOT arrayAll(x -> x > 0, arrayConcat(bid_px, ask_px, bid_sz, ask_sz)) \
     OR NOT arrayAll((a, b) -> a > b, arrayPopBack(bid_px), arrayPopFront(bid_px)) \
     OR NOT arrayAll((a, b) -> a < b, arrayPopBack(ask_px), arrayPopFront(ask_px))";

fn hourly_l2() -> String {
    format!(
        "{DQ_COLUMNS}{} SELECT toStartOfHour(ts_exch) AS hour, 'l2_snapshot', source, instrument_id, count(), \
         toUInt64(arrayMax({gaps})), toUInt64(arrayCount(gap -> gap > 5000, {gaps})), \
         countIf({INVALID_BOOK}), countIf(length(bid_px) < 20 OR length(ask_px) < 20), now64(3) \
         FROM hl.l2_snapshot FINAL WHERE ts_exch >= day_start AND ts_exch < day_end \
         GROUP BY hour, source, instrument_id",
        day_bounds!(),
        gaps = hour_gaps!("ts_exch"),
    )
}

fn hourly_fills() -> String {
    format!(
        "{DQ_COLUMNS}{} SELECT hour, 'fills', 'node_fills', instrument_id, sum(rows), \
         toUInt64(arrayMax({gaps})), toUInt64(arrayCount(gap -> gap > 5000, {gaps})), \
         countIf(bad), 0, now64(3) \
         FROM (SELECT toStartOfHour(ts_exch) AS hour, instrument_id, ts_exch, tid, count() AS rows, \
               NOT (count() = 2 AND countIf(crossed) = 1 AND countIf(side = 'B') = 1 \
                    AND countIf(px <= 0 OR sz <= 0) = 0) AS bad \
               FROM hl.fills FINAL WHERE ts_exch >= day_start AND ts_exch < day_end \
               GROUP BY hour, instrument_id, ts_exch, tid) \
         GROUP BY hour, instrument_id",
        day_bounds!(),
        gaps = hour_gaps!("ts_exch"),
    )
}

fn hourly_bbo() -> String {
    format!(
        "{DQ_COLUMNS}{} SELECT toStartOfHour(ts_exch) AS hour, 'bbo', source, instrument_id, count(), \
         toUInt64(arrayMax({gaps})), toUInt64(arrayCount(gap -> gap > 5000, {gaps})), \
         countIf(bid_px < 0 OR ask_px < 0 OR bid_sz < 0 OR ask_sz < 0 \
                 OR (bid_px > 0 AND ask_px > 0 AND bid_px >= ask_px)), \
         countIf(bid_px = 0 OR ask_px = 0), now64(3) \
         FROM hl.bbo FINAL WHERE ts_exch >= day_start AND ts_exch < day_end \
         GROUP BY hour, source, instrument_id",
        day_bounds!(),
        gaps = hour_gaps!("ts_exch"),
    )
}

fn hourly_ws_trades() -> String {
    format!(
        "{DQ_COLUMNS}{} SELECT toStartOfHour(ts_exch) AS hour, 'ws_trades', source, instrument_id, count(), \
         toUInt64(arrayMax({gaps})), toUInt64(arrayCount(gap -> gap > 5000, {gaps})), \
         countIf(px <= 0 OR sz <= 0), 0, now64(3) \
         FROM hl.ws_trades FINAL WHERE ts_exch >= day_start AND ts_exch < day_end \
         GROUP BY hour, source, instrument_id",
        day_bounds!(),
        gaps = hour_gaps!("ts_exch"),
    )
}

/// Whether a failed check is an alarm or only reported.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CheckKind {
    /// Fails when violations exceed this share of the rows compared.
    Alarm {
        /// Largest acceptable `violations / compared`.
        max_rate: f64,
    },
    /// Always passes; the numbers are for reading.
    Info,
}

impl CheckKind {
    /// Whether a result with these counts passes.
    pub fn passes(self, compared: u64, violations: u64) -> bool {
        match self {
            Self::Info => true,
            Self::Alarm { .. } if compared == 0 => violations == 0,
            Self::Alarm { max_rate } => violations as f64 / compared as f64 <= max_rate,
        }
    }

    fn code(self) -> i8 {
        match self {
            Self::Alarm { .. } => 1,
            Self::Info => 2,
        }
    }

    fn max_rate(self) -> f64 {
        match self {
            Self::Alarm { max_rate } => max_rate,
            Self::Info => 1.0,
        }
    }
}

/// One daily check: a query returning `subject, instrument_id, compared,
/// violations, detail` rows.
#[derive(Debug)]
pub struct Check {
    /// Name stored in `hl.dq_checks.check_name`.
    pub name: &'static str,
    /// Alarm threshold, or info.
    pub kind: CheckKind,
    /// What the check means, for the report.
    pub description: &'static str,
    sql: fn() -> String,
}

fn check_validity(table: &str) -> String {
    format!(
        "SELECT source AS subject, instrument_id, sum(rows) AS compared, sum(invalid) AS violations, '' AS detail \
         FROM hl.dq_hourly FINAL \
         WHERE table_name = '{table}' AND hour >= toDate({{day:Date}}) AND hour < toDate({{day:Date}}) + 1 \
         GROUP BY source, instrument_id"
    )
}

fn check_l2_validity() -> String {
    check_validity("l2_snapshot")
}

fn check_bbo_validity() -> String {
    check_validity("bbo")
}

fn check_fill_pairs() -> String {
    check_validity("fills")
}

fn check_trades_vs_archive_books() -> String {
    format!(
        "{} SELECT 'node_fills' AS subject, instrument_id, count() AS compared, \
         countIf(wrong_before AND wrong_after) AS violations, \
         toJSONString(map('wrong_side_of_book_before', countIf(wrong_before))) AS detail \
         FROM (SELECT t.instrument_id AS instrument_id, \
               if(t.side = 'A', t.px >= p.ask_px[1], t.px <= p.bid_px[1]) AS wrong_before, \
               if(t.side = 'A', t.px >= n.ask_px[1], t.px <= n.bid_px[1]) AS wrong_after \
               FROM (SELECT instrument_id, ts_exch, side, px FROM hl.fills FINAL \
                     WHERE crossed AND ts_exch >= day_start AND ts_exch < day_end) AS t \
               ASOF INNER JOIN (SELECT instrument_id, ts_exch, bid_px, ask_px FROM hl.l2_snapshot FINAL \
                     WHERE source = 's3_archive' AND ts_exch >= day_start - INTERVAL 1 MINUTE AND ts_exch < day_end) AS p \
                 ON t.instrument_id = p.instrument_id AND t.ts_exch > p.ts_exch \
               ASOF INNER JOIN (SELECT instrument_id, ts_exch, bid_px, ask_px FROM hl.l2_snapshot FINAL \
                     WHERE source = 's3_archive' AND ts_exch >= day_start AND ts_exch < day_end + INTERVAL 1 MINUTE) AS n \
                 ON t.instrument_id = n.instrument_id AND t.ts_exch <= n.ts_exch) \
         GROUP BY instrument_id",
        day_bounds!()
    )
}

fn check_ws_books_vs_bbo() -> String {
    format!(
        "{} SELECT 'ws_public' AS subject, l.instrument_id AS instrument_id, count() AS compared, \
         countIf(l.bid_px[1] != b.bid_px OR l.bid_sz[1] != b.bid_sz \
                 OR l.ask_px[1] != b.ask_px OR l.ask_sz[1] != b.ask_sz) AS violations, '' AS detail \
         FROM (SELECT instrument_id, ts_exch, bid_px, bid_sz, ask_px, ask_sz FROM hl.l2_snapshot FINAL \
               WHERE source = 'ws_public' AND ts_exch >= day_start AND ts_exch < day_end) AS l \
         INNER JOIN (SELECT instrument_id, ts_exch, bid_px, bid_sz, ask_px, ask_sz FROM hl.bbo FINAL \
               WHERE source = 'ws_public' AND ts_exch >= day_start AND ts_exch < day_end) AS b \
           ON l.instrument_id = b.instrument_id AND l.ts_exch = b.ts_exch \
         GROUP BY instrument_id",
        day_bounds!()
    )
}

fn check_ws_trades_vs_fills() -> String {
    // Only the time each loaded fills file covers counts. Hourly files are cut
    // by the node's clock (HL-114), so a file's first fills can carry the previous
    // hour's block time; comparing whole UTC hours would treat that hour as
    // covered.
    format!(
        "{}, (SELECT groupArray((ts_min, ts_max)) FROM hl.ingest_log \
              WHERE source = 'node_fills' AND status = 'done' \
                AND ts_max >= day_start AND ts_min < day_end) AS fill_spans \
         SELECT 'ws_public' AS subject, w.instrument_id AS instrument_id, count() AS compared, \
         countIf(f.tid = 0 OR f.px != w.px OR f.sz != w.sz \
                 OR CAST(f.side, 'Int8') != CAST(w.aggressor, 'Int8')) AS violations, \
         toJSONString(map('not_in_official_fills', countIf(f.tid = 0))) AS detail \
         FROM (SELECT instrument_id, ts_exch, tid, aggressor, px, sz FROM hl.ws_trades FINAL \
               WHERE ts_exch >= day_start AND ts_exch < day_end \
                 AND arrayExists(span -> ts_exch >= span.1 AND ts_exch <= span.2, fill_spans)) AS w \
         LEFT JOIN (SELECT instrument_id, ts_exch, tid, side, px, sz FROM hl.fills FINAL \
               WHERE crossed AND ts_exch >= day_start AND ts_exch < day_end) AS f \
           ON w.instrument_id = f.instrument_id AND w.tid = f.tid AND w.ts_exch = f.ts_exch \
         GROUP BY instrument_id",
        day_bounds!()
    )
}

fn check_fills_vs_ws_trades() -> String {
    // A surviving trade can have an older ingest ID than its file's state.
    // Resolve the file by hash. A legacy file supplies its own run key.
    format!(
        "{}, recorded AS (
             SELECT instrument_id, ts_exch, tid, ingest_id, live FROM hl.ws_trades FINAL
             WHERE ts_exch >= day_start AND ts_exch < day_end
         ), run_spans AS (
             SELECT w.instrument_id AS instrument_id, min(w.ts_exch) AS first, max(w.ts_exch) AS last
             FROM recorded AS w
             INNER JOIN hl.ingest_log AS l ON w.ingest_id = l.ingest_id
             LEFT JOIN (SELECT sha256, source, process_start_wall_ns FROM hl.rawlog_state FINAL) AS s
               ON l.sha256 = s.sha256
             WHERE w.live = 1
             GROUP BY w.instrument_id, s.source, s.process_start_wall_ns, if(notEmpty(s.sha256), '', l.sha256)
         )
         SELECT 'node_fills' AS subject, f.instrument_id AS instrument_id, count() AS compared,
                countIf((f.instrument_id, f.ts_exch, f.tid) NOT IN
                        (SELECT instrument_id, ts_exch, tid FROM recorded)) AS violations, '' AS detail
         FROM (SELECT instrument_id, ts_exch, tid FROM hl.fills FINAL
               WHERE crossed AND ts_exch >= day_start AND ts_exch < day_end) AS f
         INNER JOIN (SELECT instrument_id, groupArray((first, last)) AS spans
                     FROM run_spans GROUP BY instrument_id) AS r ON f.instrument_id = r.instrument_id
         WHERE arrayExists(span -> f.ts_exch BETWEEN span.1 AND span.2, r.spans)
         GROUP BY f.instrument_id",
        day_bounds!()
    )
}

fn check_coverage() -> String {
    "SELECT concat(table_name, ':', source) AS subject, toUInt32(0) AS instrument_id, \
     toUInt64(24) AS compared, toUInt64(24 - length(present)) AS violations, \
     toJSONString(map('missing_hours', toString(arrayFilter(h -> NOT has(present, h), range(24))))) AS detail \
     FROM (SELECT table_name, source, groupUniqArray(toHour(hour)) AS present FROM hl.dq_hourly FINAL \
           WHERE hour >= toDate({day:Date}) AND hour < toDate({day:Date}) + 1 GROUP BY table_name, source)"
        .to_owned()
}

fn check_key_conflicts() -> String {
    // Classify the ID lists, then count the original rows, including repeated lists.
    format!(
        "{}, conflicts AS (
             SELECT table_name, ingest_ids FROM hl.ingest_conflict
             WHERE ts_exch >= day_start AND ts_exch < day_end
         ), reloads AS (
             SELECT ingest_ids
             FROM (SELECT ingest_ids, arrayJoin(ingest_ids) AS id FROM conflicts) AS c
             INNER JOIN hl.ingest_log AS l ON c.id = l.ingest_id
             GROUP BY ingest_ids
             HAVING uniqExact(l.ingest_id) >= 2
                AND uniqExact(l.ingest_id) = arrayUniq(ingest_ids)
                AND uniqExact(l.sha256) = 1
         )
         SELECT 'all tables' AS subject, toUInt32(0) AS instrument_id, count() AS compared,
                countIf(NOT is_reload) AS violations,
                if(compared = 0, '', JSONMergePatch(
                    toJSONString(map('tables', toString(arraySort(groupUniqArray(table_name))))),
                    if(countIf(is_reload) > 0, toJSONString(map('changed_by_reload', countIf(is_reload))), '{{}}')
                )) AS detail
         FROM (SELECT table_name, ingest_ids IN (SELECT ingest_ids FROM reloads) AS is_reload FROM conflicts)
         SETTINGS output_format_json_quote_64bit_integers = 0",
        day_bounds!()
    )
}

fn check_load_problems() -> String {
    format!(
        "{}, files AS (
             SELECT source, sha256, countIf(status = 'done') AS completed,
                    argMaxIf((ts_min, ts_max, details), (finished_at, ingest_id), status = 'done') AS latest,
                    countIf(status = 'failed' AND ts_max >= day_start AND ts_min < day_end) AS failed_on_day
             FROM hl.ingest_log
             GROUP BY source, sha256
             HAVING if(completed > 0, latest.2 >= day_start AND latest.1 < day_end, failed_on_day > 0)
         ), (details, name) -> JSONExtractUInt(details, 'counters', name) AS counter
         SELECT source AS subject, toUInt32(0) AS instrument_id, count() AS compared,
                sum(counter(latest.3, 'decode_errors') + counter(latest.3, 'book_errors')
                    + counter(latest.3, 'block_gaps')) + countIf(completed = 0) AS violations,
                toJSONString(mapFilter((name, n) -> n > 0, map(
                    'truncated', sum(counter(latest.3, 'truncated')),
                    'disconnects', sum(counter(latest.3, 'disconnects')),
                    'frames_dropped_by_recorder', sum(counter(latest.3, 'frames_dropped_by_recorder')),
                    'diffs_without_snapshot', sum(counter(latest.3, 'diffs_without_snapshot')),
                    'spot_rows_skipped', sum(counter(latest.3, 'spot_rows_skipped')),
                    'outcome_rows_skipped', sum(counter(latest.3, 'outcome_rows_skipped'))))) AS detail
         FROM files GROUP BY source",
        day_bounds!()
    )
}

/// Every daily check, in report order.
pub const CHECKS: &[Check] = &[
    Check {
        name: "l2_validity",
        kind: CheckKind::Alarm { max_rate: 0.0 },
        description: "books that are crossed, unsorted or not positive",
        sql: check_l2_validity,
    },
    Check {
        name: "bbo_validity",
        kind: CheckKind::Alarm { max_rate: 0.0 },
        description: "bbo rows that are crossed or negative",
        sql: check_bbo_validity,
    },
    Check {
        name: "fill_pairs",
        kind: CheckKind::Alarm { max_rate: 0.0 },
        description: "fill rows of trades without exactly one taker and one maker row",
        sql: check_fill_pairs,
    },
    Check {
        name: "trades_vs_archive_books",
        kind: CheckKind::Alarm { max_rate: 0.001 },
        description: "taker fills on the wrong side of both neighbouring archived books",
        sql: check_trades_vs_archive_books,
    },
    Check {
        name: "ws_books_vs_bbo",
        kind: CheckKind::Alarm { max_rate: 0.0 },
        description: "rebuilt books whose top differs from bbo at the same block time",
        sql: check_ws_books_vs_bbo,
    },
    Check {
        name: "ws_trades_vs_fills",
        kind: CheckKind::Alarm { max_rate: 0.0 },
        description: "recorded trades missing from official fills or differing from them",
        sql: check_ws_trades_vs_fills,
    },
    Check {
        name: "fills_vs_ws_trades",
        kind: CheckKind::Alarm { max_rate: 0.001 },
        description: "official taker fills inside each recorder run's recorded span that were not recorded",
        sql: check_fills_vs_ws_trades,
    },
    Check {
        name: "key_conflicts",
        kind: CheckKind::Alarm { max_rate: 0.0 },
        description: "recorded key conflicts, with same-file reload changes reported separately",
        sql: check_key_conflicts,
    },
    Check {
        name: "load_problems",
        kind: CheckKind::Alarm { max_rate: 0.0 },
        description: "errors and block gaps in each file's latest completed load, and files with only failed loads",
        sql: check_load_problems,
    },
    Check {
        name: "coverage",
        kind: CheckKind::Info,
        description: "UTC hours of the day without data",
        sql: check_coverage,
    },
];

/// One row a check query returns.
#[derive(Debug, Clone, PartialEq, Eq, Row, Deserialize)]
pub struct CheckResult {
    /// What was checked: a source, or `table:source`.
    pub subject: String,
    /// The instrument, or 0 for the subject as a whole.
    pub instrument_id: u32,
    /// Rows or items compared.
    pub compared: u64,
    /// How many of them failed.
    pub violations: u64,
    /// Extra numbers as JSON, or empty.
    pub detail: String,
}

#[derive(Debug, Row, Serialize)]
struct CheckRow {
    day: u16,
    check_name: String,
    subject: String,
    instrument_id: u32,
    kind: i8,
    compared: u64,
    violations: u64,
    max_rate: f64,
    passed: bool,
    detail: String,
    computed_at: i64,
}

/// The outcome of one check over one day.
#[derive(Debug)]
pub struct CheckOutcome {
    /// The check.
    pub check: &'static Check,
    /// Its rows; empty when there was nothing to check.
    pub results: Vec<CheckResult>,
}

impl CheckOutcome {
    /// True when every row passes.
    pub fn passed(&self) -> bool {
        self.results
            .iter()
            .all(|row| self.check.kind.passes(row.compared, row.violations))
    }
}

/// Days since 1970-01-01 for a `YYYY-MM-DD` date, as ClickHouse `Date`
/// stores it. `None` for anything else.
pub fn day_number(day: &str) -> Option<u16> {
    let bytes = day.as_bytes();
    let well_formed = bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && day
            .bytes()
            .enumerate()
            .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit());
    if !well_formed {
        return None;
    }
    let field = |range: core::ops::Range<usize>| day[range].parse::<i64>().ok();
    let (year, month, date) = (field(0..4)?, field(5..7)?, field(8..10)?);
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => return None,
    };
    if !(1..=days_in_month).contains(&date) {
        return None;
    }
    // Days from civil, after Howard Hinnant.
    let shifted_year = if month <= 2 { year - 1 } else { year };
    let era = shifted_year.div_euclid(400);
    let year_of_era = shifted_year.rem_euclid(400);
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + date - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    u16::try_from(era * 146_097 + day_of_era - 719_468).ok()
}

/// Recomputes the hourly statistics of `day` (`YYYY-MM-DD`, UTC), runs every
/// check, stores the results and returns them.
pub async fn run_day(db: &Db, day: &str, now_ms: i64) -> Result<Vec<CheckOutcome>, DbError> {
    let day_value =
        day_number(day).ok_or_else(|| DbError::BadArgument(format!("not a date: {day}")))?;
    let client = db.client();
    for statement in [
        hourly_l2(),
        hourly_fills(),
        hourly_bbo(),
        hourly_ws_trades(),
    ] {
        client.query(&statement).param("day", day).execute().await?;
    }

    let mut outcomes = Vec::new();
    let mut rows = Vec::new();
    for check in CHECKS {
        let results: Vec<CheckResult> = client
            .query(&(check.sql)())
            .param("day", day)
            .fetch_all()
            .await?;
        for result in &results {
            rows.push(CheckRow {
                day: day_value,
                check_name: check.name.to_owned(),
                subject: result.subject.clone(),
                instrument_id: result.instrument_id,
                kind: check.kind.code(),
                compared: result.compared,
                violations: result.violations,
                max_rate: check.kind.max_rate(),
                passed: check.kind.passes(result.compared, result.violations),
                detail: result.detail.clone(),
                computed_at: now_ms,
            });
        }
        outcomes.push(CheckOutcome { check, results });
    }
    db.insert("hl.dq_checks", &rows, &format!("dq:{day}:{now_ms}"))
        .await?;
    Ok(outcomes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alarm_thresholds_are_rates() {
        let exact = CheckKind::Alarm { max_rate: 0.0 };
        assert!(exact.passes(100, 0));
        assert!(!exact.passes(100, 1));
        assert!(exact.passes(0, 0), "nothing compared, nothing wrong");
        assert!(!exact.passes(0, 3), "violations without a base still fail");

        let loose = CheckKind::Alarm { max_rate: 0.001 };
        assert!(loose.passes(10_000, 10));
        assert!(!loose.passes(10_000, 11));
        assert!(CheckKind::Info.passes(1, 1));
    }

    #[test]
    fn converts_dates_to_clickhouse_day_numbers() {
        assert_eq!(day_number("1970-01-01"), Some(0));
        assert_eq!(day_number("2026-06-01"), Some(20_605));
        assert_eq!(day_number("2000-02-29"), Some(11_016));
        for bad in [
            "2026-6-01",
            "2026-13-01",
            "2026-02-29",
            "2026-04-31",
            "20260601",
            "2026-06-01T00",
            "",
            "1969-12-31",
        ] {
            assert_eq!(day_number(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn every_check_has_a_unique_name_and_takes_the_day_parameter() {
        let mut names: Vec<&str> = CHECKS.iter().map(|check| check.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), CHECKS.len());
        for check in CHECKS {
            let sql = (check.sql)();
            assert!(sql.contains("{day:Date}"), "{}", check.name);
            assert!(
                !sql.contains('?'),
                "{}: '?' would be read as a bind placeholder",
                check.name
            );
        }
        for statement in [
            hourly_l2(),
            hourly_fills(),
            hourly_bbo(),
            hourly_ws_trades(),
        ] {
            assert!(statement.starts_with("INSERT INTO hl.dq_hourly"));
            assert!(statement.contains("{day:Date}") && !statement.contains('?'));
        }
    }
}
