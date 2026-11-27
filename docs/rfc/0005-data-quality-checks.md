# RFC 0005: Data-quality checks

- Status: implemented (`schema/0002_quality.sql`, `hl-loader check`, conflict
  detection in `hl-loader load`); owner review pending
- Date: 2026-11-24
- Crates: `hl-loader`

Data-quality checks have three parts. The loader computes hourly statistics,
runs daily checks within and across data sources, and searches for conflicting
rows at load time.

## Decisions

### D1. Hourly statistics, recomputed per day

`hl-loader check <day>` first recomputes `hl.dq_hourly` for that UTC day: one
row per hour, table, source and instrument with the row count, the longest
stretch without a row (counted from the start of the hour and to its end),
the number of stretches over 5 s, invalid rows and short books. What counts
as invalid:

- a book that is crossed, has a non-positive price or size, or is not
  strictly ordered best price first.
  [HL-77](../venues/hyperliquid.md#hl-77)
- a fill trade that does not have exactly two rows, one taker and one maker,
  one buy and one sell. [HL-110](../venues/hyperliquid.md#hl-110)
- a `bbo` row that is crossed or negative, a recorded trade with a
  non-positive price or size.

The table is a `ReplacingMergeTree` versioned by the computation time, so a
day can be recomputed after more files are loaded. Statistics read the data
tables with `FINAL`, so rows loaded twice are counted once.

### D2. Daily checks with explicit thresholds

The same command then runs the checks below and stores one row per check,
subject and instrument in `hl.dq_checks`. An alarm check fails when
violations / compared exceeds its rate; the command then exits with status 1.

| Check | Compares | Fails above |
| --- | --- | --- |
| `l2_validity` | every book, archived and rebuilt | 0 |
| `bbo_validity` | every recorded `bbo` row | 0 |
| `fill_pairs` | every fill row, grouped by trade | 0 |
| `trades_vs_archive_books` | each taker fill against the archived books before and after it (reconciliation 2, D3) | 0.1% |
| `ws_books_vs_bbo` | rebuilt books against `bbo` at the same block time. [HL-137](../venues/hyperliquid.md#hl-137) | 0 |
| `ws_trades_vs_fills` | recorded trades against official taker fills, inside the time each loaded fills file covers, comparing trade id, time, price, size and side. [HL-87](../venues/hyperliquid.md#hl-87) | 0 |
| `fills_vs_ws_trades` | official taker fills inside each recorder run's recorded span for the instrument, against the recording | 0.1% |
| `key_conflicts` | all recorded key conflicts, with same-file reload changes reported separately (D4) | 0 |
| `load_problems` | latest completed load of each file touching the day, counting decode errors, book errors and block gaps, plus files with only failed loads | 0 |
| `coverage` | UTC hours of the day with data, per table and source | info only |

A check with nothing to compare that day is reported as `skip`. The exact
checks (threshold 0) cover things that cannot happen in correct data. The two
0.1% thresholds allow for a recording that dropped a frame and for a fill that
lands exactly at a book update.

For each `(source, sha256)`, `load_problems` selects the latest `done` row
by `finished_at`, breaking ties with the larger `ingest_id`. If that row's
time range touches the day, its counters supply both violations and detail.
A file with only failed loads contributes one violation when any attempt
touches the day.

For `fills_vs_ws_trades`, a recorder run groups files by `source` and
`process_start_wall_ns` in `hl.rawlog_state`. A file without a state row forms
its own run. The check resolves each trade's ingest id to its file's
`sha256` so that a reload preserves its run membership. The first and last
live trades of each instrument in a run bound its span for the day. That
span includes hourly rotations and disconnects. The check compares a fill
inside any span once and looks for a live or replayed trade with the same
instrument, trade id and time. [HL-86](../venues/hyperliquid.md#hl-86),
[HL-87](../venues/hyperliquid.md#hl-87)

Each deployment chooses when to run the checks and how to report failures.

The remaining reconciliations wait for their inputs to be loaded.
Reconciliation 1 compares fills volume with context `dayNtlVlm`.
[HL-91](../venues/hyperliquid.md#hl-91)
Reconciliation 3 compares official L2 with Tardis free-day L2 data.
[VN-2](../venues/vendors.md#vn-2), [VN-12](../venues/vendors.md#vn-12)
Reconciliation 4 compares 0xArchive L4 checkpoints and changes with official
books and fills. [VN-5](../venues/vendors.md#vn-5),
[VN-6](../venues/vendors.md#vn-6)

### D3. A fill contradicts the books only if it contradicts both neighbours

An earlier design compared each trade with the previous book and raised an
alarm above 1%. In the sample hour of 2026-06-01, 3.6% of BTC taker fills
(401 of 11,108) are on the wrong side of the preceding archived book. The books
are 0.54 s apart and BTC's spread is one tick, so the book moves between them.
None is on the wrong side of both the book before and the book after it.
[HL-138](../venues/hyperliquid.md#hl-138),
[HL-139](../venues/hyperliquid.md#hl-139)

Owner's decision on 2026-11-24: a violation is a taker fill on the wrong side
of both neighbouring books (a sell at or above the ask, a buy at or below the
bid), alarming above 0.1%. The check still records the previous-book count as
`wrong_side_of_book_before` in the check's detail. A book stamped T is the
state after block T, so the book before a fill is the last one strictly
earlier, and the book after is the first one at the same time or later.
[HL-75](../venues/hyperliquid.md#hl-75)

### D4. Conflicting rows are searched for right after each load

After a file's rows are inserted, the loader looks, in the same instruments
and time range, for keys that hold rows with different content, and writes
each such key to `hl.ingest_conflict` with the ingest ids and content hashes
involved. The load reports the count as `key_conflicts`.

Content is compared by a hash of the columns that must agree for equal keys:
the stored `row_hash` alias for books and fills, the six price and size
columns for `bbo`, and side, price, size, both parties and the hash for
recorded trades. The content hash treats live and replayed copies of the
same trade equally.
[HL-81](../venues/hyperliquid.md#hl-81)

Reloading a file with `--force` writes identical content when the loader is
unchanged. A changed loader can write different rows for the same file.
The load still records these differences in `hl.ingest_conflict`.

The daily check counts a conflict row in `changed_by_reload` only when it
names at least two distinct ingest ids, every id appears in `hl.ingest_log`,
and all matching log rows share one `sha256`.
A deliberate correction can then clear the alarm, and an unexpected change
to the same input stays visible. Every other row counts as a violation,
including a conflict within a single load, a conflict between different
files, or a row with an unknown ingest id.

The conflicting rows stay in their table until a merge keeps the newest.
`hl.ingest_conflict` keeps the evidence.

## Verification

- Sample day 2026-06-01 (two archived L2 files, one fills hour): all alarm
  checks pass. `trades_vs_archive_books` compares 11,391 taker fills with 0
  violations. The detail shows 401 BTC fills on the wrong side of the preceding book.
  [HL-139](../venues/hyperliquid.md#hl-139)
- Planted faults on that day, then removed: a crossed book, a trade with a
  single row, and one archived book line loaded again with a different size.
  `l2_validity`, `fill_pairs` and `key_conflicts` failed, the conflict row
  named both loads and both hashes, and the command exited with status 1.
  After removing the rows and reloading the original file with `--force`, all
  checks passed again.
- Recorded day 2026-11-24 (90 s of 11 instruments) with the official fills
  file `20261124/6.lz4` (320,016 perpetual rows): all alarm checks pass.
  1,764 rebuilt books and 5,908 `bbo` rows are valid, and all 889 coincident
  block times agree. All 1,337 recorded trades inside the file's span (1,025
  live, 312 from the subscription replay) match an official taker fill on
  trade id, time, price, size and side, which also confirms that the public
  trade id is the fill's `tid` and that side `A` is a selling taker. All
  1,025 official taker fills inside the recording window were recorded.
  [HL-137](../venues/hyperliquid.md#hl-137),
  [HL-81](../venues/hyperliquid.md#hl-81),
  [HL-87](../venues/hyperliquid.md#hl-87), [HL-83](../venues/hyperliquid.md#hl-83)
- The first version of `ws_trades_vs_fills` compared whole UTC hours and
  flagged 11 replayed trades from 05:57 to 05:59. Hourly fills files are cut
  by the node's clock: file `6` starts at block time 05:59:59.864, so hour 5
  looked covered while it was not. The check now uses the span each loaded
  file covers, taken from the ingest log.
  [HL-81](../venues/hyperliquid.md#hl-81),
  [HL-114](../venues/hyperliquid.md#hl-114)
- The first full daily check for 2026-11-24 reported 4 book errors from two
  loads replaced after the stale-diff fix and 20 changed keys from those
  reloads. It flagged 37,290 of 38,316 taker fills. Only the 06:00 UTC fills
  hour was loaded. Its recording lasted 90 s, and continuous recording
  started at 07:08. The old span included the gap between runs. For those loads,
  `load_problems` and `key_conflicts` should pass with 20 reload changes
  reported. The fill comparison should cover only the 90 s recording.
- Unit tests cover the threshold rule, the date conversion and the query
  texts. The opt-in `crates/hl-loader/tests/test_daily_checks.py` executes
  the daily checks on synthetic data in its own disposable ClickHouse. It
  covers latest-load counters, ordering and failed retries, reload changes,
  conflicts within one load or between files, unknown ingest ids, separated
  recorder runs, missing trades across an hourly rotation, legacy recordings,
  retained rows from earlier loads, replay matches and command exit status.
  The existing `crates/hl-loader/tests/test_rawlog_carry.py` continues to
  cover loading and carrying state across recording files. Both tests run
  with `HLMM_TEST_CLICKHOUSE=1 python3 -I <test file>`.

## Not built yet

- Alarm delivery.
- `GAP` markers in replay files: the replay export does not exist yet.
- A quarantine file for lines that fail to parse. Archive files are rejected
  whole with the line number; recordings keep the first five problems in the
  ingest log.
