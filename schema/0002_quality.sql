-- Migration 0002: data-quality statistics, daily checks and key conflicts.
--
-- Design and reasons: docs/rfc/0005-data-quality-checks.md.
-- Same file rules as 0001.

-- What the data looks like, per UTC hour, table, source and instrument.
-- Recomputed by `hl-loader check <day>`; the newest computation wins.
--   max_gap_ms:   the longest stretch without a row, counting from the start
--                 of the hour and to its end;
--   gaps_over_5s: stretches longer than 5 s (the replay files mark these);
--   invalid:      books that are crossed, unsorted or not positive; fill
--                 trades without exactly one taker row and one maker row;
--                 rows with a non-positive price or size;
--   short_books:  books with fewer than 20 levels a side, bbo rows with an
--                 empty side.
CREATE TABLE IF NOT EXISTS hl.dq_hourly
(
    hour          DateTime('UTC'),
    table_name    LowCardinality(String),
    source        LowCardinality(String),
    instrument_id UInt32,
    rows          UInt64,
    max_gap_ms    UInt64,
    gaps_over_5s  UInt64,
    invalid       UInt64,
    short_books   UInt64,
    computed_at   DateTime64(3, 'UTC')
)
ENGINE = ReplacingMergeTree(computed_at)
PARTITION BY toYYYYMM(hour)
ORDER BY (hour, table_name, source, instrument_id);

-- One row per check, subject and instrument for one UTC day. An alarm check
-- passes when violations / compared is at most max_rate; an info check always
-- passes and only reports. instrument_id 0 means the subject as a whole.
CREATE TABLE IF NOT EXISTS hl.dq_checks
(
    day           Date,
    check_name    LowCardinality(String),
    subject       LowCardinality(String),
    instrument_id UInt32,
    kind          Enum8('alarm' = 1, 'info' = 2),
    compared      UInt64,
    violations    UInt64,
    max_rate      Float64,
    passed        Bool,
    detail        String,
    computed_at   DateTime64(3, 'UTC')
)
ENGINE = ReplacingMergeTree(computed_at)
PARTITION BY toYYYYMM(day)
ORDER BY (day, check_name, subject, instrument_id);

-- Keys that hold rows with different content after a load: the same book,
-- fill, bbo or trade written twice with different values. Found by the
-- loader right after it inserts a file; the rows themselves stay in their
-- table until a merge keeps one of them.
CREATE TABLE IF NOT EXISTS hl.ingest_conflict
(
    detected_at   DateTime64(3, 'UTC'),
    ingest_id     UInt64,
    table_name    LowCardinality(String),
    instrument_id UInt32,
    ts_exch       DateTime64(3, 'UTC'),
    row_key       String,
    ingest_ids    Array(UInt64),
    row_hashes    Array(UInt64)
)
ENGINE = MergeTree
PARTITION BY toYYYYMM(ts_exch)
ORDER BY (table_name, instrument_id, ts_exch);
