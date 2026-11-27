-- Migration 0003: decoder state across hourly raw recording files.
--
-- Design and reasons: docs/rfc/0004-clickhouse-schema-and-loading.md.
-- Same file rules as 0001.

-- One logical row per file in a recorder process's source chain.
-- Written after its market-data rows and before its successful ingest log.
-- The JSON contains versioned connection and book state, with fixed-point
-- prices and sizes represented by their exact scaled integers.
CREATE TABLE IF NOT EXISTS hl.rawlog_state
(
    source                LowCardinality(String),
    process_start_wall_ns  Int64,
    file_seq              UInt64,
    sha256                String,
    state_json            String CODEC(ZSTD(3)),
    ingest_id             UInt64
)
ENGINE = ReplacingMergeTree(ingest_id)
ORDER BY (source, process_start_wall_ns, file_seq)
SETTINGS non_replicated_deduplication_window = 10000;
