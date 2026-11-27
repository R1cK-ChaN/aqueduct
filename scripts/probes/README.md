# Probe scripts

One-off scripts used on 2026-11-23 to check Hyperliquid market structure, the
official S3 archives and the 0xArchive evaluation sample. [VN-3](../../docs/venues/vendors.md#vn-3)
They are exploration code: no tests, minimal error handling, results printed
to stdout.

## Requirements

- Python 3 with `websocket-client`, `lz4` and `pyarrow`.
- AWS CLI credentials for a profile that pays for the archive requests. [HL-96](../../docs/venues/hyperliquid.md#hl-96)

The archive and 0xArchive scripts read downloaded files. Keep those files in a
directory separate from the scripts and run with `python3 -I`, so nothing in a
data directory can be imported as a module. Each of these scripts appends the
user site-packages path itself because `-I` disables it.

## Live market probes (public API, no account)

The probes read market data from the public HTTP and WebSocket endpoints. [HL-38](../../docs/venues/hyperliquid.md#hl-38), [HL-40](../../docs/venues/hyperliquid.md#hl-40), [HL-41](../../docs/venues/hyperliquid.md#hl-41)

| Script | What it does |
| --- | --- |
| `market_snapshot.py` | Ranks main-dex perps by 24h volume from `metaAndAssetCtxs`, writes `perps_rank.json`, and prints `perpDexs`. [HL-58](../../docs/venues/hyperliquid.md#hl-58), [HL-59](../../docs/venues/hyperliquid.md#hl-59) |
| `hip3_snapshot.py` | Lists dexes with `perpDexs`, reads `metaAndAssetCtxs` for each HIP-3 dex, and writes `hip3_rank.json`. Metadata can include `growthMode`. The reported `deployerFeeScale` field is unverified. [HL-59](../../docs/venues/hyperliquid.md#hl-59), [HL-58](../../docs/venues/hyperliquid.md#hl-58), [HL-60](../../docs/venues/hyperliquid.md#hl-60), [HL-61](../../docs/venues/hyperliquid.md#hl-61) Also prints fees and rate limits for a dummy address. [HL-62](../../docs/venues/hyperliquid.md#hl-62), [HL-63](../../docs/venues/hyperliquid.md#hl-63) |
| `ws_spread_probe.py` | Reads the two JSON files above, subscribes to `bbo` and `trades` for the top 45 main-dex and 25 HIP-3 perps for 300 s, prints time-weighted spread, tick size, trade rate and top-of-book size per coin, plus `l2Book` and `l2` push intervals for BTC. |
| `ws_cadence_probe.py` | 30 s check of `l2Book`, `bbo`, `trades` and `l2` message intervals for BTC. |

Run the first two before `ws_spread_probe.py`, from the same directory.

### Latency probe

`latency_probe.py [market_seconds]` measures DNS, TCP and TLS time, then
times small `/info` requests and `/exchange` requests that are rejected for
an invalid signature. [HL-37](../../docs/venues/hyperliquid.md#hl-37), [HL-39](../../docs/venues/hyperliquid.md#hl-39), [HL-132](../../docs/venues/hyperliquid.md#hl-132), [HL-133](../../docs/venues/hyperliquid.md#hl-133)
It repeats those requests over WebSocket `post`. [HL-42](../../docs/venues/hyperliquid.md#hl-42)
For `bbo` and `trades`, it subtracts the block time from the local receive
time. [HL-41](../../docs/venues/hyperliquid.md#hl-41), [HL-95](../../docs/venues/hyperliquid.md#hl-95), [HL-134](../../docs/venues/hyperliquid.md#hl-134), [HL-135](../../docs/venues/hyperliquid.md#hl-135)
It uses no key or account. The lag numbers need a synchronised clock.
Record `chronyc tracking` next to the output.

Results from 2026-11-24, both machines run at the same time (02:10 UTC,
150 s of market data):

| Measure | Cloud VM in Tokyo | Second site on consumer broadband | Register |
| --- | --- | --- | --- |
| TCP connect | 3.3 ms | 9 ms | — |
| HTTPS `/info` round trip, p50 | 18 ms | 92 ms | [HL-39](../../docs/venues/hyperliquid.md#hl-39), [HL-132](../../docs/venues/hyperliquid.md#hl-132) |
| HTTPS `/exchange` rejected, min / p50 | 5 / 17 ms | 77 / 89 ms | [HL-39](../../docs/venues/hyperliquid.md#hl-39), [HL-133](../../docs/venues/hyperliquid.md#hl-133) |
| `bbo` lag, min / p50 / p99 | 178 / 254 / 522 ms | 222 / 307 / 534 ms | [HL-134](../../docs/venues/hyperliquid.md#hl-134) |
| `trades` lag, p50 | 273 ms | 326 ms | [HL-135](../../docs/venues/hyperliquid.md#hl-135) |

The `/exchange` timer stops at the rejection response. Measuring time to
block inclusion would require an accepted order. [HL-133](../../docs/venues/hyperliquid.md#hl-133)

## Official archive probes

Download the official L2 and node-fills samples first (replace `<profile>`).
The archive layouts are given in [HL-97](../../docs/venues/hyperliquid.md#hl-97), [HL-105](../../docs/venues/hyperliquid.md#hl-105).

```bash
aws s3 ls s3://hyperliquid-archive/market_data/ --request-payer requester --region us-east-1 --profile <profile>
```

```bash
aws s3 cp s3://hyperliquid-archive/market_data/20260601/9/l2Book/BTC.lz4 data/20260601_9_BTC.lz4 --request-payer requester --region us-east-1 --profile <profile>
```

```bash
aws s3 cp s3://hl-mainnet-node-data/node_fills_by_block/hourly/20261122/6.lz4 data/fills_20261122_6.lz4 --request-payer requester --region ap-northeast-1 --profile <profile>
```

| Script | Usage | Output |
| --- | --- | --- |
| `archive_l2_probe.py` | `python3 -I archive_l2_probe.py data/*_BTC.lz4` | Per file: snapshot count, interval percentiles, levels per side, crossed books, lz4 ratio. |
| `archive_fills_probe.py` | `python3 -I archive_fills_probe.py data/fills_20261122_6.lz4` | Envelope and fill fields, row and coin counts, HIP-3 and spot presence, block continuity, node lag. |

## 0xArchive sample day checks

The probes use the 2026-11-10 evaluation sample. [VN-3](../../docs/venues/vendors.md#vn-3)
Put one market's Parquet files in a directory such as `oxa/hyperliquid_btc/`
and supply the corresponding official fills hours and the 15:00 UTC L2
file. [HL-105](../../docs/venues/hyperliquid.md#hl-105), [HL-97](../../docs/venues/hyperliquid.md#hl-97)

| Script | Usage | Checks |
| --- | --- | --- |
| `oxa_parquet_schema.py` | `python3 -I oxa_parquet_schema.py oxa/hyperliquid_btc/*.parquet` | Prints schema and first two rows of each file. |
| `oxa_trades_recon.py` | `python3 -I oxa_trades_recon.py BTC oxa/hyperliquid_btc/trades.parquet fills/15.lz4 [more hours]` | Joins on (tid, user, oid, side) and compares 12 fields against official fills. Pass neighbouring hours to match rows near a file boundary. [VN-4](../../docs/venues/vendors.md#vn-4), [HL-114](../../docs/venues/hyperliquid.md#hl-114) |
| `oxa_l4_inspect.py` | `python3 -I oxa_l4_inspect.py oxa/hyperliquid_btc [official_l2.lz4]` | Inspects checkpoints, diffs, order statuses and official snapshot times. The 15:00–15:15 window is hard-coded for 2026-11-10. [VN-5](../../docs/venues/vendors.md#vn-5), [VN-6](../../docs/venues/vendors.md#vn-6) |
| `oxa_l4_recon.py` | `python3 -I oxa_l4_recon.py oxa/hyperliquid_btc BTC fills/15.lz4 [official_l2.lz4]` | Replays L4 diffs from the first checkpoint and compares against later checkpoints, the vendor's L2 grid, official L2 snapshots and resting orders for official maker fills. [VN-5](../../docs/venues/vendors.md#vn-5), [VN-6](../../docs/venues/vendors.md#vn-6), [VN-7](../../docs/venues/vendors.md#vn-7) |
| `oxa_queue_order.py` | `python3 -I oxa_queue_order.py oxa/hyperliquid_btc` | Counts price levels whose queue order after a FIFO replay differs from the next checkpoint. [VN-5](../../docs/venues/vendors.md#vn-5), [VN-10](../../docs/venues/vendors.md#vn-10) |

For HIP-3 pass the dex-prefixed coin, for example `xyz:TSLA`. [HL-55](../../docs/venues/hyperliquid.md#hl-55), [VN-8](../../docs/venues/vendors.md#vn-8)
Omit the official L2 argument for this comparison. HIP-3 coverage in the
official archive is unverified. [HL-101](../../docs/venues/hyperliquid.md#hl-101)

## Not kept here

The downloaded sample files (about 500 MB) are not in the repository.
Do not redistribute the 0xArchive sample. [VN-1](../../docs/venues/vendors.md#vn-1)
