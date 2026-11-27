# Recorder feeds, 2026-11-24

Which subscriptions returned data, and how much disk space did the feeds use?

Redacted on 2026-11-27 to remove the recorder site, instrument lists, disk capacity and repository revision.

## Method and sample

One-minute runs from a workstation outside the recording region around
03:05 UTC, zstd level 3. The separate five-contract `bookTicker` sample
below ran for 75 seconds. The recorder subscribed to the feeds and wrote
compressed files.
The daily volumes in the table use the rates from the one-minute runs.

## Subscription inputs

The 2026-11-24 recorder example supplied the inputs for the one-minute
runs. The recorder substituted each configured symbol for `{symbol}` in each
`subscribe_each` message. It sent each `subscribe_once` message once per
connection.

### `hl`

The recorder subscribed to eleven perpetuals across the main dex and HIP-3 markets.

```toml
url = "wss://api.hyperliquid.xyz/ws"
subscribe_each = [
    '{"method":"subscribe","subscription":{"type":"l2","c":"{symbol}"}}',
    '{"method":"subscribe","subscription":{"type":"bbo","coin":"{symbol}"}}',
    '{"method":"subscribe","subscription":{"type":"trades","coin":"{symbol}"}}',
    '{"method":"subscribe","subscription":{"type":"activeAssetCtx","coin":"{symbol}"}}',
]
subscribe_once = [
    '{"method":"subscribe","subscription":{"type":"fastAssetCtxs"}}',
]
```

### `binance-public`

The recorder subscribed to `bookTicker` for four contracts. Its `subscribe_once`
message (id 2) subscribed to `depth5@100ms` for four other contracts.

```toml
url = "wss://fstream.binance.com/public/ws"
subscribe_each = [
    '{"method":"SUBSCRIBE","params":["{symbol}@bookTicker"],"id":1}',
]
```

### `binance-market`

The recorder subscribed to `aggTrade` for eight contracts.

```toml
url = "wss://fstream.binance.com/market/ws"
subscribe_each = [
    '{"method":"SUBSCRIBE","params":["{symbol}@aggTrade"],"id":1}',
]
```

## Results

| Source | Content | Compressed | Per day at that rate |
| --- | --- | --- | --- |
| `hl` | 11 instruments: `l2`, `bbo`, `trades`, `activeAssetCtx`, plus `fastAssetCtxs` | 0.8 to 1.2 MB per minute | 1.1 to 1.7 GB |
| `binance-public` | `bookTicker` for 4 contracts, `depth5@100ms` for 4. [BN-10](../venues/binance.md#bn-10), [BN-11](../venues/binance.md#bn-11) | 0.36 MB per minute | 0.5 GB |
| `binance-market` | `aggTrade` for 8 contracts. [BN-12](../venues/binance.md#bn-12) | 0.25 MB per minute | 0.4 GB |

### Hyperliquid subscriptions

All 45 Hyperliquid subscriptions were acknowledged and produced data,
including the `xyz:` markets and `fastAssetCtxs`. Trades accounted for 57%
of Hyperliquid bytes. The `l2` feed also sent compressed diffs.
[HL-45](../venues/hyperliquid.md#hl-45), [HL-68](../venues/hyperliquid.md#hl-68), [HL-92](../venues/hyperliquid.md#hl-92)

### Binance route

The old Binance route acknowledged `aggTrade` but delivered no trades
during the run. The recorder used `/ws` for this check. [BN-6](../venues/binance.md#bn-6), [BN-7](../venues/binance.md#bn-7)

### Binance message rates

BTC and ETH each produced about 3,000 `bookTicker` messages/s. Recording
five contracts produced about 10 MB in 75 seconds, or roughly 12 GB/day
at that rate.

### Binance contracts

The recorder run identified four contracts as Binance USDT perpetuals.
Their listing status on the sample date remains unverified. The withdrawn
entries are [BN-2](../venues/binance.md#bn-2), [BN-3](../venues/binance.md#bn-3), [BN-4](../venues/binance.md#bn-4) and [BN-5](../venues/binance.md#bn-5).

### Compression

Higher zstd levels gain little on this data: level 9 is 5% smaller than
level 3 for Hyperliquid and 20% smaller for Binance, at several times the
CPU.

## Limits

The recorder report gives no exact duration for the BTC and ETH message
rate sample and does not name the five contracts in the separate sample.

With about 30 Hyperliquid instruments the total is likely 3 to 5 GB per day.

## Register entries that cite this note

- [HL-64](../venues/hyperliquid.md#hl-64)
- [HL-147](../venues/hyperliquid.md#hl-147)
- [BN-7](../venues/binance.md#bn-7)
- [BN-16](../venues/binance.md#bn-16), [BN-17](../venues/binance.md#bn-17)
