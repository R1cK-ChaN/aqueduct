# Architecture

This describes the structure as it is now. Reasons for interface decisions are
in [rfc/](rfc/).

## System boundary

The design separates two roles so research load cannot affect trading.

- The **trading host** handles market capture with `hl-recorder` and paper
  trading with `hl-live paper`.
- The **research machine** holds raw files, ClickHouse, the backtester and
  the Python research code.

Recordings flow from the trading host through object storage to the research
machine.

On the research machine, `hl-loader` reads raw files (official archives and
recordings) and writes the `hl` database. `hl-backtest` reads raw recordings
directly and feeds their records to `hl-run`. The library converts records
into events for the selected coin and replays them through the simulator
and example strategy. After replay, `hl-run` applies fills delivered by the cutoff and
usable BBO midpoints to `hl-ledger` in exchange-time order and returns the
account's profit and inventory extremes. `hl-backtest` formats the JSON
summary.

`hl-backtest` reads each numbered `(source, process_start_wall_ns)` chain
in `file_seq` order, rejecting holes and duplicates. It merges chains by
their next header's wall time and path. An unnumbered legacy file forms
its own chain. `hl-run` derives local event time from the maximum of raw
wall time, anchored monotonic time, exchange time and the preceding local
event time. Gaps omit exchange time. The inclusive stop takes the later of
the final complete record's wall and anchored monotonic times and raises it
to the last event's local time if necessary. [RFC 0008 D4 and D5](rfc/0008-paper-trading.md)

## Crates

| Crate | Responsibility | State |
| --- | --- | --- |
| `hl-types` | Fixed-point values, timestamps, venue-neutral instrument ids derived from venue and symbol, orders, order actions and events. No dependencies. | Implemented |
| `hl-rules` | Looks up perpetual rules by instrument and exchange time, checks prices and sizes, and finds the next valid price. Depends only on `hl-types`. [RFC 0009 D4](rfc/0009-instrument-identity.md#d4-supply-dated-instrument-rules-to-hl-sim) | Implemented |
| `hl-strategy` | `Strategy` trait; `StrategyHost`, which owns the state a strategy reads; the example `FixedSpreadQuoter`. | Implemented |
| `hl-gateway` | `OrderGateway` trait. The Hyperliquid client will be added behind a non-default feature. | Trait only |
| `hl-sim` | Applies post-only actions proved by received market data, matches strict trade-throughs and schedules reports. | Minimal |
| `hl-run` | Uses `hl-codec` to convert BBOs and trades for one coin, applies the trade cutoff and delivery clock, encodes session rows for actions, reports, gaps and timers, and computes the account at the stop. Performs no IO and reads no clock. | Incremental driver, session encoding and accounting |
| `hl-backtest` | Reads and orders compressed recordings, writes the JSON summary and session file, and verifies a session by hashing the inputs and comparing an independent replay field by field. | Recording CLI and replay-check library |
| `hl-codec` | `rawlog` frames recordings, parses `file_open` headers and identifies recording chains. `hyperliquid` owns the live-trade cutoff shared by loading and replay, network names, recording URL mapping, coin ids and dex extraction. It decodes WebSocket book and trade channels, the official L2 archive and `node_fills_by_block` into typed records. A caller-supplied coin resolver selects the records to convert to market events. For dated perpetual rules, `hyperliquid` parses JSON text into `hl-rules` histories and returns the instrument id, ordered versions and a `RuleBook` for the selected network and coin. [RFC 0009 D7](rfc/0009-instrument-identity.md#rules-file) | Partial. Context channels and Binance are planned. |
| `hl-book` | `L2Book`: the visible levels of one instrument, built from snapshots and diffs. An L4 book is planned. | L2 only |
| `hl-ledger` | One-instrument position, trading cash, reported fill fees, traded notional and marked gross and net profit. Replay and paper trading share this accounting. | v1, funding deferred |
| `hl-recorder` | `run_source` runs one configured WebSocket feed with a caller-supplied `SourceSink`. The recorder sink stamps and queues raw records for the hourly zstd writer. The optional `hl-public-v1` policy checks the public source configuration and outgoing texts and limits connection attempts. [RFC 0010 D2–D3](rfc/0010-public-recording-dataset.md#d2-fixed-universe-for-v1) | v0 recorder CLI |
| `hl-publish` | Screens one staged day with the recorder's public send policy and the codec's record and chain types. Writes a local manifest of accepted files, gaps, observed coverage and supplied quality results. [RFC 0010 D5](rfc/0010-public-recording-dataset.md#d5-seal-the-day-with-a-manifest) | Offline seal command |
| `hl-loader` | Applies the schema migrations; loads the official L2 archive, `node_fills_by_block` and Hyperliquid recordings into ClickHouse, idempotently; finds conflicting rows at load time; runs the daily data-quality checks. | Implemented for those three inputs |
| `hl-risk` | Limits and staged shutdown. | Empty |
| `hl-live` | Runs one configured feed, stamps and admits raw records, paces the shared driver, and seals the raw chain and paper session on shutdown. | Single-coin paper CLI with simulated orders |
| `hl-watchdog` | Independent cancel-all process. | Empty |

## Dependency rules

The arrows point to direct workspace dependencies at runtime. The `hl-risk`
and `hl-watchdog` placeholders have none.

```text
hl-types  <-  hl-rules
hl-types  <-  hl-strategy
hl-types  <-  hl-gateway
hl-types, hl-gateway  <-  hl-sim
hl-types  <-  hl-ledger
hl-types, hl-codec, hl-strategy, hl-gateway, hl-sim, hl-ledger  <-  hl-run
hl-types, hl-codec, hl-strategy, hl-sim, hl-run  <-  hl-backtest
hl-types, hl-codec, hl-strategy, hl-sim, hl-run, hl-recorder  <-  hl-live
hl-codec  <-  hl-recorder
hl-types, hl-codec, hl-recorder  <-  hl-publish
hl-types, hl-rules  <-  hl-codec
hl-types  <-  hl-book
hl-types, hl-codec, hl-book  <-  hl-loader
```

- `hl-types` depends on nothing.
- `hl-strategy` depends only on `hl-types`. It never depends on a gateway,
  the simulator or a crate that performs IO.
- `hl-sim` depends on `hl-gateway` for the trait and must not gain network or
  signing dependencies through it.
- `hl-run` uses `hl-codec` to convert one record at a time, drives the
  simulator and strategy, and uses `hl-ledger` for its typed accounting
  result. It performs no IO and reads no clock. Its only direct external
  dependency is the workspace's existing `serde_json` for record metadata.
  The incremental driver accepts one input or a time cutoff per call.
  Recording replay feeds it the selected coin's event slice.
- `hl-backtest` owns file IO, decompression, header reading, file ordering
  and chain validation. It supplies `hl-run` with file starts carrying
  both header stamps, the optional process anchor and any chain change,
  followed by complete records and truncation inputs. It holds the
  selected coin's events in memory and owns the command line and JSON
  formatting.
- `hl-ledger` depends only on `hl-types`. It books the fee carried by each
  fill, uses checked exact fixed-point arithmetic, and performs no IO.
  Funding and live trading integration are deferred.
- `hl-recorder` depends on `hl-codec` for the file format and on nothing else
  in the workspace. Its WebSocket client supplies the feed for `hl-live paper`.
- `hl-codec` turns bytes into values and opens no files or sockets. It may
  depend on parsing libraries (JSON, inflate for compressed payloads);
  file-level compression stays with its callers.
- `hl-book` depends only on `hl-types`. It uses `hl-codec` in tests, to
  replay recorded data.
- `hl-loader` is the only crate that talks to ClickHouse. It embeds the
  migrations in `schema/`, so the schema and the row types it writes always
  come from the same commit.

## How one event flows

```text
market event ──> driver ──> StrategyHost.on_event ──> Strategy.on_event
                   │                 │
                   │                 └─ returns Actions (order actions, timers)
                   ├─ order actions ──> OrderGateway.submit
                   └─ OrderGateway.poll_events ──> order events ──> StrategyHost.on_event
```

`hl-run` gives each received input to the simulator and then to the
strategy host. A BBO or trade with exchange time strictly past an action's
effect time establishes its outcome. The simulator schedules reports no
earlier than both the configured delay and receipt of the proving input.
At a shared local time all market and gap callbacks precede reports, then
timers fire in registration order. The driver checks for due reports after
each timer. Offline replay and `hl-live paper` use these callbacks to
record decisions in order. The paper process reads its own feed and seals the
session. `hl-backtest run --check-session` verifies it offline.
[Session file reference](session-file.md)
[RFC 0008 D2 and D3](rfc/0008-paper-trading.md)
Block execution belongs to the later research model. [HL-1](venues/hyperliquid.md#hl-1) · [RFC 0006](rfc/0006-event-and-time-model.md)

Each environment uses the driver and gateway listed below.

| Environment | Driver | Gateway |
| --- | --- | --- |
| Backtest | `hl-run::replay` drives the incremental loop through the final complete record's local time, inclusive | `hl-sim` |
| Paper trading | `hl-live paper` on its own recorded feed | `hl-sim` |
| Live trading | `hl-live` on the live feed (not written) | Hyperliquid client (not written) |

Backtest and paper trading share the strategy and `StrategyHost`. The live
trading design uses the same interfaces.
