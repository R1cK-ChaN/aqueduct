# Aqueduct

[English](README.md) · [简体中文](README.zh-CN.md)

[![CI](https://github.com/R1cK-ChaN/aqueduct/actions/workflows/ci.yml/badge.svg)](https://github.com/R1cK-ChaN/aqueduct/actions/workflows/ci.yml)
[![Licence: MIT OR Apache-2.0](https://img.shields.io/badge/licence-MIT%20OR%20Apache--2.0-blue.svg)](#licence)

Aqueduct is the plumbing for market making on order-book DEXs. It records
market data, replays it against a simulated venue, and runs the same
strategy as a paper session on the live feed. It supports Hyperliquid
perpetuals today.

## Why this is open

More market makers mean tighter spreads, and everyone who trades gets a
better price. To become one you first need a recorder, a replay that gives
the same answer twice, and a paper mode. That is a lot of work, and it has
little to do with your strategy. We did that work and opened it, so you, or
your coding agent, can start at the strategy. The name says the same thing:
an aqueduct carries water, and the water here is liquidity.

## Try it

You need Linux, Rust through [rustup](https://rustup.rs) (the toolchain
version is pinned) and `zstd`.

The repository includes a 60-second recording of one market, kept as a test
fixture. These three commands compress it, run the example quoter on it, and
verify the session that run wrote.

```bash
zstd -q -o /tmp/sample.hlrec.zst crates/hl-codec/tests/fixtures/hl_ws_pons_20261124.hlrec
```

```bash
cargo run --release -p hl-backtest -- run \
  --coin PONS --half-spread 0.00002 --qty 500 \
  --order-latency-ms 400 --report-latency-ms 250 --maker-fee-bps 1.5 \
  --session-out /tmp/session.jsonl /tmp/sample.hlrec.zst
```

```bash
cargo run --release -p hl-backtest -- run --check-session /tmp/session.jsonl
```

The second command prints a JSON summary of the run. The third prints
`session matches` and a row count. This shows that the pipeline works. It
shows nothing about a strategy: it is sixty seconds of one market. The first
two commands refuse to overwrite a file, so delete `/tmp/sample.hlrec.zst`
and `/tmp/session.jsonl` before you run them again.

[docs/usage.md](docs/usage.md) has every flag and output field, and how to
record your own data and run a paper session.

## The pipeline

| Step | Program | Input | Output |
| --- | --- | --- | --- |
| Record | `hl-recorder` | a public market-data feed | raw recording files |
| Store and check | `hl-loader` | recordings and the official archives | ClickHouse tables, daily data-quality checks |
| Backtest | `hl-backtest run` | recordings and your strategy | a summary and a session file |
| Paper | `hl-live paper` | the live feed and your strategy | its own recording and a session file |
| Verify | `hl-backtest run --check-session` | a session file and its recordings | a match, or the first field that differs |

Your strategy is the same code in the backtest and in the paper session.

## What is not here

- Strategies. There is one example, `FixedSpreadQuoter`, so that the
  pipeline has something to run.
- A research method. Which runs to make and how to judge them is up to you.
- Anything that sends an order to a venue. The order gateway is an
  interface, and what stands behind it today is a simulator.
- Results.

The simulator is a model. What it assumes, and what would have to be
measured on a live account to test each assumption, is listed under
"Limits against live trading" in
[RFC 0008 D8](docs/rfc/0008-paper-trading.md#d8-replay-the-recording-and-compare-decisions-in-order).

## Status

| Part | Crates | State |
| --- | --- | --- |
| Recorder for public feeds | `hl-recorder` | Working. [HL-41](docs/venues/hyperliquid.md#hl-41), [BN-8](docs/venues/binance.md#bn-8) |
| Decoders and L2 book | `hl-codec`, `hl-book` | Working for Hyperliquid frames and the official archives. Books rebuild from diffs. [HL-97](docs/venues/hyperliquid.md#hl-97), [HL-105](docs/venues/hyperliquid.md#hl-105) |
| Loading and daily data-quality checks | `hl-loader` | Working |
| Simulated venue | `hl-sim` | Working, minimal: post-only orders, filled by a trade strictly through the price |
| Replay and backtest | `hl-run`, `hl-backtest` | Working, one coin per run |
| Paper session | `hl-live` | Working |
| Dated price and size rules | `hl-rules` | Written, not yet applied by the simulator. [RFC 0009 D7](docs/rfc/0009-instrument-identity.md#d7-supply-dated-rules-to-backtest-and-paper-runs) |
| Public recording dataset | `hl-publish` | Designed, not yet available. [RFC 0010 D8](docs/rfc/0010-public-recording-dataset.md#d8-order-of-later-issues) |
| Live order gateway | `hl-gateway` | Interface only |
| Risk limits and watchdog | `hl-risk`, `hl-watchdog` | Not started |

## Rules the code keeps

- The same inputs give the same output, byte for byte. Nothing a backtest
  result depends on reads the wall clock or uses unseeded randomness.
- A session can be replayed and compared field by field, with
  `hl-backtest run --check-session`.
- A strategy does no IO. It receives events and returns actions.
- A statement about how a venue behaves cites an entry in the venue
  register, and the entry has a source and the date it was checked.

These rules are what let a coding agent work here as well as a person: it
can check its own result. The working rules for both are in
[AGENTS.md](AGENTS.md).

## Documentation

- [Usage](docs/usage.md): build, test, run a backtest and a paper session.
- [Documentation map](docs/INDEX.md): every document and what it owns.
- [Architecture](docs/ARCHITECTURE.md): crates, dependencies and data flow.
- [RFC 0001](docs/rfc/0001-core-types-and-interfaces.md): why the core
  interfaces look the way they do.
- [Venue register](docs/venues/README.md): venue rules and measurements,
  each with a source.

## Roadmap

- Lighter is the next venue. Crates are named `hl-` because they were
  written for one venue. They get new names when the second one arrives.
- Dated price and size rules in the simulator.
- A public dataset of recordings.
- A `learn/` directory: what a maker does, and the ways a backtest misleads.

## Contributing

Read [AGENTS.md](AGENTS.md) first. A change to a shared interface starts as
an RFC in `docs/rfc/`. Issues and pull requests are in English.

## Licence

You may use this code under either the MIT License or the Apache License,
Version 2.0, at your option (`MIT OR Apache-2.0`).
[MIT](LICENSE-MIT) · [Apache-2.0](LICENSE-APACHE)

Unless you explicitly state otherwise, any contribution you intentionally
submit for inclusion in this project, as defined in the Apache License,
Version 2.0, is dual-licensed as above, without additional terms or conditions.
