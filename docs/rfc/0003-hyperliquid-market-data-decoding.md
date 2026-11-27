# RFC 0003: Decoding Hyperliquid market data

- Status: WebSocket book and trade channels and both official archive
  formats implemented; context channels and Binance pending; owner review
  pending
- Date: 2026-11-24
- Crates: `hl-codec` (decoding), `hl-types` (additions), `hl-book` (L2 book)

Raw recordings and official archives hold venue JSON. This RFC fixes how that
JSON becomes the events of RFC 0001, so that backfill and live data produce
identical events from identical payloads.

## Decisions

### D1. Two layers: venue records, then events

**Changed by RFC 0009 D3.** The second layer originally obtained ids from
D2's registry. It now accepts a caller-supplied id resolver backed by
`InstrumentId::derive`. The proposed `hl-run` conversion calls this layer.
[RFC 0009](0009-instrument-identity.md)

`hl-codec` first decodes a payload into a **venue record**: a typed, faithful
picture of one message (`WsBook`, `WsBookDiff`, `WsBbo`, `WsTrade`). Prices
and sizes are parsed exactly into fixed-point; coins stay strings; nothing is
dropped that the loader will want in a table (trade hash, buyer and seller).
[HL-90](../venues/hyperliquid.md#hl-90)

A second step turns records into `hl-types::MarketEvent`s using the
resolver and a caller-supplied local timestamp. `hl-run` owns coin
selection, the trade-history cutoff, gaps and counters. It passes the
record's raw receipt time and applies the RFC 0008 D4 delivery-time
floors to admitted events.
[RFC 0009 D3](0009-instrument-identity.md) ·
[HL-81](../venues/hyperliquid.md#hl-81) ·
[RFC 0008 D4](0008-paper-trading.md#d4-close-a-timestamp-before-firing-its-deadlines)

Archive records do not supply the strategy's receive time.
[HL-120](../venues/hyperliquid.md#hl-120)

Keeping the steps apart means the loader can write full rows from records
while replay and live trading use events, and both come from one parser.

### D2. Derive ids after caller selection

**Changed by RFC 0009 D1–D2.** This decision previously used
`InstrumentRegistry` to map `(network, coin)` to an id, treated it as the
in-memory form of `hl.instruments`, and made an absent coin an error.
[RFC 0009](0009-instrument-identity.md)

**Our choice.** Obtain every id with `InstrumentId::derive(venue, symbol)`.
Drivers select the coin names they requested through the id resolver.
Archive event callers select their requested coins too. The loader retains
all coins in its admitted market scope and checks id collisions as specified
by RFC 0004 D1. RFC 0009 D1 gives the policy for each current caller, and D2
removes the identity lookup types. [RFC 0009](0009-instrument-identity.md) ·
[RFC 0004 D1](0004-clickhouse-schema-and-loading.md#d1-instrument-ids-are-derived-from-the-venue-and-the-coin-name)

### D3. `L2Diff` is a market event

The `l2` channel starts with a snapshot, then sends positional removals and
price-level updates. [HL-66](../venues/hyperliquid.md#hl-66),
[HL-71](../venues/hyperliquid.md#hl-71), [HL-72](../venues/hyperliquid.md#hl-72)

`MarketEventKind::L2Diff` carries exactly that, and `hl-book::L2Book` applies
it. Recorded diffs can therefore flow through the same event path as
snapshots, and the book is the single place that knows how to apply them.

The subscription snapshot already includes its block's updates (D6).
[HL-75](../venues/hyperliquid.md#hl-75)

A book built from a snapshot at exchange time T skips every diff with block
time at or before T. The book stays in sync and reports the skip to the caller.
Later diffs apply by position. Only a new snapshot replaces the reference
time.

At the 2026-11-24 10:54 UTC subscription, MON and VVV each received a
positional removal stamped with the snapshot's block time. Applying the
removals twice shortened both books, and later removals went out of range.
Skipping those diffs kept both books in sync through the file (551 MON
diffs and 604 VVV diffs). Every comparison with a `bbo` message at the same
block time agreed. In the earlier four-file chain, about 260,000 diffs all
followed their snapshots. In a separate recording, skipping ETH's same-time
diff produced identical books throughout.
[HL-75](../venues/hyperliquid.md#hl-75),
[HL-137](../venues/hyperliquid.md#hl-137),
[recorded-book comparison method](../../crates/hl-book/tests/recorded_book.rs)

The book applies the recorded interface as follows.

- remove positions refer to the book **before** this diff, best level first,
  and may arrive unsorted. [HL-71](../venues/hyperliquid.md#hl-71)
- set levels replace a size or insert a price.
  [HL-72](../venues/hyperliquid.md#hl-72)
- deletions use positions. [HL-73](../venues/hyperliquid.md#hl-73)
- `Level::orders` is 0 for a level set by a diff, since those updates omit
  order counts. [HL-74](../venues/hyperliquid.md#hl-74)

Because removal is by position, one missed diff corrupts the book silently.
`L2Book` therefore refuses diffs until it has a snapshot, treats a `FeedGap`
as loss of sync, and drops out of sync on any inconsistency (position out of
range, crossed book) until the next snapshot.

### D4. Strict where it protects correctness, lenient where it protects uptime

- A field we decode must be present and well-formed, or the frame is an
  error. A price with more than eight decimals is an error, not a rounding.
- Unknown fields are ignored to tolerate schema changes, whose history
  remains unverified. The raw file stays available to re-parse with a newer
  codec. [HL-94](../venues/hyperliquid.md#hl-94)
- A channel we do not decode yet is returned as `WsFrame::Other`, so callers
  can count what they are not using.
- Decoding never panics on any input.

### D5. Compressed payloads are inflated inside the codec

Compressed `l2` diffs require base64 decoding and DEFLATE inflation.
[HL-68](../venues/hyperliquid.md#hl-68)

Inflating is part of decoding that channel, so `hl-codec` depends on
`miniz_oxide` and `data-encoding`. Output size is capped to bound memory on
corrupt input. File-level compression (zstd, lz4) stays with the callers.

### D6. The L2 archive is decoded line by line, strictly by version

The official L2 archive wraps each book with a node clock and `ver_num`.
[HL-97](../venues/hyperliquid.md#hl-97), [HL-98](../venues/hyperliquid.md#hl-98)

`decode_archive_l2_line` accepts version 1 only. An unknown version is an
error, so a format change stops the loader instead of being misread.

The outer `time` is the node's write clock, distinct from the book's block
time. [HL-95](../venues/hyperliquid.md#hl-95),
[HL-99](../venues/hyperliquid.md#hl-99)

It is kept as `node_time`. The archives supply no strategy receive time, so
`ts_local` is assigned later from measured latency.
[HL-120](../venues/hyperliquid.md#hl-120)

A book stamped T shows the state **after** block T.
[HL-75](../venues/hyperliquid.md#hl-75)

The sample's reported fourfold disagreement ratio for same-block fills
remains unverified. [HL-140](../venues/hyperliquid.md#hl-140)

### D7. Node fills: perpetuals in full, other markets counted

`decode_fill_block` decodes each `node_fills_by_block` line as one block,
which can mix market types. [HL-105](../venues/hyperliquid.md#hl-105),
[HL-106](../venues/hyperliquid.md#hl-106), [HL-108](../venues/hyperliquid.md#hl-108)

- Rows for perpetuals, main dex and HIP-3, become `NodeFill` records with
  every decoded field, including both parties, order ids and fees.
  [HL-108](../venues/hyperliquid.md#hl-108),
  [HL-109](../venues/hyperliquid.md#hl-109),
  [HL-115](../venues/hyperliquid.md#hl-115),
  [HL-116](../venues/hyperliquid.md#hl-116)
- Rows for spot and outcome markets are counted and skipped. They are out of
  scope, and the sampled spot-fee precision exceeds our eight-decimal types
  (see Verification).
  [HL-108](../venues/hyperliquid.md#hl-108),
  [HL-117](../venues/hyperliquid.md#hl-117)
  Markets are told apart by the coin-name forms in
  [HL-56](../venues/hyperliquid.md#hl-56) and
  [HL-57](../venues/hyperliquid.md#hl-57).
- `startPosition` can exceed eight decimals. Its suspected floating-point
  origin remains unverified. It is the one field rounded to eight decimals
  instead of being required to parse exactly.
  [HL-111](../venues/hyperliquid.md#hl-111),
  [HL-112](../venues/hyperliquid.md#hl-112)
- Fee and PnL fields use `feeToken`, which varies across the sampled
  perpetual dexes (see Verification). `priorityGas` is in HYPE.
  [HL-28](../venues/hyperliquid.md#hl-28), [HL-31](../venues/hyperliquid.md#hl-31)
- `dir` stays text to accommodate its open set of values.
  [HL-113](../venues/hyperliquid.md#hl-113)
- `trade_events` emits one `Trade` event from the taker of the paired fill
  rows, retaining the aggressor side.
  [HL-110](../venues/hyperliquid.md#hl-110), [HL-83](../venues/hyperliquid.md#hl-83)
  **Changed by RFC 0009 D1 and D3.** This step previously skipped fills
  outside the registry. The caller now selects coin names through the id
  resolver before an event is emitted. [RFC 0009](0009-instrument-identity.md) ·
  [HL-108](../venues/hyperliquid.md#hl-108)

## Not decided yet

- **Context channels** (`activeAssetCtx`, `fastAssetCtxs`). Funding, premium
  and volume fields can exceed eight-decimal precision.
  [HL-91](../venues/hyperliquid.md#hl-91), [HL-93](../venues/hyperliquid.md#hl-93)
  The observed value is retained in Verification. These channels need a
  precision decision together with the ledger and the `asset_ctx` table.
  Until then these frames decode to `WsFrame::Other`.
- **Fills outside perpetuals.** Fills for the whole market would include spot
  fees that do not fit the `Decimal64(8)` columns. Either those rows stay out
  or the column needs more scale. [HL-117](../venues/hyperliquid.md#hl-117)
- **Binance reference data.** Decoding remains later work. RFC 0009 D1 and
  D5 now decide its instrument identity and naming, replacing this RFC's
  earlier open identity question. [RFC 0009](0009-instrument-identity.md)

## Verification

- Unit tests for every decoded channel, with payloads copied from recorded
  frames, and for each error path.
- `crates/hl-codec/tests/fixtures/` holds a real recording excerpt. The codec
  test decodes every frame in it. The `hl-book` test replays it through
  codec, events and book, and checks that whenever a `bbo` message and a book
  update share a block time, the book's best bid and ask equal the `bbo`
  message. [HL-137](../venues/hyperliquid.md#hl-137)
- The same check was run once on two full recordings of 11 instruments (75 s
  and 60 s, 2026-11-24): every frame decoded, 2,684 diffs applied with the
  book in sync throughout, and all 1,520 block times that had both a book
  update and a `bbo` message agreed on price and size. Those recordings had
  no diff at its snapshot's block time. About one `bbo` update
  in seven shares a block time with a book update; the rest fall between
  book updates and cannot be compared this way.
  [HL-137](../venues/hyperliquid.md#hl-137)
- Changing the book to remove positions one at a time, instead of all
  relative to the old book, makes the fixture test fail.
- Archive decoders: unit tests run on lines taken unchanged from the archives
  for 2026-06-01 09:00 UTC (four L2 lines, eleven fill blocks covering every
  optional field).
- The recorded context value `"funding":"-0.0000027259"` demonstrates
  ten-decimal precision. [HL-93](../venues/hyperliquid.md#hl-93)
- In the archive sample, 1,005 spot rows carry fees with ten decimals, paid
  in tokens such as UBTC. The perpetual rows' `fee`, `closedPnl`,
  `builderFee` and `deployerFee` use `feeToken`, with USDC on the main dex
  and USDC, USDT0, USDH or USDE on the sampled HIP-3 dexes. `priorityGas` is
  in HYPE. [HL-117](../venues/hyperliquid.md#hl-117),
  [HL-28](../venues/hyperliquid.md#hl-28), [HL-31](../venues/hyperliquid.md#hl-31)
- `crates/hl-book/tests/archive_samples.rs` (ignored by default; the samples
  are not in the repository) was run on that whole hour: 6,674 books each for
  BTC and JUP, all valid with 20 levels a side; 52,352 contiguous blocks with
  387,496 perpetual rows, 36,200 spot rows and 2,802 outcome rows; all
  193,748 perpetual trades have exactly one taker row and one maker row.
  [HL-138](../venues/hyperliquid.md#hl-138),
  [HL-100](../venues/hyperliquid.md#hl-100),
  [HL-108](../venues/hyperliquid.md#hl-108),
  [HL-110](../venues/hyperliquid.md#hl-110)
- Cross-check between the two archives on that hour: of 11,108 BTC taker
  fills, 401 (3.6%) are on the wrong side of the book archived before them,
  and none is on the wrong side of both the book before and the book after.
  For JUP it is 0 of 283. The BTC figure is the market moving between books
  that are 0.54 s apart, on a one-tick spread.
  [HL-138](../venues/hyperliquid.md#hl-138),
  [HL-139](../venues/hyperliquid.md#hl-139)

## Consequence for the data-quality checks

A daily check that compares trade prices with the previous snapshot's best bid
and ask and raises an alarm above 1% would fire constantly on tight-spread
instruments. BTC shows 3.6% in a normal hour. The check should count fills that fit
neither neighbouring book, which is 0 here, or use per-instrument
thresholds.
[HL-139](../venues/hyperliquid.md#hl-139)
