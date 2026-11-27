# RFC 0004: ClickHouse schema and loading

- Status: implemented in `hl-loader`, owner review pending
- Date: 2026-11-24
- Crates: `hl-loader`; `hl-types` (instrument ids)

This RFC describes the first migration and the loader. Its three inputs
are the official L2 archive, the official `node_fills_by_block`
archive, and recordings made by `hl-recorder` of a Hyperliquid feed.
[HL-97](../venues/hyperliquid.md#hl-97),
[HL-105](../venues/hyperliquid.md#hl-105)

## Decisions

### D1. Instrument ids are derived from the venue and the coin name

**Changed by RFC 0009 D1 and D5.** This decision previously defined derived
ids for the loader and left Binance's venue name open. RFC 0009 makes the
formula the only source of ids across callers and fixes Binance naming.
[RFC 0009](0009-instrument-identity.md)

`InstrumentId::derive(venue, symbol)` is the 32-bit FNV-1a hash of the UTF-8
text `"{venue}:{symbol}"`. RFC 0009 D1 fixes the exact inputs for Hyperliquid,
and D5 fixes them for Binance. [RFC 0009](0009-instrument-identity.md) ·
[HL-55](../venues/hyperliquid.md#hl-55)

Chosen by the owner over a registry file in the repository and over the
venue's asset ids, because an id then needs no stored state: rebuilding the
database from raw files gives the same ids, and a newly listed coin gets one
without a code change. The reported count of 302 perpetuals in one hour
of fills remains unverified. [HL-142](../venues/hyperliquid.md#hl-142)

The cost is collisions. With n names the chance of any collision is about
n² / 2³³: 1 in 100,000 for 300 names, 1 in 1,000 for 3,000. The loader reads
`hl.instruments` before every load and refuses to continue when a new name
hashes to an id another name already has. Resolving one would need an
override table; that is a new RFC if it ever happens.

This originally refined RFC 0001 D3 by fixing id derivation. RFC 0009 now
owns the shared identity contract. The loader continues the collision
check above and uses the existing `hl.instruments` table.
[RFC 0009](0009-instrument-identity.md)

### D2. Tables

| Table | Rows | Key | Engine version |
| --- | --- | --- | --- |
| `hl.l2_snapshot` | one 20-level book, archived or rebuilt from recorded `l2`. [HL-100](../venues/hyperliquid.md#hl-100) | `(instrument_id, source, ts_exch, ts_node)` | `ingest_id` |
| `hl.fills` | one party of one perpetual trade, from `node_fills_by_block`. [HL-110](../venues/hyperliquid.md#hl-110) | `(instrument_id, ts_exch, tid, side)`. [HL-86](../venues/hyperliquid.md#hl-86) | `ingest_id` |
| `hl.trades` | view of the taker row of each trade in `hl.fills`. [HL-110](../venues/hyperliquid.md#hl-110) | — | — |
| `hl.bbo` | one recorded `bbo` message | `(instrument_id, source, ts_exch)` | `ingest_id` |
| `hl.ws_trades` | one recorded public trade, both parties. [HL-90](../venues/hyperliquid.md#hl-90) | `(instrument_id, ts_exch, tid)`. [HL-86](../venues/hyperliquid.md#hl-86) | `live` |
| `hl.instruments` | one `(venue, coin)` | `(venue, coin)` | `ingest_id` |
| `hl.ingest_log` | one input file per load attempt | `(sha256, ingest_id)` | — |
| `hl.rawlog_state` | end state of one numbered recording file | `(source, process_start_wall_ns, file_seq)` | `ingest_id` |
| `hl.schema_migrations` | one applied migration | `version` | — |

The tables use the following storage rules.

- **Fills are perpetuals only.** Owner's decision. Spot and outcome rows are
  out of scope, and some spot fees need ten decimals. Their counts go into
  the ingest log; the raw files still hold them.
  [HL-117](../venues/hyperliquid.md#hl-117)
- **The fills key ends with `side`, not `user`.** A self-trade would have the
  same user on both rows and collapse into one. Every perpetual trade in the
  sample hour has exactly one `B` row and one `A` row, so `side` is unique.
  [HL-110](../venues/hyperliquid.md#hl-110)
- **`row_hash` is computed when read** (`ALIAS cityHash64(...)`), not stored.
  Stored, it took 12% of the fills table and did not compress.
- **`hl.trades` is a plain view**, not a materialized view. The planned
  month-level backfill replaces partitions, which does not feed materialized
  views.
- **Order counts are `UInt32`**, the type of `Level::orders`, so nothing is
  truncated. Levels set by a recorded diff have 0: the venue does not send a
  count with diffs. [HL-74](../venues/hyperliquid.md#hl-74)
- **`hl.ws_trades` is new.** Recorded trades carry both parties but no order
  ids or fees, so they do not fit `hl.fills`. The loader and replay use the
  same trade cutoff in `hl_codec::hyperliquid`. When the connection start
  is unknown, trades count as live (`live = 1`). After a `connected`
  record, trades with `ts_exch` before its receipt wall time count as
  replayed (`live = 0`), and trades at or after it count as live. Trades
  received after a disconnect or shutdown have `live = 0` until reconnection.
  The table keeps the live copy when both exist, preserving its `ts_recv`.
  [HL-90](../venues/hyperliquid.md#hl-90),
  [HL-81](../venues/hyperliquid.md#hl-81)
- **Absent optional fields are stored as zero** (builder fee and builder,
  deployer fee, priority gas, cloid, TWAP id, liquidation details) instead of
  `Nullable`.
  [HL-109](../venues/hyperliquid.md#hl-109)

### D3. Loading is idempotent in three layers

1. **Ingest log.** The SHA-256 of the file as stored is checked against
   `hl.ingest_log`. A file already loaded with status `done` is skipped.
   For numbered recordings the loader also checks the file's state row (D8).
2. **Deduplication token.** Rows go in chunks of 50,000, each with
   `insert_deduplication_token = {sha256}:{table}:{chunk}`, and the tables keep
   the last 10,000 tokens. When a recording starts from carried state, its
   token also includes the SHA-256 of the starting state's JSON. A retry
   reuses chunks decoded from the same input. Finding a previously missing
   predecessor can change the decoded rows, so it produces a different token.
   When the loader skips a stale diff (D5), it uses a distinct L2 token so
   ClickHouse writes the corrected rows on a retry after an interrupted load
   with an older decoder.
3. **ReplacingMergeTree.** Rows with the same key collapse to the newest
   `ingest_id` when parts merge, or at once with `FINAL`.

`hl-loader load <kind> --force` loads a file again under a new ingest id, with a
token that includes it, so the new rows are written and replace the old.

Month-level backfill through a staging table and `REPLACE PARTITION`
is still pending because it needs whole months of data.

### D4. Archives are strict, recordings are forgiving

One line of an official archive that does not decode rejects the file, and
the failure is logged with its line number: a format change must stop the
backfill. In a recording, a frame that does not decode, a book that goes out
of sync, or a cut-off end is counted in the ingest log's `details` and the
rest is loaded: one odd frame must not cost an hour of data.

### D5. Books are rebuilt from recordings, one row per update

Every recorded snapshot and every diff that applies cleanly becomes a full
book row. A diff at or before its subscription snapshot's block time is
already included (RFC 0003 D3). [HL-75](../venues/hyperliquid.md#hl-75)

The loader increments the file's
`stale_diffs_skipped` counter and emits no row for that diff. A
`disconnected` record drops every book until its next snapshot, and diffs
that arrive with no book are counted as `diffs_without_snapshot`.

### D6. Migrations ship inside the loader

The loader embeds `schema/*.sql`, applies the ones not yet in
`hl.schema_migrations`, and stops if an applied file's text has changed. The
row types and the schema therefore always come from the same commit. File
rules: one statement per `;` at a line end, `--` comments on their own lines,
no `?`.

Before `load` and `check` write anything, the loader compares
`hl.schema_migrations` with its own list and exits with status 3 unless they
are identical: same versions, names and text. Both directions matter. A
database migrated by a newer loader has tables this loader's rows do not
describe; an insert can then fail, or succeed with new columns left at their
defaults, which no check would notice. A database that lacks one of the
loader's migrations needs `migrate` first. `migrate` itself refuses a
database that has a migration it does not include. This rule was added on
2026-11-24 because an installed loader, such as one run by a timer, keeps
the migrations it was built with. After a newer migration is applied,
`load` and `check` exit with status 3 until that copy is rebuilt.

### D7. Connection settings come from the environment

`HLMM_CLICKHOUSE_URL`, `HLMM_CLICKHOUSE_USER` and `HLMM_CLICKHOUSE_PASSWORD`. The
password never appears on a command line or in output.

### D8. Hourly files continue the same stream

A Hyperliquid `l2` subscription supplies an initial snapshot, followed by
diffs. [HL-66](../venues/hyperliquid.md#hl-66)

A file boundary can fall between any two frames. Splitting the same
stream at those boundaries must produce identical `l2_snapshot`, `bbo` and
`ws_trades` rows, including trade `live` flags, apart from `ingest_id`.
The loader carries the open connection's establishment time in wall-clock
nanoseconds and the full L2 book for each instrument. A disconnect or
shutdown clears the connection and books. A new connection starts a
fresh set of books.
A trade is live when a connection is up and its exchange time is at or
after that connection's establishment.

`hl-loader load rawlog [--force] [--no-carry] <file>...` reads the chain
and sequence from the RFC 0002 `file_open` header. Sequence 0 starts from
empty state. Sequence N starts from the stored end of N−1 for the same
`(source, process_start_wall_ns)`. If that row is missing, the loader
prints `waiting_for_predecessor` with `source`, `process_start_wall_ns`
and `missing_file_seq`, writes nothing for this file and counts it as a
failure for the command's exit status. A later load can retry the file.
For a permanently lost predecessor, `--no-carry` permits an empty start
and records `"carry": "missing_predecessor"` in the ingest log's `details`.
An available predecessor is used even with this flag.

Migration 0003 creates `hl.rawlog_state`. Each row records the file's
SHA-256 and ingest id. Its compressed JSON stores the connection state and
all book levels with integer fixed-point prices, sizes and order counts.
Version 2 of `state_json` also stores each book's subscription snapshot time
in `snapshot_time_ns` (exchange time in nanoseconds). The loader can still
read version 1, which held only the levels and sync status for each book.
A synced version 1 book continues to accept diffs without a cutoff. Its
version 2 state stores `null` until a new snapshot establishes the time.

The loader writes market rows, checks for conflicts, stores the end state
and then records `done` in the ingest log. A state write failure is a failed
load. Returning `already_loaded` for a numbered file requires its state
row to match the latest successful ingest. The loader reports a missing
match as an error. A forced load reads the same predecessor and replaces
its own state under a new ingest id, which also repairs a missing state
row. Subsequent files can be reloaded individually.

**Reloading earlier recordings.** A file without `file_seq` starts from
empty state when loaded alone. To rebuild older hours, supply all available
consecutive files for one process and source, starting with the file that
contains the connection, in a single `hl-loader load rawlog --force`
invocation. The loader orders that chain by the `file_open` record's
process-relative timestamp and carries state in memory. The monotonic
clock preserves file order across a wall-clock correction. Legacy files
have no sequence with which to
detect a missing intermediate hour. Run the command separately for each
known contiguous range when a file is missing.

## Measured on the sample hour

2026-06-01 09:00 to 10:00 UTC from the official archives, loaded into
ClickHouse 26.8.45.6 and merged with `OPTIMIZE FINAL`.

| Table | Rows | Stored | Per row | Source file (lz4) |
| --- | --- | --- | --- | --- |
| `l2_snapshot`, BTC and JUP | 13,348 | 1.14 MB | 86 B | 1.66 MB |
| `fills`, all perpetuals | 387,496 | 21.0 MB | 54 B | 38.1 MB |

The archive book counts and cadence are recorded in
[HL-138](../venues/hyperliquid.md#hl-138).

Loading took 0.5 s for the two L2 files and 1.7 s for the fills hour, with at
most 180 MB of memory, on the workstation.

Extrapolated, assuming this hour is typical:

- **Archived L2.** The sampled cadence extrapolates to about 160,000 books
  per coin per day, or 13.7 MB per coin-day. BTC and JUP are averaged, and
  other coins will differ. [HL-138](../venues/hyperliquid.md#hl-138)
  The estimate of about 130 GB for 30 coins assumes coverage from
  2025-07-27 to 2026-06-12, which remains unverified.
  [HL-103](../venues/hyperliquid.md#hl-103)
  The additional assumption of tenfold thinning from 2026-06-13 also
  remains unverified.
  [HL-104](../venues/hyperliquid.md#hl-104)
- **Fills.** 9.3 million perpetual rows per day, 0.5 GB per day, about 240 GB
  for the 16 months from 2025-07-27. This hour's file is 30% larger than the
  hourly average of 2026-11-22, so 180 to 250 GB is the better range.

A 90-second recording of 11 instruments gave 1,764 rebuilt books,
5,908 `bbo` rows and 1,348 trades.

On that one sample, recording 30 instruments would add roughly
1 GB per day to the database.

## Verification

- Unit tests read the real archive lines and the recording excerpt in
  `crates/hl-codec/tests/fixtures` and check rows field by field, including
  a recording cut off mid-file, a disconnect, an undecodable frame and a
  recording of another venue.
- The rawlog CLI integration test starts a disposable ClickHouse 26.8.
  It compares hourly loads with the uncut recording, exercises missing
  predecessors and force reloads, and checks migration and retry behaviour.
  Its run command is in
  [test_rawlog_carry.py](../../crates/hl-loader/tests/test_rawlog_carry.py).
- Against a local ClickHouse 26.8 (not automated; there is no test database
  yet):
  - the migration applies, and a second run applies nothing;
  - stored values of a known fill and a known book equal the source lines;
  - 193,748 trades, each with two rows and one taker row, and the view
    returns 193,748. [HL-110](../venues/hyperliquid.md#hl-110)
  - loading a file twice skips it; deleting its ingest-log entry and loading
    again writes no rows (tokens); `--force` writes new rows that collapse to
    the newest ingest id under `FINAL`;
  - in the recording, all 889 block times at which a rebuilt book and a `bbo`
    message coincide agree on best price and size.
    [HL-137](../venues/hyperliquid.md#hl-137)
  - a Binance recording is skipped with a reason.

## Not built yet

- Data-quality tables and the daily checks: built later, see RFC 0005.
- Asset contexts, funding, Binance reference data, and the `live` and
  `research` databases.
- Backups, and the upload of recordings from the trading host.
