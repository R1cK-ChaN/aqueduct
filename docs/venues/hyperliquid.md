# Hyperliquid venue facts

Entry conventions and citation rules are in the [register guide](README.md).

## Rules

[Blocks and priority](#hl-1) · [Orders and modifications](#hl-15) · [Price, size and fees](#hl-23) · [Request allowances and cancels](#hl-32)

### HL-1

Hyperliquid executes order-book actions inside consensus batches in L1 blocks.

- **Kind:** rule
- **Source:** [Order book](https://hyperliquid.gitbook.io/hyperliquid-docs/hypercore/order-book)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-2

Hyperliquid states that a block usually contains one or two consensus batches.

- **Kind:** rule
- **Source:** [Order book](https://hyperliquid.gitbook.io/hyperliquid-docs/hypercore/order-book)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-3

Within a consensus batch, Hyperliquid processes non-cancel actions that send no GTC
or IOC first, cancels second, and actions containing a GTC or IOC last. The first
class includes ALO placements.

- **Kind:** rule
- **Source:** [Order book](https://hyperliquid.gitbook.io/hyperliquid-docs/hypercore/order-book)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-4

Within each action class, execution follows the block proposer's proposed ordering.

- **Kind:** rule
- **Source:** [Order book](https://hyperliquid.gitbook.io/hyperliquid-docs/hypercore/order-book)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-5

A modify belongs to the action class of its replacement order.

- **Kind:** rule
- **Source:** [Order book](https://hyperliquid.gitbook.io/hyperliquid-docs/hypercore/order-book)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-6

The order book normally matches by price, then queue position at that price. Time of
placement determines the initial order subject to the ALO priority rules in
[HL-8](#hl-8) and [HL-9](#hl-9).

- **Kind:** rule
- **Source:** [Order book](https://hyperliquid.gitbook.io/hyperliquid-docs/hypercore/order-book), [ALO priority](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/priority-fees#alo-priority)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-7

Priority grouping requires every order to use a non-outcome asset and the batch to
contain either only IOCs or only non-reduce-only ALOs.

- **Kind:** rule
- **Source:** [Priority fees](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/priority-fees#order-write-priority)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-8

At each price level, ALO priority sorts the recent queue tail by decreasing priority
rate over a rolling 400 ms window.

- **Kind:** rule
- **Source:** [ALO priority](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/priority-fees#alo-priority)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-9

In the venue's worked example, a younger ALO locks its slot before it is
400 ms old. It stays ahead of a newcomer paying a higher priority rate
because the order behind it has reached 400 ms. The page gives no general
rule for early locking.

- **Kind:** rule
- **Source:** [ALO priority](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/priority-fees#alo-priority)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-10

Hyperliquid states that IOC priority from 0 to 8 bps currently reduces mainnet
end-to-end latency by approximately 45 ms per bp.

- **Kind:** rule
- **Source:** [IOC priority](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/priority-fees#ioc-priority)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-11

For eligible IOCs paying at least 8 bps, the proposer orders transactions within
each 70 ms Unix-time bucket by decreasing priority fee.

- **Kind:** rule
- **Source:** [IOC priority](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/priority-fees#ioc-priority)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-12

Hyperliquid states that cancels and ALOs sent at the same time as IOC and GTC orders
almost always execute first, with prioritization extending across several blocks.

- **Kind:** rule
- **Source:** [Optimizing write latency](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/optimizing-latency#optimizing-write-latency-sending-orders-and-cancels)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-13

Hyperliquid states that a geographically co-located client sending an order receives
a committed response after 0.2 s at the median and 0.9 s at the 99th percentile.

- **Kind:** rule
- **Source:** [HyperCore latency](https://hyperliquid.gitbook.io/hyperliquid-docs/hypercore/overview#latency)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-14

Hyperliquid states that approximately 380 ms is an example end-to-end latency for a
cancel or ALO order.

- **Kind:** rule
- **Source:** [Optimizing write latency](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/optimizing-latency#optimizing-write-latency-sending-orders-and-cancels)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-15

An ALO that would immediately match is canceled on arrival. The same condition is
represented as `BadAloPx` in order errors and `badAloPxRejected` in historical order
statuses.

- **Kind:** rule
- **Source:** [Place an order](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/exchange-endpoint#place-an-order), [Error responses](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/error-responses), [Order status](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint#query-order-status-by-oid-or-cloid)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-16

An IOC executes immediately available quantity within its limit and cancels the
unfilled remainder.

- **Kind:** rule
- **Source:** [Place an order](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/exchange-endpoint#place-an-order), [Order types](https://hyperliquid.gitbook.io/hyperliquid-docs/trading/order-types)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-17

A marketable GTC limit order can execute on arrival, and its unfilled remainder
rests at its limit until filled or canceled.

- **Kind:** rule
- **Source:** [Place an order](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/exchange-endpoint#place-an-order), [Order types](https://hyperliquid.gitbook.io/hyperliquid-docs/trading/order-types)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-18

With `always_place` set, a modify places the replacement even if canceling the
original fails. When the flag is omitted, the replacement must be a non-trigger
order and either ALO or non-executable GTC. Hyperliquid converts the latter to ALO.

- **Kind:** rule
- **Source:** [Modify an order](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/exchange-endpoint#modify-an-order)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-19

A successful modify that omits replacement `c` retains the previous venue-side
`cloid`.

- **Kind:** rule
- **Source:** [Modify an order](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/exchange-endpoint#modify-an-order)
- **Checked:** 2026-11-25
- **Status:** unverified. The page permits an optional replacement `c` field but does not specify automatic retention of the old client ID.

### HL-20

Every rejected modify leaves the original resting order unchanged.

- **Kind:** rule
- **Source:** [Modify an order](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/exchange-endpoint#modify-an-order)
- **Checked:** 2026-11-25
- **Status:** unverified. The documented `always_place` behavior does not specify the resting order's outcome for every replacement failure.

### HL-21

A successful replacement by modify receives a new venue `oid`.

- **Kind:** rule
- **Source:** [Modify an order](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/exchange-endpoint#modify-an-order)
- **Checked:** 2026-11-25
- **Status:** unverified. The page defines the original-order selector but gives no successful-modify response or general ID-retention rule.

### HL-22

A reduce-only order may reduce an existing position without opening a position in
the opposite direction.

- **Kind:** rule
- **Source:** [Order options](https://hyperliquid.gitbook.io/hyperliquid-docs/trading/order-types#order-options)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-23

A non-integer perpetual price has at most five significant figures.

- **Kind:** rule
- **Source:** [Tick and lot size](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/tick-and-lot-size)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-24

The maximum number of decimal places in a perpetual price is `6 - szDecimals`.

- **Kind:** rule
- **Source:** [Tick and lot size](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/tick-and-lot-size)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-25

Integer prices are allowed regardless of their number of significant figures.

- **Kind:** rule
- **Source:** [Tick and lot size](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/tick-and-lot-size)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-26

An order's size must fit the asset's `szDecimals` precision, supplied by the `meta`
response.

- **Kind:** rule
- **Source:** [Tick and lot size](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/tick-and-lot-size)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-27

The published minimum notional for a new perpetual order is $10, where
notional is price times size. `MinTradeNtl` reports a value below the minimum.
We infer that exactly $10 passes this check. The pages give no boundary
example or rounding procedure.

- **Kind:** rule
- **Source:** [Error responses](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/error-responses), [Order status](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint#query-order-status-by-oid-or-cloid), [Notation](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/notation)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-28

Hyperliquid’s main crypto-perp margining scheme uses a USDT-denominated oracle and
USDC for collateral and PnL. HYPE-USD and PURR-USD use USDC-denominated oracles.
HIP-3 deployers choose their dex’s collateral token. In the archived perpetuals in
RFC 0003, `fee`, `closedPnl`, `builderFee` and `deployerFee` used the row’s
`feeToken`, which was USDC on the main dex and USDC, USDT0, USDH or USDE on the
sampled HIP-3 dexes.

- **Kind:** rule
- **Source:** [Contract specifications](https://hyperliquid.gitbook.io/hyperliquid-docs/trading/contract-specifications#crypto-perps), [HIP-3 specification](https://hyperliquid.gitbook.io/hyperliquid-docs/hyperliquid-improvement-proposals-hips/hip-3-builder-deployed-perpetuals#spec), [RFC 0003 sample findings](../rfc/0003-hyperliquid-market-data-decoding.md#verification)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-29

A negative fill fee or maker fee rate represents a rebate.

- **Kind:** rule
- **Source:** [Fill type](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket/subscriptions#data-type-definitions), [Maker rebates](https://hyperliquid.gitbook.io/hyperliquid-docs/trading/fees#maker-rebates)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-30

The fee schedule publishes trading rates and leaves the precision and rounding
mode of the fee charged on each fill unspecified.

- **Kind:** rule
- **Source:** [Fees](https://hyperliquid.gitbook.io/hyperliquid-docs/trading/fees)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-31

Order-priority gas is charged in HYPE, and the node's fill data exposes IOC priority
charges as `priorityGas`.

- **Kind:** rule
- **Source:** [Priority fees](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/priority-fees#order-write-priority)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-141

The perpetual fee schedule lists a tier-0 base maker rate of 0.015% (1.5 bps).

- **Kind:** rule
- **Source:** [Perps fee tiers](https://hyperliquid.gitbook.io/hyperliquid-docs/trading/fees#perps-fee-tiers)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-32

An address earns additional action requests as its cumulative traded volume
increases. The published base rate is one request per USDC of volume.

- **Kind:** rule
- **Source:** [Address-based limits](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/rate-limits-and-user-limits#address-based-limits), [Fee schedule and growth-mode volume weighting](https://hyperliquid.gitbook.io/hyperliquid-docs/trading/fees)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-33

Hyperliquid limits REST requests by their combined weight per IP address.

- **Kind:** rule
- **Source:** [Rate limits](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/rate-limits-and-user-limits)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-143

One IP address may have up to 1,000 WebSocket subscriptions in total.

- **Kind:** rule
- **Source:** [Rate limits and user limits](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/rate-limits-and-user-limits)
- **Checked:** 2026-11-26
- **Status:** confirmed

### HL-144

One IP address may have up to 10 WebSocket connections open at once.

- **Kind:** rule
- **Source:** [Rate limits and user limits](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/rate-limits-and-user-limits)
- **Checked:** 2026-11-26
- **Status:** confirmed

### HL-145

One IP address may open up to 30 new WebSocket connections per minute.

- **Kind:** rule
- **Source:** [Rate limits and user limits](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/rate-limits-and-user-limits)
- **Checked:** 2026-11-26
- **Status:** confirmed

### HL-146

One IP address may send up to 2,000 messages per minute to Hyperliquid
across all its WebSocket connections.

- **Kind:** rule
- **Source:** [Rate limits and user limits](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/rate-limits-and-user-limits)
- **Checked:** 2026-11-26
- **Status:** confirmed

### HL-34

`scheduleCancel` is an account-level dead-man switch that cancels all open orders at
a specified future deadline. The order-status API identifies orders canceled by that
deadline with `scheduledCancel`.

- **Kind:** rule
- **Source:** [Schedule cancel](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/exchange-endpoint#schedule-cancel-dead-mans-switch), [Order status](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint#query-order-status-by-oid-or-cloid)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-35

Newly created accounts can use `scheduleCancel`.

- **Kind:** rule
- **Source:** [Schedule cancel](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/exchange-endpoint#schedule-cancel-dead-mans-switch)
- **Checked:** 2026-11-25
- **Status:** unverified. The page describes scheduling constraints without specifying new-account eligibility.

### HL-36

The venue can cancel an order without an order-owner cancel request, including for
insufficient margin, liquidation, or asset delisting.

- **Kind:** rule
- **Source:** [Order status](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint#query-order-status-by-oid-or-cloid)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-37

Order actions sent to `/exchange` require a signature from an authorized signer. The
API can reject an incorrectly signed action before execution.

- **Kind:** rule
- **Source:** [Exchange endpoint](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/exchange-endpoint#place-an-order), [Signing](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/signing)
- **Checked:** 2026-11-25
- **Status:** confirmed

## Feeds and archives

[API and WebSocket sessions](#hl-38) · [Assets and metadata](#hl-53) · [Live order books](#hl-66) · [Public trades](#hl-80) · [Asset contexts and timestamps](#hl-91) · [L2 archive](#hl-96) · [Node fills and block archives](#hl-105)

### HL-38

The HTTP API uses `https://api.hyperliquid.xyz` on mainnet and
`https://api.hyperliquid-testnet.xyz` on testnet.

- **Kind:** interface
- **Source:** [API networks](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-39

Read requests use HTTP POST `/info`, and exchange actions use HTTP POST `/exchange`.

- **Kind:** interface
- **Source:** [Info endpoint](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint), [Exchange endpoint](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/exchange-endpoint)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-40

The WebSocket endpoints are `wss://api.hyperliquid.xyz/ws` on mainnet and
`wss://api.hyperliquid-testnet.xyz/ws` on testnet.

- **Kind:** interface
- **Source:** [Websocket](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-41

The public market-data subscriptions used by the recorder can be opened without an
account or a signing key.

- **Kind:** interface
- **Source:** [Websocket connection example](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket), [Subscriptions](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket/subscriptions)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-42

WebSocket `post` transports the HTTP API's info requests and signed actions in an
envelope with `method`, `id`, and `request` containing `type` and `payload`.

- **Kind:** interface
- **Source:** [WebSocket post requests](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket/post-requests)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-43

A WebSocket heartbeat sends `{"method":"ping"}` and receives `{"channel":"pong"}`.

- **Kind:** interface
- **Source:** [Timeouts and heartbeats](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket/timeouts-and-heartbeats)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-44

The server closes a WebSocket connection after 60 seconds without sending a message
to that connection.

- **Kind:** interface
- **Source:** [Timeouts and heartbeats](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket/timeouts-and-heartbeats)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-149

The WebSocket documentation says the API servers may disconnect clients
periodically without notice and advises automated clients to reconnect.
It specifies no interval, close code or reason for these disconnections.

- **Kind:** interface
- **Source:** [WebSocket](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket)
- **Checked:** 2026-11-27
- **Status:** confirmed

### HL-45

A successful `{"method":"subscribe","subscription":...}` request is acknowledged on
`subscriptionResponse`, whose `data` contains the original subscription.

- **Kind:** interface
- **Source:** [Subscriptions](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket/subscriptions)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-46

The recorded control-frame format includes an `error` channel whose `data` is an
error string, including refused-subscription errors.

- **Kind:** interface
- **Source:** [Recorded control-frame tests](../../crates/hl-codec/src/hyperliquid/ws/tests.rs)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-47

The subscription coin key is `c` for `l2` and `coin` for `l2Book`, `bbo`, `trades`
and `activeAssetCtx`.

- **Kind:** interface
- **Source:** [Subscriptions](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket/subscriptions)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-48

The modify request supplies a replacement order with its own price and size,
identifying the existing order by `oid` or `cloid`.

- **Kind:** interface
- **Source:** [Modify an order](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/exchange-endpoint#modify-an-order)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-49

Hyperliquid’s signing guide instructs callers to remove trailing zeros from prices
and sizes.

- **Kind:** interface
- **Source:** [Tick and lot size](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/tick-and-lot-size#signing)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-50

The venue returns its assigned order ID, `oid`, on successful placement, including
immediately filled and resting orders.

- **Kind:** interface
- **Source:** [Place an order](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/exchange-endpoint#place-an-order)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-51

The optional client order ID, `cloid`, is a 128-bit value encoded as `0x` followed
by 32 hexadecimal digits.

- **Kind:** interface
- **Source:** [Place an order](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/exchange-endpoint#place-an-order), [Order status](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint#query-order-status-by-oid-or-cloid)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-52

Account addresses in the API are 20 bytes represented as `0x` followed by 40
hexadecimal digits.

- **Kind:** interface
- **Source:** [Info endpoint address format](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint#query-user-rate-limits), [Recorded address decoder tests](../../crates/hl-codec/src/hyperliquid/ws/tests.rs)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-53

Spot asset IDs differ between mainnet and testnet. For a main-dex perpetual,
the asset ID is the coin's index in the `meta` response. Builder-deployed
perpetuals use that index with the dex-specific offset in [HL-54](#hl-54).

- **Kind:** interface
- **Source:** [Asset IDs](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/asset-ids)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-54

A HIP-3 perpetual's numeric asset ID is `100000 + perp_dex_index * 10000 +
index_in_meta`.

- **Kind:** interface
- **Source:** [Asset IDs](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/asset-ids#perps)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-55

A main-dex perpetual uses its metadata coin name, such as `BTC`, while a
builder-deployed HIP-3 perpetual uses `{dex}:{coin}`, such as `xyz:NVDA`.

- **Kind:** interface
- **Source:** [Info endpoint coin names](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint), [Asset IDs](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/asset-ids)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-56

Spot subscription coins use `PURR/USDC` for PURR and `@<pair index>` for other spot
pairs.

- **Kind:** interface
- **Source:** [Info endpoint spot coin names](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-57

Outcome spot coins use `#<encoding>`, and the corresponding outcome token names use
`+<encoding>`.

- **Kind:** interface
- **Source:** [Asset IDs outcome representations](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/asset-ids)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-58

The `metaAndAssetCtxs` info request returns perpetual metadata and per-asset
contexts, including daily notional volume, with an optional `dex` name selecting the
perpetual dex.

- **Kind:** interface
- **Source:** [Perpetual asset contexts](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint/perpetuals#retrieve-perpetuals-asset-contexts-includes-mark-price-current-funding-open-interest-etc)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-59

The `perpDexs` info request returns the list of perpetual dexes.

- **Kind:** interface
- **Source:** [Perpetual dex list](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint/perpetuals#retrieve-all-perpetual-dexs)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-60

HIP-3 metadata can include a `growthMode` field for an instrument.

- **Kind:** interface
- **Source:** [HIP-3 metadata response](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint/perpetuals#retrieve-perpetuals-asset-contexts-includes-mark-price-current-funding-open-interest-etc)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-61

The probe's HIP-3 metadata response includes `deployerFeeScale`.

- **Kind:** interface
- **Source:** [Perpetual metadata documentation](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint/perpetuals), [Existing probe description](../../scripts/probes/README.md#live-market-probes-public-api-no-account)
- **Checked:** 2026-11-25
- **Status:** unverified. The opened perpetual metadata examples omit this field, and the probe description retains no response fixture establishing it.

### HL-62

The `userFees` info request accepts a user address and returns that user's fee
schedule and fee-related information.

- **Kind:** interface
- **Source:** [User fee query](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint#query-a-users-fees)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-63

The `userRateLimit` info request accepts a user address and returns cumulative
volume and request-allowance counters.

- **Kind:** interface
- **Source:** [User rate-limit query](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint#query-user-rate-limits)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-64

All subscriptions in the 2026-11-24 recorder check were acknowledged and
produced data across both main-dex and HIP-3 markets.

- **Kind:** interface
- **Source:** [Recorded subscription check](../measurements/2026-11-24-recorder-feeds.md#hyperliquid-subscriptions)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-65

The three HIP-3 markets in the checked configuration operated in growth mode
on the sample date, 2026-11-24.

- **Kind:** interface
- **Source:** [Perpetual metadata examples](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint/perpetuals)
- **Checked:** 2026-11-25
- **Status:** unverified. The public metadata example shows growth mode for NVDA and XYZ100 but does not establish it for all three markets on the sample date.

### HL-66

An `l2` subscription starts with one visible-book snapshot of up to 20 levels per
side, then supplies incremental diffs.

- **Kind:** interface
- **Source:** [Subscriptions](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket/subscriptions)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-67

The `l2` payload places a snapshot under `s`, a plain JSON diff under `u`, or a
compressed diff under `c`.

- **Kind:** interface
- **Source:** [Subscriptions data types](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket/subscriptions)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-68

Compressed `l2` diffs are base64-encoded raw DEFLATE containing UTF-8 JSON.

- **Kind:** interface
- **Source:** [Subscriptions compression format](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket/subscriptions)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-70

An `l2` diff uses `c` for coin, `t` for time, `l` for bid/ask level updates with
string `p` and `s`, and `r` for bid/ask removal-index arrays.

- **Kind:** interface
- **Source:** [Subscriptions data types](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket/subscriptions)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-71

All `l2` removal indexes refer to the book before that diff, with index zero at the
best level, and the indexes need not arrive sorted.

- **Kind:** interface
- **Source:** [Subscriptions diff application](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket/subscriptions), [Recorded reconstruction evidence](../rfc/0003-hyperliquid-market-data-decoding.md#d3-l2diff-is-a-market-event)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-72

After positional removals, an `l2` level update replaces the size at its price or
inserts a new price level.

- **Kind:** interface
- **Source:** [Subscriptions diff application](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket/subscriptions)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-73

In the recorded `l2` interface, deletions use removal indexes and a zero-size level
update does not encode a deletion.

- **Kind:** interface
- **Source:** [Recorded diff semantics and verification](../rfc/0003-hyperliquid-market-data-decoding.md#d3-l2diff-is-a-market-event), [Subscriptions diff application](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket/subscriptions)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-74

Incremental `l2` level updates omit the number of resting orders.

- **Kind:** interface
- **Source:** [Subscriptions](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket/subscriptions)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-75

Reconciliation of the checked WebSocket and archive samples identifies a book
stamped `T` as the state after block `T`. The subscription snapshot already includes
diffs with block time at or before `T`.

- **Kind:** interface
- **Source:** [Snapshot reconciliation](../rfc/0003-hyperliquid-market-data-decoding.md#d3-l2diff-is-a-market-event), [Archive book reconciliation](../rfc/0003-hyperliquid-market-data-decoding.md#d6-the-l2-archive-is-decoded-line-by-line-strictly-by-version), [Archive reconciliation test](../../crates/hl-book/tests/archive_samples.rs)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-76

Each `l2Book` message is a full snapshot of the visible levels, encoded as `coin`,
`time`, and `levels`, with each level carrying string `px`, string `sz`, and numeric
`n` for order count.

- **Kind:** interface
- **Source:** [Subscriptions book types](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket/subscriptions)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-77

Book level arrays put bids first in descending price order and asks second in
ascending price order.

- **Kind:** interface
- **Source:** [Recorded book decoder tests](../../crates/hl-codec/src/hyperliquid/ws/tests.rs), [Archive decoder tests](../../crates/hl-codec/src/hyperliquid/archive/tests.rs), [L2 book example](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint#l2-book-snapshot)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-78

A `bbo` message supplies the current best bid and best ask in a two-element array,
with `null` for an absent side.

- **Kind:** interface
- **Source:** [Subscriptions BBO type](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket/subscriptions)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-79

The `bbo` channel emits an update only when the best bid or best ask changes in a
block.

- **Kind:** interface
- **Source:** [Subscriptions, BBO stream](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket/subscriptions)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-80

In the recorded PONS `trades` excerpt, all seven frames listed trades from oldest to
newest by block time.

- **Kind:** interface
- **Source:** [Recorded trade decoder fixture](../../crates/hl-codec/src/hyperliquid/ws/tests.rs), [PONS recording excerpt](../../crates/hl-codec/tests/fixtures/hl_ws_pons_20261124.hlrec)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-81

The captured `trades` subscriptions replayed recent trades in the initial batch,
including trades from before the connection was opened.

- **Kind:** interface
- **Source:** [Recorded trade/fill reconciliation](../rfc/0005-data-quality-checks.md#verification), [Feed-timing method](../../research/reports/feed-timing-2026-11.md#method-and-coverage)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-82

One public `trades` frame can contain trades from several block times. The initial
PONS frame grouped 30 trades across 16 timestamps.

- **Kind:** interface
- **Source:** [Recorded trade decoder fixture](../../crates/hl-codec/src/hyperliquid/ws/tests.rs), [Feed-timing method](../../research/reports/feed-timing-2026-11.md#method-and-coverage), [PONS recording excerpt](../../crates/hl-codec/tests/fixtures/hl_ws_pons_20261124.hlrec)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-83

The public trade `side` is the aggressor side, with `B` meaning buy and `A` meaning
sell.

- **Kind:** interface
- **Source:** [API notation](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/notation), [Recorded trade/fill reconciliation](../rfc/0005-data-quality-checks.md#verification)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-84

The public trade `users` array contains the buyer first and the seller second.

- **Kind:** interface
- **Source:** [Subscriptions trade type](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket/subscriptions)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-85

The maker and taker records for the same trade share `tid`.

- **Kind:** interface
- **Source:** [Node-fill decoder test](../../crates/hl-codec/src/hyperliquid/archive/tests.rs#L246-L277), [Trade type](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket/subscriptions#data-type-definitions)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-86

A trade's `tid` alone is not globally unique. Hyperliquid specifies `(block_time,
coin, tid)` as its globally unique trade identifier.

- **Kind:** interface
- **Source:** [Trade type](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket/subscriptions#data-type-definitions)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-87

The public trade `tid` equals the corresponding archive fill `tid` in the
repository's recorded trade/fill reconciliation.

- **Kind:** interface
- **Source:** [Recorded reconciliation](../rfc/0005-data-quality-checks.md#verification)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-88

A public trade or a node fill can carry an all-zero transaction hash. Hyperliquid
documents a zero hash for TWAP fills.

- **Kind:** interface
- **Source:** [Recorded trade decoder tests](../../crates/hl-codec/src/hyperliquid/ws/tests.rs), [Archive optional-field tests](../../crates/hl-codec/src/hyperliquid/archive/tests.rs), [TWAP slice fills](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint#retrieve-a-users-twap-slice-fills)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-89

An all-zero trade or fill hash identifies a trade generated internally by the venue.

- **Kind:** interface
- **Source:** [Subscriptions transaction fields](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket/subscriptions), [Archive optional-field tests](../../crates/hl-codec/src/hyperliquid/archive/tests.rs)
- **Checked:** 2026-11-25
- **Status:** unverified. The documentation does not define zero-hash provenance, and the captured TWAP example establishes occurrence without proving the general attribution.

### HL-90

The documented public trade record contains coin, side, price, size, time, hash,
trade id and both users, and omits order ids and fees.

- **Kind:** interface
- **Source:** [Subscriptions trade type](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket/subscriptions)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-91

The `activeAssetCtx` channel carries a coin and its asset context, whose perpetual
fields include funding, open interest, oracle price, mark price and daily notional
volume.

- **Kind:** interface
- **Source:** [Subscriptions context types](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket/subscriptions)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-92

The `fastAssetCtxs` subscription has no coin parameter and supplies a base64-encoded
raw-DEFLATE JSON map from coin names to mark/mid-price updates.

- **Kind:** interface
- **Source:** [Subscriptions fast contexts](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket/subscriptions)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-93

A recorded perpetual asset context can carry ten decimal places in funding, premium
or daily notional volume. RFC 0003 records the funding value `-0.0000027259`, and
the archived PONS recording includes ten-place premium and volume fields.

- **Kind:** interface
- **Source:** [Context precision observation](../rfc/0003-hyperliquid-market-data-decoding.md#verification), [Recorded context decoder fixture](../../crates/hl-codec/src/hyperliquid/ws/tests.rs), [PONS recording excerpt](../../crates/hl-codec/tests/fixtures/hl_ws_pons_20261124.hlrec)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-94

Hyperliquid adds JSON fields to its feeds over time.

- **Kind:** interface
- **Source:** [Current subscriptions schema](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket/subscriptions)
- **Checked:** 2026-11-25
- **Status:** unverified. The current schema describes fields and a planned removal of undocumented WebData3 fields, but the opened page provides no version history establishing additions to the recorded feeds.

### HL-95

The WebSocket `l2` snapshot, `l2Book`, `bbo` and public trades carry Unix block time
in milliseconds in `time`, as do `l2` diffs in `t`. The L2 archive preserves the
book’s milliseconds in `raw.data.time`. Each archived fill row’s `time` is in
milliseconds, while its `node_fills_by_block` envelope’s ISO `block_time` carries up
to nine fractional digits, including nonzero submillisecond digits. The L2
envelope’s outer `time` and the fills envelope’s `local_time` are separate node
clocks.

- **Kind:** interface
- **Source:** [Recorded WebSocket decoder tests](../../crates/hl-codec/src/hyperliquid/ws/tests.rs), [Archive precision tests](../../crates/hl-codec/src/hyperliquid/archive/tests.rs), [Shared WebSocket book schema](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket/subscriptions)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-96

Both official historical-data buckets require requester-pays access.

- **Kind:** interface
- **Source:** [Historical data requester-pays examples](https://hyperliquid.gitbook.io/hyperliquid-docs/historical-data), [Archive probe prerequisites](../../scripts/probes/README.md#requirements)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-97

The official L2 archive is stored under
`hyperliquid-archive/market_data/<date>/<hour>/l2Book/<coin>.lz4`.

- **Kind:** interface
- **Source:** [Historical data](https://hyperliquid.gitbook.io/hyperliquid-docs/historical-data)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-98

Each decompressed L2 archive line in the checked sample is one JSON object with
outer `time`, `ver_num` equal to 1, and a `raw` envelope whose channel is `l2Book`.

- **Kind:** interface
- **Source:** [Archive decoder tests](../../crates/hl-codec/src/hyperliquid/archive/tests.rs), [Checked L2 fixture](../../crates/hl-codec/tests/fixtures/archive_l2_jup_20260601.jsonl)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-99

The L2 archive’s outer `time` records the archiving node’s write clock. The node
writes in batches, and in the sample the write time lagged the book time by up to
five seconds.

- **Kind:** interface
- **Source:** [Archive node-clock test](../../crates/hl-codec/src/hyperliquid/archive/tests.rs), [Archive observations](../rfc/0003-hyperliquid-market-data-decoding.md#d6-the-l2-archive-is-decoded-line-by-line-strictly-by-version)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-100

The checked official L2 archive sample contains 20 visible levels on each side.

- **Kind:** interface
- **Source:** [Archive decoder test](../../crates/hl-codec/src/hyperliquid/archive/tests.rs), [Whole-hour archive verification](../rfc/0003-hyperliquid-market-data-decoding.md#verification)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-101

The official L2 archive contains no HIP-3 books.

- **Kind:** interface
- **Source:** [Historical data](https://hyperliquid.gitbook.io/hyperliquid-docs/historical-data), [Archive probe finding](../../scripts/probes/README.md#0xarchive-sample-day-checks)
- **Checked:** 2026-11-25
- **Status:** unverified. The public historical-data page does not enumerate HIP-3 coverage, and the repository records this claim without an inventory that can be checked here.

### HL-102

The documented `hyperliquid-archive` market dataset supplies L2 snapshots and asset
contexts, with no historical incremental `l2` stream or `bbo` stream listed.

- **Kind:** interface
- **Source:** [Historical data](https://hyperliquid.gitbook.io/hyperliquid-docs/historical-data)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-103

The L2 archive covers the range 2025-07-27 through 2026-06-12 used by RFC 0004's
storage calculation.

- **Kind:** interface
- **Source:** [Historical data](https://hyperliquid.gitbook.io/hyperliquid-docs/historical-data), [Existing archive capacity calculation](../rfc/0004-clickhouse-schema-and-loading.md#measured-on-the-sample-hour)
- **Checked:** 2026-11-25
- **Status:** unverified. The opened documentation supplies no date inventory, and the retained sample proves one hour on 2026-06-01 rather than the asserted range.

### HL-104

The official L2 archive became ten times sparser on 2026-06-13.

- **Kind:** interface
- **Source:** [Historical data](https://hyperliquid.gitbook.io/hyperliquid-docs/historical-data), [Existing archive capacity calculation](../rfc/0004-clickhouse-schema-and-loading.md#measured-on-the-sample-hour)
- **Checked:** 2026-11-25
- **Status:** unverified. The opened documentation gives no thinning date or factor, and the repository contains no before-and-after sample establishing this change.

### HL-105

The repository’s historical fills come from
`hl-mainnet-node-data/node_fills_by_block/hourly/<date>/<hour>.lz4`.

- **Kind:** interface
- **Source:** [Historical data](https://hyperliquid.gitbook.io/hyperliquid-docs/historical-data), [Recorded archive path](../../crates/hl-codec/src/hyperliquid/archive.rs#L5)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-106

Each decompressed line in the hourly node-fills archive contains the fills for one
block.

- **Kind:** interface
- **Source:** [Archive probe paths](../../scripts/probes/README.md#official-archive-probes), [Archive decoder tests](../../crates/hl-codec/src/hyperliquid/archive/tests.rs)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-107

A fills block in the checked archive has `block_number`, `block_time`, `local_time`
and `events`, where each event pairs an account address with a fill record.

- **Kind:** interface
- **Source:** [Archive decoder tests](../../crates/hl-codec/src/hyperliquid/archive/tests.rs), [Checked fills fixture](../../crates/hl-codec/tests/fixtures/archive_fills_20260601.jsonl)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-108

The checked fills archive mixes main-dex perpetuals, HIP-3 perpetuals, spot and
outcome fills within the same block stream.

- **Kind:** interface
- **Source:** [Archive market-kind tests](../../crates/hl-codec/src/hyperliquid/archive/tests.rs), [Whole-hour archive verification](../rfc/0003-hyperliquid-market-data-decoding.md#verification)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-109

The checked node-fill records contain `coin`, `time`, `tid`, `oid`, `side`,
`crossed`, `px`, `sz`, `startPosition`, `dir`, `closedPnl`, `fee`, `feeToken` and
`hash`. Optional fields observed in the fixture are `cloid`, `builderFee`,
`builder`, `deployerFee`, `priorityGas`, `twapId` and `liquidation`, whose object
contains `liquidatedUser`, `markPx` and `method`.

- **Kind:** interface
- **Source:** [Archive field tests](../../crates/hl-codec/src/hyperliquid/archive/tests.rs), [Checked fills fixture](../../crates/hl-codec/tests/fixtures/archive_fills_20260601.jsonl)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-110

Each perpetual trade in the checked archive has two party rows, exactly one with
`crossed=true` for the taker and one with `crossed=false` for the maker.

- **Kind:** interface
- **Source:** [Archive two-party tests](../../crates/hl-codec/src/hyperliquid/archive/tests.rs), [Whole-hour archive verification](../rfc/0003-hyperliquid-market-data-decoding.md#verification), [Subscriptions fill crossed flag](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket/subscriptions)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-111

The checked archive contains `startPosition` with more than eight fractional digits,
including `-1081829.6000000001`.

- **Kind:** interface
- **Source:** [Start-position fixture test](../../crates/hl-codec/src/hyperliquid/archive/tests.rs), [Checked fills fixture](../../crates/hl-codec/tests/fixtures/archive_fills_20260601.jsonl)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-112

The venue serializes archive `startPosition` from a floating-point value.

- **Kind:** interface
- **Source:** [Subscriptions fill fields](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket/subscriptions), [Start-position fixture test](../../crates/hl-codec/src/hyperliquid/archive/tests.rs)
- **Checked:** 2026-11-25
- **Status:** unverified. The captured decimal tail is consistent with this explanation, but the opened documentation specifies a string without identifying the internal number representation.

### HL-113

The fill `dir` field is a string used for display and has no documented closed
enumeration.

- **Kind:** interface
- **Source:** [Subscriptions fill type](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket/subscriptions)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-114

Hourly node-fill files are partitioned by the node's clock, so their block-time span
can cross the UTC hour boundary in the path. In the repository's 2026-11-24 check,
file `6` began at block time 05:59:59.864.

- **Kind:** interface
- **Source:** [Archive hour-boundary observation](../rfc/0005-data-quality-checks.md#verification)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-115

The fill's `builderFee` is the amount paid to a builder and is already included in
`fee`.

- **Kind:** interface
- **Source:** [Fill type](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/websocket/subscriptions#data-type-definitions)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-116

The archived `deployerFee` records the HIP-3 deployer's fee share and can be
negative.

- **Kind:** interface
- **Source:** [Node-fill representation](../../crates/hl-codec/src/hyperliquid/archive.rs#L111), [Recorded-fill decoder assertion](../../crates/hl-codec/src/hyperliquid/archive/tests.rs#L330-L333)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-117

Spot fill fees in the repository's 2026-06-01 archive check reached ten decimal
places, including token-denominated fees such as UBTC.

- **Kind:** interface
- **Source:** [Archive precision observation and sample method](../rfc/0003-hyperliquid-market-data-decoding.md#verification)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-118

The official `hl-mainnet-node-data/explorer_blocks` prefix contains historical
explorer blocks.

- **Kind:** interface
- **Source:** [Historical data](https://hyperliquid.gitbook.io/hyperliquid-docs/historical-data)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-119

Hyperliquid's node `raw_book_diff` files carry `insertBefore` on `RawBookDiff::New`,
and its order-book server uses that field to maintain queue ordering.

- **Kind:** interface
- **Source:** [ALO priority implementation](https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/priority-fees#alo-priority)
- **Checked:** 2026-11-25
- **Status:** confirmed

### HL-120

The official L2 and node-fill archive records supply node clocks and exchange clocks
without the strategy process's receive time.

- **Kind:** interface
- **Source:** [Archive schemas](../../crates/hl-codec/src/hyperliquid/archive/tests.rs), [Archive timestamp interpretation](../rfc/0003-hyperliquid-market-data-decoding.md#d6-the-l2-archive-is-decoded-line-by-line-strictly-by-version)
- **Checked:** 2026-11-25
- **Status:** confirmed

## Measured by us

[Inflated diff sizes](#hl-69) · [Block cadence and delivery](#hl-121) · [Receive latency and activity](#hl-127) · [Two-site latency probe](#hl-132) · [Book and fill reconciliation](#hl-137) · [WebSocket connection lifetimes](#hl-150)

### HL-69

The 104 compressed `l2` diffs in the PONS recording excerpt inflated to
144–1,095 bytes each, with a median of 324.5 bytes.

- **Kind:** measured
- **Source:** For each received `l2` frame with `data.c`, base64-decode that field, inflate the raw DEFLATE stream and count the output bytes. [PONS recording excerpt](../../crates/hl-codec/tests/fixtures/hl_ws_pons_20261124.hlrec), [fixture provenance](../../crates/hl-codec/tests/recorded_frames.rs#L1)
- **Checked:** 2026-11-24, one PONS recording excerpt, all 104 compressed diffs
- **Status:** confirmed

### HL-121

The most common interval between visible live blocks was 67 ms. The median was 69 ms
across 80,819 intervals after excluding subscription history.

- **Kind:** measured
- **Source:** [Tokyo feed-timing report](../../research/reports/feed-timing-2026-11.md#block-spacing)
- **Checked:** 2026-11-24, Tokyo, 1.846 observed hours across two continuous recording segments
- **Status:** confirmed

### HL-122

An estimated 18.38% of block steps had no update for the recorded instruments. The
estimate counts 18,199 missing steps by rounding the observed gaps to multiples of
the modal cadence.

- **Kind:** measured
- **Source:** [Tokyo feed-timing report](../../research/reports/feed-timing-2026-11.md#block-spacing)
- **Checked:** 2026-11-24, Tokyo, 1.846 observed hours across two continuous recording segments
- **Status:** confirmed

### HL-123

Within a block, the usual delivery order was `bbo`, then `l2`, then `trades`. All
`bbo` frames preceded all `l2` frames in 10,469 of 10,506 shared blocks, `bbo`
preceded `trades` in 24,608 of 24,608, and `l2` preceded `trades` in 3,419 of 3,421.

- **Kind:** measured
- **Source:** [Tokyo feed-timing report](../../research/reports/feed-timing-2026-11.md#delivery-order-and-socket-reads)
- **Checked:** 2026-11-24, Tokyo, 1.846 observed hours across two continuous recording segments
- **Status:** confirmed

### HL-124

Frames for 348 of 81,179 block groups (0.43%) were interleaved with frames from
another block.

- **Kind:** measured
- **Source:** [Tokyo feed-timing report](../../research/reports/feed-timing-2026-11.md#delivery-order-and-socket-reads)
- **Checked:** 2026-11-24, Tokyo, 1.846 observed hours across two continuous recording segments
- **Status:** confirmed

### HL-125

Of 564,933 market frames, 1,875 (0.33%) contained a block time older than the latest
one already received in the segment. These included 1,803 `trades` frames and 72
`l2` frames.

- **Kind:** measured
- **Source:** [Tokyo feed-timing report](../../research/reports/feed-timing-2026-11.md#delivery-order-and-socket-reads)
- **Checked:** 2026-11-24, Tokyo, 1.846 observed hours across two continuous recording segments
- **Status:** confirmed

### HL-126

Of 564,931 consecutive market-frame pairs, none shared an exact receive timestamp
and 482,731 (85.45%) fell within the same millisecond.

- **Kind:** measured
- **Source:** [Tokyo feed-timing report](../../research/reports/feed-timing-2026-11.md#delivery-order-and-socket-reads)
- **Checked:** 2026-11-24, Tokyo, 1.846 observed hours across two continuous recording segments
- **Status:** confirmed

### HL-127

The pooled activity quintiles had upper boundaries of 69, 81, 92 and 105 frames.
Activity counted physical `bbo`, `l2` and `trades` frames in the preceding second,
excluding frames tied at the current receive time. The five ranges were 0–69, 70–81,
82–92, 93–105 and 106 or more.

- **Kind:** measured
- **Source:** [Tokyo feed-timing report](../../research/reports/feed-timing-2026-11.md#latency)
- **Checked:** 2026-11-24, Tokyo, 1.846 observed hours across two continuous recording segments
- **Status:** confirmed

### HL-128

For `bbo`, receive wall time minus block time had a median of 229.625 ms and a p99
of 456.329 ms. From the lowest to the highest activity quintile, p99 was 494.246,
466.420, 451.721, 407.997 and 385.477 ms.

- **Kind:** measured
- **Source:** [Tokyo feed-timing report](../../research/reports/feed-timing-2026-11.md#latency)
- **Checked:** 2026-11-24, Tokyo, 1.846 observed hours across two continuous recording segments
- **Status:** confirmed

### HL-129

For `l2`, receive wall time minus block time had a median of 261.301 ms and a p99 of
462.932 ms. Across increasing activity quintiles, p99 was 479.352, 470.108, 456.768,
440.995 and 400.676 ms.

- **Kind:** measured
- **Source:** [Tokyo feed-timing report](../../research/reports/feed-timing-2026-11.md#latency)
- **Checked:** 2026-11-24, Tokyo, 1.846 observed hours across two continuous recording segments
- **Status:** confirmed

### HL-130

For live `trades`, receive wall time minus block time had a median of 248.989 ms and
a p99 of 473.335 ms. Across increasing activity quintiles, p99 was 514.846, 498.430,
472.915, 448.980 and 416.360 ms. The report counts each distinct block time in a
frame and excludes subscription-history batches from these values.

- **Kind:** measured
- **Source:** [Tokyo feed-timing report](../../research/reports/feed-timing-2026-11.md#latency)
- **Checked:** 2026-11-24, Tokyo, 1.846 observed hours across two continuous recording segments
- **Status:** confirmed

### HL-131

In each of the two UTC hours sampled, the quietest activity quintile had a higher
receive-latency p99 than the busiest quintile on each of the three live channels.

- **Kind:** measured
- **Source:** [Tokyo feed-timing report](../../research/reports/feed-timing-2026-11.md#latency)
- **Checked:** 2026-11-24, Tokyo, 1.846 observed hours across two continuous recording segments
- **Status:** confirmed

### HL-132

Small HTTPS `/info` requests had a median round trip of 18 ms from Tokyo and 92 ms
from a second site on consumer broadband.

- **Kind:** measured
- **Source:** [Two-site latency probe](../../scripts/probes/README.md#latency-probe)
- **Checked:** 2026-11-24, Tokyo and the second site, simultaneous probes around 02:10 UTC, 150 seconds of market data
- **Status:** confirmed

### HL-133

For HTTPS `/exchange` requests rejected at the API, the minimum and median round
trips were 5 and 17 ms from Tokyo, and 77 and 89 ms from a second site on consumer
broadband. The probe timed through the rejection response.

- **Kind:** measured
- **Source:** [Two-site latency probe](../../scripts/probes/README.md#latency-probe)
- **Checked:** 2026-11-24, Tokyo and the second site, simultaneous probes around 02:10 UTC, 150 seconds of market data
- **Status:** confirmed

### HL-134

The simultaneous probe measured `bbo` receive lag (minimum / median / p99) of 178 /
254 / 522 ms from Tokyo and 222 / 307 / 534 ms from a second site on consumer broadband.

- **Kind:** measured
- **Source:** [Two-site latency probe](../../scripts/probes/README.md#latency-probe)
- **Checked:** 2026-11-24, Tokyo and the second site, simultaneous probes around 02:10 UTC, 150 seconds of market data
- **Status:** confirmed

### HL-135

The simultaneous probe measured median `trades` receive lag of 273 ms from Tokyo and
326 ms from a second site on consumer broadband.

- **Kind:** measured
- **Source:** [Two-site latency probe](../../scripts/probes/README.md#latency-probe)
- **Checked:** 2026-11-24, Tokyo and the second site, simultaneous probes around 02:10 UTC, 150 seconds of market data
- **Status:** confirmed

### HL-136

The reported median time from sending a market order in Tokyo to receiving its fill
was 0.88 s. RFC 0006 lists it as a latency prior.

- **Kind:** measured
- **Source:** No order-timing sample or measurement method is available.
- **Checked:** Tokyo, sample date, duration and order count unreported (checked 2026-11-25 UTC)
- **Status:** unverified. There are no individual order timings from which to reproduce the median.

### HL-137

In two recorder runs, all 1,520 block times with both a rebuilt `l2` update and a
`bbo` message agreed on best bid and ask prices and sizes. The separate recording in
RFC 0004 agreed at all 889 shared block times.

- **Kind:** measured
- **Source:** [Two full recordings](../rfc/0003-hyperliquid-market-data-decoding.md#verification), [loaded recording](../rfc/0004-clickhouse-schema-and-loading.md#verification)
- **Checked:** 2026-11-24, 11 instruments, 75-second and 60-second runs in RFC 0003 and a 90-second recording in RFC 0005
- **Status:** confirmed

### HL-138

The June archive sample contained 6,674 BTC books and 6,674 JUP books in one hour,
approximately one snapshot per coin every 0.54 s.

- **Kind:** measured
- **Source:** [Archive sample report](../rfc/0003-hyperliquid-market-data-decoding.md#verification)
- **Checked:** 2026-06-01, 09:00–10:00 UTC, BTC and JUP L2 snapshots and the corresponding fills hour
- **Status:** confirmed

### HL-139

Of 11,108 BTC taker fills, 401 (3.6%) fell on the wrong side of the preceding
archived book, and none contradicted both neighbouring books. BTC traded at a
one-tick spread in the comparison. For JUP, none of the 283 fills contradicted the
preceding book. The comparison counts a buy at or below the bid or a sell at or
above the ask.

- **Kind:** measured
- **Source:** [Archive sample report](../rfc/0003-hyperliquid-market-data-decoding.md#verification) and [daily check sample](../rfc/0005-data-quality-checks.md#d3-a-fill-contradicts-the-books-only-if-it-contradicts-both-neighbours)
- **Checked:** 2026-06-01, 09:00–10:00 UTC, BTC and JUP L2 snapshots and the corresponding fills hour
- **Status:** confirmed

### HL-140

In the RFC 0003 sample, fills sharing a block time with an archived book reportedly
contradicted that book four times as often as they contradicted the preceding book.

- **Kind:** measured
- **Source:** [RFC 0003, archive book timing](../rfc/0003-hyperliquid-market-data-decoding.md#d6-the-l2-archive-is-decoded-line-by-line-strictly-by-version) and [archive comparison method](../../crates/hl-book/tests/archive_samples.rs#L151)
- **Checked:** 2026-06-01, 09:00–10:00 UTC, BTC and JUP L2 snapshots and the corresponding fills hour
- **Status:** unverified. The write-up gives the ratio, but the checked-in comparison pools all taker fills and reports no counts for the equal-time subset.

### HL-142

RFC 0004 reports 302 distinct perpetuals in one hour of archived fills.

- **Kind:** measured
- **Source:** No input file is identified for the sampled hour.
- **Checked:** One hour of archived fills, date and hour unknown. Checked 2026-11-25 UTC.
- **Status:** unverified. RFC 0004 gives neither a list of perpetuals nor saved counting output for this count.

### HL-147

The 11-instrument `hl` source produced 0.8 to 1.2 MB per minute at zstd
level 3. The recorder report extrapolated 1.1 to 1.7 GB per day from
those runs. It measured four channels per coin and one `fastAssetCtxs`
subscription for the source.

- **Kind:** measured
- **Source:** [Recorder feed volumes](../measurements/2026-11-24-recorder-feeds.md#results)
- **Checked:** 2026-11-24, one-minute runs from a workstation outside the recording region around 03:05 UTC
- **Status:** confirmed

### HL-148

Fourteen main-dex perpetuals appeared in the top 30 by approximate notional
in all 28 weekly rankings. Together they accounted for 92.3% of approximate
main-dex notional over the 196 days used to rank the markets, including
markets later delisted.

- **Kind:** measured
- **Source:** [Universe churn measurement](../measurements/2026-11-26-universe-churn.md)
- **Checked:** 2026-05-11 to 2026-11-22 UTC, 196 days of candles and 28 weekly rankings effective 2026-05-18 to 2026-11-23
- **Status:** confirmed

### HL-150

The public WebSocket closed 24 of the Tokyo recorder's 35 connections with
code `1000` and reason `Expired`. A workstation outside the recording region
recorded no such closes in its first paper run (two connections) and three in
its second (five connections). Of the 27 connections that ended `Expired`, 23 lasted
between 2 h 46 min 53 s and 3 h 02 min 19 s. The other four, all in Tokyo,
lasted between 7 min 57 s and 23 min 15 s.

- **Kind:** measured
- **Source:** [WebSocket connection lifetime measurement](../measurements/2026-11-24-websocket-connection-lifetimes.md)
- **Checked:** Tokyo, 75 files from 2026-11-24 07:08 UTC to the 2026-11-27 07:00 UTC file-open cutoff. Workstation outside the recording region, 4 and 15 files from the two runs on 2026-11-26–27 UTC.
- **Status:** confirmed
