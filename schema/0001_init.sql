-- Migration 0001: market data from the official archives and our recordings.
--
-- Design and reasons: docs/rfc/0004-clickhouse-schema-and-loading.md.
-- Never edit this file once it has been applied anywhere; add 0002 instead.
--
-- Rules for migration files, enforced by hl-loader:
--   * every statement ends with ';' at the end of a line;
--   * no ';' inside a statement, and no '?' anywhere;
--   * '--' comments only on their own lines.
--
-- Conventions:
--   * prices, sizes and money are Decimal64(8), bit-for-bit the i64 fixed-point
--     values of hl-types;
--   * ts_exch is block time in milliseconds, ts_node a node's wall clock and
--     ts_recv our receive time, both in nanoseconds; 0 means not known;
--   * instrument_id is derived from the venue and coin name (RFC 0004);
--   * ingest_id identifies the load that wrote a row; a newer load of the same
--     key wins in ReplacingMergeTree tables;
--   * row_hash is a content hash computed when read (ALIAS, not stored), for
--     finding rows that share a key but differ in content.

-- One row per input file per load attempt.
CREATE TABLE IF NOT EXISTS hl.ingest_log
(
    ingest_id      UInt64,
    source         LowCardinality(String),
    file_name      String,
    sha256         String,
    bytes          UInt64,
    rows           UInt64,
    ts_min         DateTime64(3, 'UTC'),
    ts_max         DateTime64(3, 'UTC'),
    loader_version LowCardinality(String),
    started_at     DateTime64(3, 'UTC'),
    finished_at    DateTime64(3, 'UTC'),
    status         Enum8('done' = 1, 'failed' = 2),
    details        String,
    error          String
)
ENGINE = MergeTree
ORDER BY (sha256, ingest_id);

-- Every instrument that appears in loaded data.
CREATE TABLE IF NOT EXISTS hl.instruments
(
    venue         LowCardinality(String),
    coin          String,
    instrument_id UInt32,
    dex           LowCardinality(String),
    ingest_id     UInt64
)
ENGINE = ReplacingMergeTree(ingest_id)
ORDER BY (venue, coin);

-- Full order books: one row per book. Archived books, and books rebuilt from
-- recorded l2 snapshots and diffs. Order counts are 0 where the source does
-- not report them (levels set by a diff).
CREATE TABLE IF NOT EXISTS hl.l2_snapshot
(
    instrument_id UInt32,
    source        LowCardinality(String),
    ts_exch       DateTime64(3, 'UTC') CODEC(Delta, ZSTD(3)),
    ts_node       DateTime64(9, 'UTC') CODEC(Delta, ZSTD(3)),
    ts_recv       DateTime64(9, 'UTC') CODEC(Delta, ZSTD(3)),
    bid_px        Array(Decimal64(8)) CODEC(ZSTD(3)),
    bid_sz        Array(Decimal64(8)) CODEC(ZSTD(3)),
    bid_n         Array(UInt32) CODEC(ZSTD(3)),
    ask_px        Array(Decimal64(8)) CODEC(ZSTD(3)),
    ask_sz        Array(Decimal64(8)) CODEC(ZSTD(3)),
    ask_n         Array(UInt32) CODEC(ZSTD(3)),
    row_hash      UInt64 ALIAS cityHash64(bid_px, bid_sz, bid_n, ask_px, ask_sz, ask_n),
    ingest_id     UInt64
)
ENGINE = ReplacingMergeTree(ingest_id)
PARTITION BY toYYYYMM(ts_exch)
ORDER BY (instrument_id, source, ts_exch, ts_node)
SETTINGS non_replicated_deduplication_window = 10000;

-- Fills on perpetuals from node_fills_by_block: one row per party per trade.
-- Optional venue fields are stored as zero when absent: builder_fee and
-- builder, deployer_fee, priority_gas, cloid, twap_id, and the liquidation
-- columns when liquidation is false.
CREATE TABLE IF NOT EXISTS hl.fills
(
    instrument_id       UInt32,
    ts_exch             DateTime64(3, 'UTC') CODEC(Delta, ZSTD(3)),
    block_number        UInt64 CODEC(Delta, ZSTD(3)),
    ts_node             DateTime64(9, 'UTC') CODEC(Delta, ZSTD(3)),
    tid                 UInt64,
    side                Enum8('B' = 1, 'A' = 2),
    user                FixedString(20),
    crossed             Bool,
    px                  Decimal64(8),
    sz                  Decimal64(8),
    start_pos           Decimal64(8),
    dir                 LowCardinality(String),
    closed_pnl          Decimal64(8),
    fee                 Decimal64(8),
    fee_token           LowCardinality(String),
    builder_fee         Decimal64(8),
    builder             FixedString(20),
    deployer_fee        Decimal64(8),
    priority_gas        Decimal64(8),
    oid                 UInt64,
    cloid               FixedString(16),
    twap_id             UInt64,
    hash                FixedString(32),
    liquidation         Bool,
    liquidated_user     FixedString(20),
    liquidation_mark_px Decimal64(8),
    liquidation_method  LowCardinality(String),
    source              LowCardinality(String),
    row_hash            UInt64 ALIAS cityHash64(user, crossed, px, sz, start_pos, dir, closed_pnl, fee, fee_token, builder_fee, deployer_fee, priority_gas, oid, cloid, twap_id, hash, liquidation),
    ingest_id           UInt64
)
ENGINE = ReplacingMergeTree(ingest_id)
PARTITION BY toYYYYMM(ts_exch)
ORDER BY (instrument_id, ts_exch, tid, side)
SETTINGS non_replicated_deduplication_window = 10000;

-- One row per trade: the taker's row of hl.fills.
CREATE VIEW IF NOT EXISTS hl.trades AS
SELECT instrument_id, ts_exch, block_number, tid, side AS aggressor, px, sz, user AS taker, oid AS taker_oid, hash, liquidation, source
FROM hl.fills
WHERE crossed;

-- Best bid and ask changes recorded from the bbo channel. A price of 0 means
-- the side was empty.
CREATE TABLE IF NOT EXISTS hl.bbo
(
    instrument_id UInt32,
    source        LowCardinality(String),
    ts_exch       DateTime64(3, 'UTC') CODEC(Delta, ZSTD(3)),
    ts_recv       DateTime64(9, 'UTC') CODEC(Delta, ZSTD(3)),
    bid_px        Decimal64(8),
    bid_sz        Decimal64(8),
    bid_n         UInt32,
    ask_px        Decimal64(8),
    ask_sz        Decimal64(8),
    ask_n         UInt32,
    ingest_id     UInt64
)
ENGINE = ReplacingMergeTree(ingest_id)
PARTITION BY toYYYYMM(ts_exch)
ORDER BY (instrument_id, source, ts_exch)
SETTINGS non_replicated_deduplication_window = 10000;

-- Trades recorded from the public trades channel: one row per trade, with
-- both parties. live = 1 when the trade arrived while we were subscribed;
-- live = 0 for trades replayed in the snapshot sent after subscribing, whose
-- ts_recv is not a real receive time. A live copy replaces a replayed one.
CREATE TABLE IF NOT EXISTS hl.ws_trades
(
    instrument_id UInt32,
    source        LowCardinality(String),
    ts_exch       DateTime64(3, 'UTC') CODEC(Delta, ZSTD(3)),
    ts_recv       DateTime64(9, 'UTC') CODEC(Delta, ZSTD(3)),
    tid           UInt64,
    aggressor     Enum8('B' = 1, 'A' = 2),
    px            Decimal64(8),
    sz            Decimal64(8),
    buyer         FixedString(20),
    seller        FixedString(20),
    hash          FixedString(32),
    live          UInt8,
    ingest_id     UInt64
)
ENGINE = ReplacingMergeTree(live)
PARTITION BY toYYYYMM(ts_exch)
ORDER BY (instrument_id, ts_exch, tid)
SETTINGS non_replicated_deduplication_window = 10000;
