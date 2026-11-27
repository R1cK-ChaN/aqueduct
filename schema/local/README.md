# Local ClickHouse

A single ClickHouse 26.8 LTS server for queries and backfill. Raw files remain
the source of truth. Every market-data table can be rebuilt from them with
`hl-loader`. The table design is in [RFC 0004](../../docs/rfc/0004-clickhouse-schema-and-loading.md).

## Start

Run the commands from the repository root. For the first start, copy the
settings file and set `CLICKHOUSE_PASSWORD` in the copy. Git ignores the copy.

```bash
cp schema/local/.env.example schema/local/.env
```

If either default port is in use, change `CLICKHOUSE_HTTP_PORT` (8123) or
`CLICKHOUSE_NATIVE_PORT` (9000) in the file. Start the server with the
settings in `schema/local/.env`.

```bash
schema/local/start.sh
```

To use a different settings file, pass its path to `start.sh`. The script
defaults to `data` and `logs` directories next to the settings file. Set
`CLICKHOUSE_DATA_DIR` or `CLICKHOUSE_LOG_DIR` to use other directories.

The script runs `clickhouse/clickhouse-server:26.8` as container
`hlmm-clickhouse`, listening on `127.0.0.1` only. The image is about 900 MB.
The same container is described for the Docker Compose plugin in
[compose.yaml](compose.yaml).

## Load data

Build the loader and give it the connection through the environment. If you
started the server with a different settings file, source that file here.

```bash
cargo build --release -p hl-loader
set -a && . schema/local/.env && set +a
export HLMM_CLICKHOUSE_URL="http://127.0.0.1:${CLICKHOUSE_HTTP_PORT:-8123}"
export HLMM_CLICKHOUSE_USER=hlmm HLMM_CLICKHOUSE_PASSWORD="$CLICKHOUSE_PASSWORD"
```

Create or update the tables.

```bash
target/release/hl-loader migrate
```

Use `archive-l2` for one coin-hour of the official L2 archive and
`node-fills` for an hourly fills file. [HL-97](../../docs/venues/hyperliquid.md#hl-97), [HL-105](../../docs/venues/hyperliquid.md#hl-105)

Use `rawlog` for a finished `.hlrec.zst` from `hl-recorder`.
The loader accepts lz4, zstd and plain files. Replace each example path with
the path to your file.

```bash
target/release/hl-loader load archive-l2 path/to/l2.lz4
target/release/hl-loader load node-fills path/to/fills.lz4
target/release/hl-loader load rawlog path/to/recording.hlrec.zst
```

Before loading, the loader checks that the database has exactly the
migrations built into it. If they differ, it exits with status 3 and writes
nothing. It prints one JSON line per file. The status is `loaded` with row
counts, `already_loaded`, `skipped` with a reason (a file still being written
or a recording of another venue), `waiting_for_predecessor` for a missing
earlier recording state, or `failed` with the error. A file loaded before is
skipped. Add `--force` right after the kind to load it again and replace its rows.

For numbered Hyperliquid recordings, `load rawlog` carries connection and
L2 book state between files in the same `(source, process_start_wall_ns)`
chain. Sequence 0 starts empty. A later file needs the stored state of the
preceding `file_seq`. `waiting_for_predecessor` names the chain and
`missing_file_seq`, writes nothing for that file and makes the command
exit non-zero. Load the missing predecessor and retry.

For a permanently lost predecessor, `load rawlog --no-carry` permits an
empty start and records `"carry": "missing_predecessor"` in the ingest
log's `details`. If the predecessor is available, its state is used.
`load rawlog --force` uses the predecessor's state and replaces the file's
own stored state. Subsequent files are reloaded only when requested.
If a lost predecessor is recovered, load it, then force-reload the affected
files in sequence to rebuild the rows originally decoded across the gap.

Older recordings without `file_seq` carry state in memory when all
contiguous files of one chain are passed to one `load rawlog --force`
invocation, including the file with the connection. A legacy file loaded
alone starts empty.

Use `status` for sizes and a summary of loads.

```bash
target/release/hl-loader status
```

## Check data quality

Run the checks for the UTC day you loaded.

```bash
target/release/hl-loader check 2026-06-01
```

The command recomputes the hourly statistics in `hl.dq_hourly` for that day,
runs the daily checks and stores their results in `hl.dq_checks`. The report
prints `pass` or `FAIL` for each alarm check, `info` for coverage and `skip`
where the day has no data for a check. The command exits with status 1 when
any alarm fails, so it can run from a timer. Rerunning a day replaces its
results. Run it again after loading more files for that day.

A load also reports `key_conflicts` when a file brings a row whose key
already holds different content. The keys are listed in `hl.ingest_conflict`.
The comparisons and thresholds are in [RFC 0005](../../docs/rfc/0005-data-quality-checks.md).

## Stop

```bash
docker stop hlmm-clickhouse
```

Data stays in the data directory. `docker rm hlmm-clickhouse` followed by
`start.sh` with the same settings brings the same data back.
