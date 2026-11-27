# RFC 0001: Core types, strategy interface and order gateway

- Status: implemented in the skeleton; owner review pending
- Date: 2026-11-24
- Crates: `hl-types`, `hl-strategy`, `hl-gateway` (contracts), `hl-sim`,
  `hl-run` (minimal implementations that exercise the contracts)

This document records the decisions behind the first version of the shared
types and the two interfaces every other crate builds on. The rustdoc of each
crate is the precise contract; this file explains why it looks the way it does.

## Goal

One strategy implementation must run unchanged in backtest, paper trading and
live trading. That requires:

1. a single vocabulary for instruments, prices, sizes, orders and events;
2. a strategy interface with no access to IO, clocks or venue APIs;
3. an order gateway interface that a simulator and a live Hyperliquid client
   can both implement.

## Terms

| Term | Meaning |
| --- | --- |
| Instrument | One market identified by a venue and exact symbol text. `InstrumentId` is our stable number for that pair. |
| `ts_exch` | Exchange time of an event. WebSocket and inner archive-row block times have millisecond granularity, while fills envelopes retain submillisecond digits. [HL-95](../venues/hyperliquid.md#hl-95) |
| `ts_local` | Time at which the strategy process sees the event. |
| Order action | A request from the strategy: place, modify, cancel, cancel all. |
| Order event | A report from the venue about one order: accepted, rejected, filled, modified, canceled. |
| cloid | `ClientOrderId`, the 128-bit id we assign to an order. [HL-51](../venues/hyperliquid.md#hl-51) |
| oid | `OrderId`, the id the venue assigns once the order is accepted. [HL-50](../venues/hyperliquid.md#hl-50) |
| Gateway | Whatever turns order actions into order events: the simulator or the live client. |
| Host | `StrategyHost`, the IO-free runtime that feeds a strategy and keeps the state it may read. |

## Decisions

### D1. Fixed-point `i64` scaled by 1e-8 for prices, sizes and money

`Px`, `Qty`, `Usd` and `FeeRate` are newtypes over `i64` with eight decimal
places. This is bit-for-bit the ClickHouse `Decimal64(8)` used by the planned
schema, so values move between Rust and the database without conversion.
Parsing is exact: a string with more than eight decimals is an error, never a
silent rounding. `Display` prints the shortest exact form (no trailing zeros),
which is also the form the venue expects on the wire.
[HL-49](../venues/hyperliquid.md#hl-49)

Separate newtypes keep a price from being added to a size by accident. The
only cross-type operations are the ones with a meaning: `Px::notional(Qty)`
gives `Usd`, `FeeRate::fee_on(Usd)` gives `Usd`. Products go through `i128`
and truncate toward zero.

`hl-types` denies `clippy::float_arithmetic`, so accounting values cannot pick
up floating-point code by accident. Strategies may use floats for signals and
convert at the edge.

Rejected alternative: `rust_decimal`. It is 128-bit, slower, and its layout
does not match the database column. We need one scale, not arbitrary scales.

### D2. One `Timestamp` type; the role is in the field name

`Timestamp` is nanoseconds since the Unix epoch in an `i64`. `ts_exch` and
`ts_local` are both `Timestamp`. The simulator orders exchange-side and
strategy-side work on one timeline, and latency (`ts_local - ts_exch`) is
computed everywhere, so two incompatible types would force a conversion at
almost every use. Nanoseconds match the recorder's receive clock. WebSocket
and inner archive-row block times convert from milliseconds without loss,
and the fills envelope's `block_time` retains its submillisecond digits.
[HL-95](../venues/hyperliquid.md#hl-95)

### D3. `InstrumentId` is our number, not the venue's

**Changed by RFC 0009 D1–D2.** This decision previously put the
`Instrument { id, network, coin, asset_id }` record in `hl-types` and
assigned translation to the codec and live gateway. It referred to RFC 0004
for id derivation. [RFC 0009](0009-instrument-identity.md)

**Our choice.** Events, orders and database rows carry `InstrumentId(u32)`.
`InstrumentId::derive(venue, symbol)` is the only source of ids. `hl-types`
owns this venue-neutral identity. Hyperliquid's `Network`, venue names,
coin metadata, asset id and dex handling belong under
`hl_codec::hyperliquid`, as specified by RFC 0009 D2. The live gateway uses
that metadata. [RFC 0009](0009-instrument-identity.md) ·
[HL-53](../venues/hyperliquid.md#hl-53) ·
[HL-54](../venues/hyperliquid.md#hl-54) ·
[HL-55](../venues/hyperliquid.md#hl-55)

### D4. Events are one enum with three sources

`Event` is `Market`, `Order` or `Timer`. Market and order events carry both
timestamps. Market event kinds in this version: `L2Snapshot`, `Bbo`, `Trade`
and `FeedGap`.

`FeedGap` tells the strategy that data for an instrument is missing from that
point until the next snapshot. Replay emits it for holes in recorded data and
the live feed emits it on staleness or disconnect, so a strategy has one code
path for both.

The in-memory enum is not the on-disk replay format. Snapshots hold their
levels in a `Vec`; drivers pass events by reference and can reuse buffers.

### D5. Strategies see order identity only as `ClientOrderId`

A cloid is assigned when the strategy asks to place an order. Our gateway
contract keeps that identity for the order's whole life, including across
modifies. The venue's oid appears only as information on events. Automatic
venue-side cloid retention and replacement oid policy remain open questions.
If the venue assigns a new oid on modify, the live gateway hides the mapping.
[HL-19](../venues/hyperliquid.md#hl-19), [HL-21](../venues/hyperliquid.md#hl-21)

Layout: version 1 byte, strategy tag 1 byte, instrument
2 bytes, millisecond time 6 bytes, sequence 6 bytes. The host allocates
cloids from the event time and a counter, so the same event stream always
produces the same ids. The instrument field holds the low 16 bits of the
`InstrumentId` and is for people reading logs; uniqueness comes from time and
sequence.

### D6. Strategy interface: one callback, a read-only context, an action sink

```rust
pub trait Strategy {
    fn on_event(&mut self, ctx: &Context<'_>, event: &Event, actions: &mut Actions);
}
```

- `Context` exposes the current time and the strategy's live orders. It will
  gain the local book and the position when `hl-book` and `hl-ledger` exist;
  those are additive changes.
- `Actions` is the only way to act: `place`, `modify`, `cancel`, `cancel_all`,
  `set_timer`. `place` returns the new cloid.
- The trait has no async, no generics over a gateway and no access to a clock.
  A strategy cannot call the venue, so the three environments cannot drift.

Passing a sink gives the same contract as `on_event(...) -> Actions`
without an allocation per event.

`StrategyHost` wraps a strategy and owns the state behind `Context`: it
applies order events to the order table, calls the strategy, then marks the
orders the strategy just acted on as pending. It returns the actions to the
driver. The host does not know about gateways, so `hl-strategy` depends only
on `hl-types`.

### D7. Order states are the live ones; terminal outcomes are events

`OrderState` has `PendingNew`, `Open`, `PartiallyFilled`, `PendingModify`,
`PendingCancel`. An order that is filled, canceled or rejected leaves the
table; the event that ended it is the record. The deferred `Unknown` state
means no report arrived within the timeout. It needs the live timeout logic
and is added with `hl-live`.

An order tracks `open_qty` and `filled_qty`. `Modify { px, qty }` sets the
price and the open quantity, matching the venue, where a modify replaces the
resting order with a new size.
[HL-48](../venues/hyperliquid.md#hl-48)

### D8. Gateway interface: submit a batch, poll reports, time passed in

```rust
pub trait OrderGateway {
    fn submit(&mut self, now: Timestamp, actions: &[OrderAction]);
    fn poll_events(&mut self, now: Timestamp, out: &mut Vec<OrderEvent>);
}
```

- One `submit` for all action kinds keeps the order between a cancel and a
  place explicit and lets the gateway implementation choose how to batch
  them.
- `submit` never fails synchronously. Every outcome, including local
  validation failures, comes back as an `OrderEvent`. Callers have one path
  for results.
- The gateway never reads a clock. The simulator is therefore deterministic,
  and the live implementation uses `now` for action expiry.
- The trait is synchronous and non-blocking. The live implementation will put
  signing and the WebSocket on its own task and exchange messages with these
  two methods over queues. A simulator behind an async trait would add a
  runtime to the backtest hot path for no benefit.

Outcome rules, stated in full in the trait documentation: a `Place` yields
exactly one `Accepted` or `Rejected`; a `Modify` yields `Modified` or
`ModifyRejected`; a `Cancel` yields `Canceled` or `CancelRejected`; a
`CancelAll` yields one `Canceled` per order it removed. A cloid can be placed
once; a second `Place` with the same cloid is rejected and does not touch the
first order. `Filled` can arrive at any time while the order is live,
including while a cancel is pending.

### D9. `hl-gateway` holds the trait; the live client goes behind a feature

`hl-sim` implements `OrderGateway` and must not pull in signing or network
dependencies. The Hyperliquid implementation (hypersdk 0.3.1) will live in
`hl-gateway` behind a non-default cargo feature.

### D10. The simulator needs market data; the trait does not

`SimGateway::on_market_event` accepts inputs in receipt order. A BBO or
trade with exchange time strictly past a pending action's effect time
proves it. The simulator applies the action before the proving input.
`next_wakeup` returns the earliest determined report's delivery time.
`poll_events(now)` releases reports due through `now`. An action can wait
for evidence across any number of polls. [RFC 0008 D2](0008-paper-trading.md#d2-an-action-needs-a-received-event-to-establish-its-effect)

## Dependency rules

```text
hl-types  <-  hl-strategy
hl-types  <-  hl-gateway  <-  hl-sim
hl-strategy, hl-sim, hl-gateway  <-  hl-run  <-  hl-backtest
```

`hl-types` depends on nothing. `hl-strategy` never depends on `hl-gateway`,
`hl-sim` or any crate that performs IO.

## What the minimal implementations cover

`hl-sim::SimGateway` in this version:

- accepts post-only (`Alo`) orders with a positive price and size, subject
  to minimum notional, a unique cloid and the crossing check. Other
  time-in-force values and reduce-only orders produce `Unsupported`.
- checks post-only orders against the newest stored best prices by exchange
  time before applying the proving input. A crossing order produces `WouldCross`.
  [HL-15](../venues/hyperliquid.md#hl-15), [HL-27](../venues/hyperliquid.md#hl-27)
- fills an eligible resting version at its price when an opposite trade
  prints strictly through it after its placement or last successful modify.
  One trade's size is shared best price then oldest first. Late prints
  against superseded versions contribute only to `late_print_misses`.
  [HL-6](../venues/hyperliquid.md#hl-6) · [RFC 0008 D2](0008-paper-trading.md#d2-an-action-needs-a-received-event-to-establish-its-effect)
- adds constant order latency to submission time to obtain the modeled
  effect time. Report delivery is the latest of effect time plus report
  latency, evidence receipt and the previous delivery scheduled for that cloid.
- charges the configured maker fee, truncated toward zero. [HL-141](../venues/hyperliquid.md#hl-141)

`hl-run::replay` feeds the incremental driver in receipt order through an
inclusive stop. At each local time the simulator and strategy handle each
input, then the strategy receives due reports and timers in that order.
Reports due after a timer callback precede the next tied timer.

This is enough to prove the interfaces. It is not a fill model: block-level
ordering, queue position, the F0/F1/F2 models, request-weight limits and
funding belong to later simulator work.

## Scenarios checked against the design

| Scenario | Behaviour |
| --- | --- |
| Cancel sent while a fill is in flight | `Filled` arrives first and removes the order; the later `CancelRejected(UnknownOrder)` is ignored by the host. |
| Cancel sent before the accept arrives | The order stays `PendingCancel`; `Accepted` only records the oid. |
| Price moves before proof of a placement | The simulator checks the stored best prices before applying the proving event. A crossing placement produces `WouldCross`. |
| Report not yet visible | `poll_events(now)` returns only events with `ts_local <= now`. |
| Same cloid placed twice | Second `Place` is rejected with `DuplicateClientOrderId`; the first order is unaffected. |
| Feed interruption | Strategy receives `FeedGap` and can `cancel_all` for that instrument. |
| Timer set in the past | Fires at the current time, never earlier. |

## Deferred, with reasons

- **`ScheduleCancel` action.** This strategy action controls an account-level
  dead-man switch. New-account eligibility remains unverified. Its report
  shape can be decided when it is added with `hl-live`.
  [HL-34](../venues/hyperliquid.md#hl-34),
  [HL-35](../venues/hyperliquid.md#hl-35)
- **Instrument rules.** RFC 0009 D4 revises the original choice of `hl-codec`
  and `hl-book` for this work. It assigns Hyperliquid rule computation
  to venue-specific code and compares two ways to supply dated validation
  rules to `hl-sim`, with the choice marked for the owner. RFC 0007 D10
  owns validation behavior. Until implementation, the example strategy
  does not round prices to a tick.
  [RFC 0009](0009-instrument-identity.md) ·
  [RFC 0007 D10](0007-fill-models.md#d10-validate-new-orders-at-arrival) ·
  [HL-23](../venues/hyperliquid.md#hl-23),
  [HL-24](../venues/hyperliquid.md#hl-24),
  [HL-25](../venues/hyperliquid.md#hl-25),
  [HL-26](../venues/hyperliquid.md#hl-26)
- **Fee rounding.** The simulator's fee model owns fee calculation and its
  truncation-toward-zero assumption. Live trading uses the venue-reported fee.
  `hl-ledger` books the fee carried by each fill.
  [HL-30](../venues/hyperliquid.md#hl-30)
- **`Unknown` order state, taker orders, position and book in `Context`.**

## Open risks

1. Whether a modify keeps the venue-side cloid, and what happens to the
   resting order when a modify is rejected, must be confirmed on testnet. The
   interface does not depend on the answer; the live gateway does.
   [HL-19](../venues/hyperliquid.md#hl-19), [HL-20](../venues/hyperliquid.md#hl-20)
2. "Exactly one outcome per action" cannot be guaranteed by a network client.
   The live gateway must resolve lost reports by querying the venue, and the
   host needs the `Unknown` state for the interval in between.
3. The `Vec` in `L2Snapshot` is fine with buffer reuse, but the replay reader
   has to be written to reuse it to reach the throughput target.

## Verification

- Unit tests in each crate cover parsing, arithmetic, the cloid layout, order
  state transitions and every simulator rule listed above.
- `crates/hl-run/tests/minimal_chain.rs` runs the same `FixedSpreadQuoter`
  type against `SimGateway` and against a second gateway written in the test,
  which shows the strategy has no dependency on the gateway behind it.
