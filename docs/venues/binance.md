# Binance venue facts

Entry conventions and citation rules are in the [register guide](README.md).

## Rules

### BN-1

Binance limits each USD-M futures WebSocket connection to 10 incoming messages per second.

- **Kind:** rule
- **Source:** [WebSocket connection rules](https://developers.binance.com/en/docs/products/derivatives-trading-usds-futures/websocket-market-streams/Connect)
- **Checked:** 2026-11-25
- **Status:** confirmed

### BN-2

Withdrawn before publication: a listing observation for one contract, not used by any decision.

- **Kind:** rule
- **Source:** [Exchange information documentation](https://developers.binance.com/en/docs/catalog/core-trading-derivatives-trading-usd-s-m-futures/api/rest-api/market-data)
- **Checked:** 2026-11-25 (documentation review of the 2026-11-24 observation)
- **Status:** unverified. The public documentation reviewed does not establish the listing on the sample date.

### BN-3

Withdrawn before publication: a listing observation for one contract, not used by any decision.

- **Kind:** rule
- **Source:** [Exchange information documentation](https://developers.binance.com/en/docs/catalog/core-trading-derivatives-trading-usd-s-m-futures/api/rest-api/market-data)
- **Checked:** 2026-11-25 (documentation review of the 2026-11-24 observation)
- **Status:** unverified. The public documentation reviewed does not establish the listing on the sample date.

### BN-4

Withdrawn before publication: a listing observation for one contract, not used by any decision.

- **Kind:** rule
- **Source:** [Exchange information documentation](https://developers.binance.com/en/docs/catalog/core-trading-derivatives-trading-usd-s-m-futures/api/rest-api/market-data)
- **Checked:** 2026-11-25 (documentation review of the 2026-11-24 observation)
- **Status:** unverified. The public documentation reviewed does not establish the listing on the sample date.

### BN-5

Withdrawn before publication: a listing observation for one contract, not used by any decision.

- **Kind:** rule
- **Source:** [Exchange information documentation](https://developers.binance.com/en/docs/catalog/core-trading-derivatives-trading-usd-s-m-futures/api/rest-api/market-data)
- **Checked:** 2026-11-25 (documentation review of the 2026-11-24 observation)
- **Status:** unverified. The public documentation reviewed does not establish the listing on the sample date.

## Feeds and archives

### BN-6

USD-M futures market streams use `wss://fstream.binance.com`, with book streams on `/public/ws` and `aggTrade` on `/market/ws`.

- **Kind:** interface
- **Source:** [WebSocket route mapping](https://developers.binance.com/en/docs/products/derivatives-trading-usds-futures/websocket-market-streams/Important-WebSocket-Change-Notice)
- **Checked:** 2026-11-25
- **Status:** confirmed

### BN-7

The old `/ws` path acknowledged an `aggTrade` subscription but delivered no trades in the 2026-11-24 recorder experiment. Binance documents that an unrouted connection delivers only Public streams.

- **Kind:** interface
- **Source:** [Recorder route observation](../measurements/2026-11-24-recorder-feeds.md#binance-route) and [WebSocket connection rules](https://developers.binance.com/en/docs/products/derivatives-trading-usds-futures/websocket-market-streams/Connect)
- **Checked:** 2026-11-25
- **Status:** confirmed

### BN-8

The public USD-M streams accept an unsigned JSON subscription with `method` set to `SUBSCRIBE`, a `params` array of stream names and an `id`.

- **Kind:** interface
- **Source:** [Live subscriptions](https://developers.binance.com/en/docs/products/derivatives-trading-usds-futures/websocket-market-streams/Live-Subscribing-Unsubscribing-to-streams)
- **Checked:** 2026-11-25
- **Status:** confirmed

### BN-9

The symbol portion of a Binance stream name is lowercase, as in `btcusdt@bookTicker`.

- **Kind:** interface
- **Source:** [WebSocket connection rules](https://developers.binance.com/en/docs/products/derivatives-trading-usds-futures/websocket-market-streams/Connect)
- **Checked:** 2026-11-25
- **Status:** confirmed

### BN-10

`<symbol>@bookTicker` reports changes to the best bid or ask price or quantity in real time. The payload gives prices in `b` and `a`, and quantities in `B` and `A`.

- **Kind:** interface
- **Source:** [Individual Symbol Book Ticker Streams](https://developers.binance.com/en/docs/catalog/core-trading-derivatives-trading-usd-s-m-futures/api/ws-streams/public)
- **Checked:** 2026-11-25
- **Status:** confirmed

### BN-11

`<symbol>@depth5@100ms` reports the top five bid and ask levels at the documented 100 ms update speed.

- **Kind:** interface
- **Source:** [Partial Book Depth Streams](https://developers.binance.com/en/docs/catalog/core-trading-derivatives-trading-usd-s-m-futures/api/ws-streams/public)
- **Checked:** 2026-11-25
- **Status:** confirmed

### BN-12

`<symbol>@aggTrade` reports market fills aggregated by price and taking side, with a documented 100 ms update speed.

- **Kind:** interface
- **Source:** [Aggregate Trade Streams](https://developers.binance.com/en/docs/catalog/core-trading-derivatives-trading-usd-s-m-futures/api/ws-streams/market)
- **Checked:** 2026-11-25
- **Status:** confirmed

### BN-13

The USD-M WebSocket payloads the recorder subscribes to identify their event type in the `e` field.

- **Kind:** interface
- **Source:** [Public stream schemas](https://developers.binance.com/en/docs/catalog/core-trading-derivatives-trading-usd-s-m-futures/api/ws-streams/public) and [Market stream schemas](https://developers.binance.com/en/docs/catalog/core-trading-derivatives-trading-usd-s-m-futures/api/ws-streams/market)
- **Checked:** 2026-11-25
- **Status:** confirmed

### BN-14

The recorded Binance feeds carry Binance exchange timestamps in `E` (event time) and `T` (transaction time for book data, trade time for `aggTrade`). These fields describe events on Binance's clock.

- **Kind:** interface
- **Source:** [Public stream schemas](https://developers.binance.com/en/docs/catalog/core-trading-derivatives-trading-usd-s-m-futures/api/ws-streams/public) and [Market stream schemas](https://developers.binance.com/en/docs/catalog/core-trading-derivatives-trading-usd-s-m-futures/api/ws-streams/market)
- **Checked:** 2026-11-25
- **Status:** confirmed

### BN-15

Quieter Binance contracts can have several seconds between `aggTrade` messages.

- **Kind:** interface
- **Source:** [Aggregate Trade Streams](https://developers.binance.com/en/docs/catalog/core-trading-derivatives-trading-usd-s-m-futures/api/ws-streams/market)
- **Checked:** 2026-11-25 (documentation review)
- **Status:** unverified. The documentation gives an update speed but no measured gap between `aggTrade` messages for a named contract over a stated interval.

### BN-18

The USD-M futures `bookTicker`, partial-depth and `aggTrade` payloads identify the venue symbol in `s`. The official examples use `BNBUSDT` for `bookTicker` and `aggTrade`, and `BTCUSDT` for partial depth.

- **Kind:** interface
- **Source:** [Public stream schemas](https://developers.binance.com/en/docs/catalog/core-trading-derivatives-trading-usd-s-m-futures/api/ws-streams/public) and [Market stream schemas](https://developers.binance.com/en/docs/catalog/core-trading-derivatives-trading-usd-s-m-futures/api/ws-streams/market)
- **Checked:** 2026-11-26
- **Status:** confirmed

## Measured by us

### BN-16

The recorder experiments reported about 3,000 `bookTicker` messages per second for each of BTC and ETH.

- **Kind:** measured
- **Source:** [Recorder measurements](../measurements/2026-11-24-recorder-feeds.md#binance-message-rates)
- **Checked:** 2026-11-24, a workstation outside the recording region around 03:05 UTC, short recorder runs, exact rate-sample duration unrecorded
- **Status:** confirmed

### BN-17

Recording full `bookTicker` for five contracts produced about 10 MB of zstd level-3 data in 75 seconds. At that sample rate, decimal units give about 11.52 GB per day, rounded to 12 GB per day in the recorder README.

- **Kind:** measured
- **Source:** [Recorder measurements](../measurements/2026-11-24-recorder-feeds.md#binance-message-rates)
- **Checked:** 2026-11-24, 75 seconds, five contracts, a workstation outside the recording region around 03:05 UTC
- **Status:** confirmed
