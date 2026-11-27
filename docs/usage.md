# Usage

## Build and test

The Rust toolchain version is pinned in `rust-toolchain.toml`. With
[rustup](https://rustup.rs) installed, cargo downloads that version on first
use. Run these locally from the repository root.

```bash
cargo fmt --all --check
```

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

```bash
cargo test --workspace
```

The end-to-end example is `crates/hl-run/tests/minimal_chain.rs`. It
drives a fixed-spread quoter with hand-written market events against the
simulator, then runs the same strategy against a second gateway.

## Run a recording backtest

Run `FixedSpreadQuoter` against the simulator with one or more Hyperliquid
recordings. Replace the example paths with the files to replay and choose
the half spread and order size for the recorded coin.

```bash
cargo run --release -p hl-backtest -- run \
  --coin BTC --half-spread 10 --qty 0.001 \
  --order-latency-ms 400 --report-latency-ms 250 \
  --maker-fee-bps 1.5 \
  /path/to/first.hlrec.zst /path/to/next.hlrec.zst
```

`--network mainnet|testnet` is optional and defaults to `mainnet`.
`--min-notional <usd>` is optional and defaults to `10`, using the registered
minimum-order rule. [HL-27](venues/hyperliquid.md#hl-27)

Every flag shown in the command is required. Prices, sizes, notionals and
fee rates are parsed as fixed-point decimals. Latencies are whole
milliseconds. The example's `--maker-fee-bps 1.5` follows the base rate
entry. [HL-141](venues/hyperliquid.md#hl-141)

The reader traverses each numbered recorder process by `file_seq` and
repeatedly takes the chain whose next file has the smallest `file_open`
wall time, breaking ties by path. Each legacy file without `file_seq`
forms its own chain. Missing or duplicate sequence numbers fail the run.
The reader holds the selected coin's BBO, live-trade and feed-gap events
in memory. Both the simulator and the strategy process them in receipt
order. A BBO or trade proves an earlier action when its exchange time
exceeds the action's effect time. [RFC 0008 D2, D3 and D5](rfc/0008-paper-trading.md)

Local delivery time is the maximum of the raw receive wall stamp, the
anchored monotonic stamp, the event's exchange time and the preceding
event's local time. Gaps omit exchange time. The inclusive stop is the
latest of the final complete record's wall time, its anchored monotonic
time and the last event's local time.
[RFC 0008 D4 and D7](rfc/0008-paper-trading.md)

Stdout contains one JSON summary followed by a newline. Repeating the same
inputs and arguments produces byte-identical stdout. Stderr reports
`read_decode_ms` and `replay_ms` on one line. The exit status is 0 for a
completed run, 1 for a rules file, recording, replay or accounting error,
and 2 for invalid arguments.

Add `--session-out session.jsonl` to the recording command to keep the
settings, decisions, fills and final account. The writer seals a new file
through `session.jsonl.part` and preserves an existing destination. The
session tools use GNU Coreutils `sha256sum` and `mv` on Linux. To verify a
session, run the following command with the same build and the recordings
listed in the file.

```bash
cargo run --release -p hl-backtest -- run --check-session session.jsonl
```

The check verifies the build identity and raw hashes, then recomputes the
decisions from the recorded settings. A match prints one line. A field
difference reports the first sequence or boundary row, the field and both
values, and exits 1. [Session file reference](session-file.md)

The summary's `time.process_clock_raised` counts events whose anchored
monotonic time exceeds the maximum of raw wall and exchange time.
`time.process_clock_max_raise_ns` gives the largest such raise in integer
nanoseconds, or zero. For gaps the comparison uses raw wall time alone.
`time.local_time_raised` counts subsequent raises to the preceding event's
local time. `time.negative_latency` compares raw wall and exchange time.

`late_print_misses` counts late trades and their total size that could have
filled a superseded order version. `pending_at_end` counts actions awaiting
proof, undelivered reports and outstanding timers. `report_wait_ns` measures
the extra delay beyond the configured report latency for delivered action and
fill reports. Each group gives its size, the number delayed, nearest-rank
percentiles and maximum. An empty group has null percentiles and maximum.

The summary's final member, `pnl`, reports one instrument's account with
`excluded` set to `["funding"]`. Accounting starts with zero position and
cash. Monetary amounts use the instrument's settlement token, and fixed-point
values are decimal strings. `position` is bought size minus sold size,
`notional` is the sum of fill price times size, and `fees` is the sum of the
reported fill fees, with a negative fee representing a rebate.
[HL-28](venues/hyperliquid.md#hl-28) · [HL-29](venues/hyperliquid.md#hl-29)

`mark_px` is the midpoint of the most recent BBO with both sides present and
bid below ask. `mark_ts_exch_ms` is that quote's exchange time in milliseconds.
Both are `null` until a usable quote is applied. `gross` is trading cash plus
position valued at the mid, and `net` is gross profit minus fees. A nonzero
position requires a mark to report profit. An inexact midpoint, an inexact
product or a fixed-point overflow stops the run. The backtest independently
checks final net profit from the fills before emitting the summary.

`max_abs_position` is the largest absolute inventory after any fill or mark.
`min_net` is the lowest defined net profit, starting at zero, and
`min_net_ts_exch_ms` records when that minimum first occurred. The time is
`null` if net profit never falls below the initial zero. Accounting applies
fills and marks by exchange time, with marks before fills at equal times and
input order preserved otherwise.

## Paper session

Build both binaries from the same checkout. The [example configuration](../crates/hl-live/example.toml)
selects BTC, the fixed-spread quoter, simulator parameters, a public feed and
a relative output directory. Set `source.url` and its subscriptions to the
feed you intend to record. The example uses the public endpoint documented
in [HL-41](venues/hyperliquid.md#hl-41).

```bash
cargo build --release -p hl-live -p hl-backtest
target/release/hl-live paper --config crates/hl-live/example.toml
```

Stop with Ctrl-C (SIGINT) or send SIGTERM. The process drains the admitted
input and prints the sealed session path. Each restart gets its own directory
under `out_dir`. Keep its raw tree beside the session file. Use the printed
path for the offline check.

```bash
target/release/hl-backtest run --check-session "paper-runs/<run-directory>/session.jsonl"
```

A complete session includes the final shutdown record, the hashes of the
sealed recordings, the account and remaining work.

Run the daily check with the session and its sealed raw files in the same
relative layout. After each UTC midnight, this command checks the running
session through its latest published checkpoint and prints the UTC cutoff
and row count. It also works on a copy left by an interrupted process.
[Session file reference](session-file.md)

```bash
target/release/hl-backtest run --check-session "paper-runs/<run-directory>/session.jsonl.part"
```

## Dated price and size rules

Backtests and paper sessions can read the same local UTF-8 JSON file. Save
this example as `rules.json`. The coin, times and asset ids are synthetic.

```json
{
  "instruments": [
    {
      "network": "testnet",
      "coin": "SYNTH",
      "versions": [
        {"effective_from": "100", "sz_decimals": 2, "asset_id": "17"},
        {"effective_from": "200", "sz_decimals": 3, "asset_id": "21"}
      ]
    }
  ]
}
```

Use one history per pair of network (`mainnet` or `testnet`) and exact coin
name, including any dex prefix. Each array needs at least one entry. Use
exactly the fields shown and list versions in increasing start-time order.

- Set `effective_from` to the first exchange time covered by the version,
  using your dated records. Write it as a signed 64-bit decimal string in
  UTC Unix epoch nanoseconds.
- Set `sz_decimals` to the version's `szDecimals` from perpetual `meta`, as
  a JSON integer from 0 through 6. The simulator uses this precision for its
  size and price checks. [HL-26](venues/hyperliquid.md#hl-26) ·
  [HL-23](venues/hyperliquid.md#hl-23) · [HL-24](venues/hyperliquid.md#hl-24) ·
  [HL-25](venues/hyperliquid.md#hl-25)
- For `asset_id`, use the main-dex index in `meta` or the builder-dex
  calculation for the selected market, encoded as an unsigned 64-bit
  decimal string. [HL-53](venues/hyperliquid.md#hl-53) ·
  [HL-54](venues/hyperliquid.md#hl-54)

Write integer strings with unpadded decimal digits and a minus sign only for
negative starts. To replay a recording of the example instrument, pass
`--rules-file` once. A relative path starts at the working directory.

```bash
target/release/hl-backtest run --network testnet --coin SYNTH \
  --half-spread 1 --qty 1.001 --order-latency-ms 0 --report-latency-ms 0 \
  --maker-fee-bps 0 --rules-file rules.json --session-out session.jsonl \
  recording.hlrec.zst
```

For paper, copy `crates/hl-live/example.toml` to `paper.toml` beside
`rules.json`. Set these top-level keys before `[source]`. Set the source URL
to a feed for the selected market and its `symbols` to `["SYNTH"]`.

```toml
network = "testnet"
coin = "SYNTH"
rules_file = "rules.json"
```

Paper resolves a relative `rules_file` from the config file's directory.
Both drivers also accept absolute paths. The first command below checks
both files before the second opens the feed.

```bash
target/release/hl-live check --config paper.toml
target/release/hl-live paper --config paper.toml
```

Each driver reads the file once at startup and selects the run's network
and coin. An unreadable or invalid file, or a missing pair, stops the run
before backtest opens a recording or paper creates a run directory or
connects. A new or modified order that fails the rules at its effect time
gets an `invalid_order` rejection. An effect time before the first version
stops the run.

With a file, the backtest summary includes the selected `rules` array
immediately after `min_notional`, even without `--session-out`. Both drivers
write the same array in the version 2 session's `start` row.
`--check-session` uses those embedded versions after the rules file is
deleted, for both sealed sessions and open paper checkpoints.
Without a file, each driver prints `no grid check applied` once on stderr,
sessions use version 1 and the backtest summary omits `rules`.

## Layout

```text
crates/
  hl-types      fixed-point prices and sizes, instruments, orders, events
  hl-rules      dated perpetual price and size checks, next valid price
  hl-strategy   Strategy trait, its IO-free host, the example quoter
  hl-gateway    OrderGateway trait (the live client will sit behind a feature)
  hl-sim        simulated venue implementing OrderGateway (minimal)
  hl-run        record conversion, in-memory replay driver and accounting summary
  hl-backtest   recording file reader, JSON summary and command line
  hl-codec      recorder file format; decoding of Hyperliquid frames and official archives
  hl-book       L2 order book from snapshots and diffs (L4 planned)
  hl-ledger     one-instrument position, cash, fill fees and marked profit (v1, funding deferred)
  hl-recorder   raw market data recorder (v0) and example.toml
  hl-publish    offline disclosure checks and daily manifests for staged public recordings
  hl-loader     schema migrations; loads archives and recordings into ClickHouse; quality checks
  hl-risk       limits and staged shutdown                       (empty)
  hl-live       single-coin paper session capture and scheduling
  hl-watchdog   independent cancel-all process                   (empty)
docs/           documentation map, architecture, RFCs
research/       Python tools for feed timing and weekly roster churn, tests and a feed-timing report
schema/         ClickHouse migrations, applied by hl-loader
schema/local/   local ClickHouse setup, loading files and checking data quality
scripts/probes/ one-off exploration scripts and what they measured
.github/        CI workflow
```

## Conventions in one paragraph

Prices, sizes and money are `i64` fixed-point values with eight decimals;
accounting never uses floating point. Every event carries `ts_exch` (block
time) and `ts_local` (when the strategy sees it). [HL-95](venues/hyperliquid.md#hl-95)
A strategy depends only on
`hl-strategy` and `hl-types`, so the same strategy can run in backtests, paper
sessions and a future live client. Raw files are the source of truth for
data. ClickHouse can always be rebuilt from them.
