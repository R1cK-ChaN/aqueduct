# Documentation map

| Document | What it owns |
| --- | --- |
| [README](../README.md) · [Simplified Chinese](../README.zh-CN.md) | Project overview, status and a quickstart with the sample recording |
| [Usage](usage.md) | Build and test commands, recording backtests, paper sessions, directory layout and conventions |
| [AGENTS](../AGENTS.md) | Working rules for contributors |
| [Architecture](ARCHITECTURE.md) | Crate responsibilities, dependency rules and data flow |
| [RFCs](rfc/) | Interface and rule decisions with their reasons |
| [Session file reference](session-file.md) | Records written by backtests and paper sessions |
| [Venue register](venues/README.md) | Venue rules, feed and archive formats, and the measurements used in decisions |
| [Measurement notes](measurements/README.md) | Dated inputs, methods, results and limits of measurements cited by the venue register |
| [Probes](../scripts/probes/README.md) | Exploration scripts and their measurements |
| [Feed timing report](../research/reports/feed-timing-2026-11.md) | Timing and coverage of a recorder sample |
| [ClickHouse schema](../schema/README.md) | Tables and migration rules |
| [Local ClickHouse](../schema/local/README.md) | Starting ClickHouse, loading files and checking data quality |
| [Recording manifest](../crates/hl-publish/README.md) | Checking a day of staged recordings and building its manifest |

## Which source wins

1. RFCs record interface and rule decisions and their reasons.
2. Rustdoc gives the precise contract for each public item.
3. Code and tests show what the system does today.

When these disagree, use the documented decision to identify which source
needs correcting.

## What to update

| Change | Update |
| --- | --- |
| A venue rule, feed or archive format, or measurement that a decision depends on | Its entry and Checked date in the [venue register](venues/README.md), then every document that cites the id |
| Shared types, `Strategy`, `OrderGateway` or another cross-crate contract | A new or amended RFC and the rustdoc |
| A crate's responsibility or a dependency between crates | `ARCHITECTURE.md` |
| Build, test or setup steps | `README.md` |
| Contributor rules | `AGENTS.md` |
| ClickHouse tables | A new migration in `schema/` |
| The raw recording format | RFC 0002 and `hl-codec::rawlog`. Keep the meaning of existing kind codes. |

## RFCs

- [0001: Core types, strategy interface and order gateway](rfc/0001-core-types-and-interfaces.md)
- [0002: Raw recording format and recorder behaviour](rfc/0002-raw-recording-format.md)
- [0003: Decoding Hyperliquid market data](rfc/0003-hyperliquid-market-data-decoding.md)
- [0004: ClickHouse schema and loading](rfc/0004-clickhouse-schema-and-loading.md)
- [0005: Data-quality checks](rfc/0005-data-quality-checks.md)
- [0006: Event and time model](rfc/0006-event-and-time-model.md)
- [0007: Fill models](rfc/0007-fill-models.md)
- [0008: Paper trading with one timing rule for backtest and paper](rfc/0008-paper-trading.md)
- [0009: Instrument identity and venue metadata](rfc/0009-instrument-identity.md)
- [0010: A public dataset of Hyperliquid recordings](rfc/0010-public-recording-dataset.md)
