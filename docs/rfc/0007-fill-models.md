# RFC 0007: Fill models

- Status: proposed for owner review
- Date: 2026-11-24
- Applies to: `hl-sim`, `hl-backtest`, paper trading and `research/`

A trade at our resting price fills our order only after it exhausts the
quantity ahead. This RFC specifies how to estimate that quantity and
determine the size and price of each fill. RFC 0006 supplies the execution
block, the order of actions in it and the time the strategy receives the report.
Today the simulator accepts post-only orders and requires a trade strictly
through a resting price. Fills at the order price and the other rules below
remain to be implemented in the simulator and its reports.
[RFC 0006][time] · [Current simulator][sim]

## Decisions

### D1. Report F0 and F1 together

The later fill model has three independent settings. F0 and F1 are fixed
presets.

| Setting | Meaning | Values | F0 | F1 |
| --- | --- | --- | --- | --- |
| Race share `w` | Probability that our cancel or modify takes effect before all of the block's historical takers in an eligible same-block race (D3) | `0 <= w <= 1` | 0 | 1 |
| Cancellation placement | Where to remove an inferred cancellation at our level (D2 step 3) | `back`, or `powerprob` with exponent `n > 0` (D4) | `back` | `powerprob`, `n = 2` |
| Jump share `p` | Share of quantity added at an order age below 400 ms that joins ahead of us (D5) | `0 <= p <= 1` | 1 | 0.5 |

Each strategy has its own simulated account (owner decision, 2026-11-24).
Run it on the same inputs in a separate account for each preset.
F0 is the conservative lower bound. F1 uses the expected queue position
from D4. An optimistic queue estimate adds fills at our quote just before
the price reverts. The operator's research protocol defines the run sets
and how to compare their results.

The session file is the run manifest. Record `w`, the cancellation
placement (with `n` for `powerprob`), `p`, the D6 counterfactual-fill
setting and the timing configuration as simulator parameters in its
`start` row. Include the matching preset name, if any. Record the seed
in the same row whenever the run samples, including every run with
`0 < w < 1`. [RFC 0008, D6][session] · [RFC 0006, D9][time]

F0 and F1 use L2 depth and trades. F2 needs L4 order states and raw book
diffs. Insert the virtual order among identified orders at its price, then
advance it when orders ahead fill or cancel. Honor new-order insertion
positions from the node's `insertBefore` or 0xArchive REST's
`insert_before`. [HL-119](../venues/hyperliquid.md#hl-119), [VN-9](../venues/vendors.md#vn-9)
The vendor's Parquet changes omit those positions. [VN-10](../venues/vendors.md#vn-10)
Verify REST coverage from 2026-07-21 when deciding whether to buy the data.
F2 implementation follows that decision and will validate F1, particularly for
strategies quoting at the best bid or ask.
Use an in-house simulator to model
Hyperliquid's block ordering ([HL-1](../venues/hyperliquid.md#hl-1)), queue
jumps ([HL-8](../venues/hyperliquid.md#hl-8)) and, in later work,
volume-earned request allowance ([HL-32](../venues/hyperliquid.md#hl-32)).
Port the queue functions and latency model from hftbacktest
without adding it as a dependency.

### D2. Reconcile displayed quantity with trades at the order price

Track each instrument, side and price separately. For an order joining an
existing level, seed the **external quantity ahead** from the displayed size
at the end of the placement block. At a known empty price, including a new
level inside the spread, start at zero. Track older orders of our own at
the same price in queue order, outside that external quantity. For a new
order at a displayed price, D2 starts matching at-price trades after that
block closes. Debiting a placement-block print against the closing size
would count its depletion twice. A strict trade-through (D3) or a print
against an inside-spread quote (D6) can still fill in the placement block
if the order was resting when the taker executed.
[RFC 0003, D6–D7][decoding] · [RFC 0006, D3][time]

Use `bbo` for the top level, reconstructed `l2` snapshots or diffs for all
visible levels, and archive snapshots for the levels they cover. Reconcile
on exchange time. A BBO and L2 update describing the same closing level
count as one observation even if delivery times differ. The recorded BBO and
rebuilt top of book agreed at coincident block times. [HL-137](../venues/hyperliquid.md#hl-137)
For consecutive displayed sizes `V0` and `V1`, let `T` be the historical
volume at that price that traded against that side in the interval. Count
each print once.
[RFC 0003, D3 and D6–D7][decoding] · [RFC 0006, D8][time]

1. Spend each eligible print in price and queue order. At the print price
   consume the external quantity ahead first. Exactly exhausting it yields
   zero fill. Our fill is the smaller of the excess trade volume and our
   open size. Multiple orders of ours share the print and the external
   queue between them.
2. Compute `V0 - T - V1` using the historical print volume. A positive
   residual is inferred cancellation. A negative residual is net addition
   of its absolute value. For example, a level unchanged after a trade of
   10 has gained 10 units. New quantity joins behind us except under D5.
3. With `back` placement, assign inferred cancellations to the back.
   With `powerprob`, split them by D4. In either case, bound the resulting
   external quantity ahead to `[0, V1]`. With `back`, a cancellation
   reduces the estimate only if the new displayed size forces this cap.
   A depth decrease by itself cannot generate a fill.

Offsetting additions and cancellations between observations are invisible
to L2. Order-level diffs let F2 distinguish them. An archive snapshot
replaces the visible 20 levels per side in the sampled books. [HL-100](../venues/hyperliquid.md#hl-100)
Beyond that range depth is unknown. Freeze the queue estimate while its
level is out of view. On return, use the first depth as the new baseline
and apply the visible-size cap. For an order first placed beyond coverage,
seed its estimate on first observation. Inferring a cancellation across the blind interval would grant
unobserved progress. At a `FeedGap`, RFC 0006 withdraws our orders and
invalidates the book until a full snapshot.
[RFC 0006, D7][time]

### D3. Share trade-through volume in price-time order

A buy above our ask or a sell below our bid proves that the aggressor
accepted a worse price than ours. Fill at **our resting price**, up to
the quantity printed strictly through it in that block.
Spend each print on eligible orders best price first, then oldest at equal
price. This approximates venue matching priority, with D5 handling fee-driven
queue jumps ([HL-6](../venues/hyperliquid.md#hl-6)). Cap each fill at the
open size. Only the unspent part of that print can reach the queue at the
print price under D2. D3 preserves the simulator's existing volume limit
even when several quotes qualify.
[Current simulator and its matching tests][sim-tests]

Preserve batch order when known. When batch boundaries are unknown, use
the race share `w` for eligible cancels and modifies. At `w = 0` the
historical takers go first. At `w = 1` the action goes first under the
one-batch approximation. For `0 < w < 1`, RFC 0006 D3 selects between
those orders once per eligible race. For a modify, the race concerns the
resting order. Apply D8 if the replacement takes liquidity.
[HL-1](../venues/hyperliquid.md#hl-1) · [HL-3](../venues/hyperliquid.md#hl-3) ·
[HL-5](../venues/hyperliquid.md#hl-5) · [RFC 0006, D3][time]

### D4. Use the expected queue position from PowerProb

For `powerprob` placement, port hftbacktest's unnumbered
`PowerProbQueueFunc` into `hl-sim` as a formula. For a residual
cancellation `C > 0`, let `F` be the external quantity ahead after the
interval's trades. Upstream estimates the back as `B = V0 - F`, using
the preceding observation. Its probability `r` is the share removed
**behind** the order. For exponent `n > 0`, use the following update.
[hftbacktest queue source][queue]

```text
r          = B^n / (F^n + B^n)
next_front = clamp(F - (1 - r) * C - max(0, r * C - B), 0, V1)
```

The last subtraction moves any cancellation too large to fit at the back
to the front. With nothing ahead, the front stays zero. With nothing on
either side, skip the division. The `powerprob` update uses the expected
queue position without a random draw. This gives repeatable, smooth
sensitivity to the exponent. Use fixed-point quantities in the simulator,
round a fractional front up to the representable quantity unit, and execute
only whole lots within trade overflow. Upstream uses floating-point depth
and rounds to the nearest lot when deriving a fill. Our fill must fit the
volume available. [hftbacktest queue source][queue] · [RFC 0001, D1][core]

Use `n = 2` as the initial default and follow the operator's research
protocol for the exponent sensitivity runs.
The current tutorial uses 2 with this variant and 3 with
`power_prob_queue_model3`, a different formula. hftbacktest's Python
builder defaults to a logarithmic model. The exponent here comes from the
tutorial example. A source-derived port must retain upstream's copyright
notice (`Copyright (c) 2022 nkaz001@protonmail.com`) and MIT permission
notice in copies or substantial portions. Include the full MIT text with
the port and cite the source revision.
[Queue tutorial][tutorial] · [Upstream defaults][defaults] · [MIT license][license]

#### Worked example

Our sell order for 20 units rests behind two asks totaling 200 at the
same price. Each row ends at a new observation. The size increase comes
after the priority window. Quantities are in whole lots. The fill columns
show executions in that row.

| Observation at our ask | Displayed size | `back` ahead | `back` fill | `powerprob` ahead (`n = 2`) | `powerprob` fill |
| --- | ---: | ---: | ---: | ---: | ---: |
| Our order joins | 200 | 200 | 0 | 200 | 0 |
| 100 join behind us after 400 ms | 300 | 200 | 0 | 200 | 0 |
| A buy trades 150 | 150 | 50 | 0 | 50 | 0 |
| 60 disappear with no trade | 90 | 50 | 0 | 38 | 0 |
| A buy trades 45 | 45 | 5 | 0 | 0 | 7 |

At the cancellation, `powerprob` puts `50² / (50² + 100²) = 1/5` of the
60 ahead of us, removing 12. The next buy reaches 7 of our 20 lots.
With `back`, all 60 cancellations fit in the 100 behind us. We still
have 5 ahead after that buy.

### D5. Allow fee-paying ALO orders to jump the queue

We pay zero priority fee. L2 does not identify the individual orders.
For our resting order, approximate ALO priority on the block clock.
Measure age from placement or the last D7 reset.
[HL-7](../venues/hyperliquid.md#hl-7) · [HL-8](../venues/hyperliquid.md#hl-8) ·
[HL-9](../venues/hyperliquid.md#hl-9)

For additions observed at `0 <= age < 400 ms`, put the share
`p` ahead of us and the rest behind. From age 400 ms onward
all additions join behind. Until L0 calibration, use
`p = 0.5` as an uninformed prior and follow the operator's research
protocol for the `p` sensitivity runs.

IOC priority changes taker latency and sequencing
([HL-10](../venues/hyperliquid.md#hl-10),
[HL-11](../venues/hyperliquid.md#hl-11)). Our zero-fee model uses RFC 0006
for inclusion and races and D8 for competition with historical takers. It
has no paid-IOC sequencing parameter. [RFC 0006, D3–D4][time]

### D6. Run the strategy twice for fills inside the spread

An accepted bid above the historical best bid or ask below the historical
best ask improves its side while it rests inside the spread. Its external
queue ahead is zero. With counterfactual fills enabled, give it price
priority on opposite-aggressor prints at its price or worse for the
aggressor. Fill at our price within the same shared volume budget as D3.
Flag and count these fills separately in the simulator's fill log. A live
venue cannot flag how a counterparty would have reacted to an absent quote.
Keep that flag private to reporting. The strategy receives the ordinary
`hl_types::Fill`. [Fill contract][orders]

With counterfactual fills disabled, suppress those executions while our
quote improves on the historical best. Once the historical best on our
side reaches or passes our price, the resting order can fill by D2 or D3.
A BBO change alone produces no fill. Permanently excluding the order would
lose subsequent trades in an observed market. For example, our ask at 101
inside a historical 100/102 market can intercept a buy at 102 in the
enabled run. In the disabled run it stays open. Once the historical ask is
101, a buy there can consume its queue and fill it normally. Run the
strategy independently under both settings. Deleting flagged fills after
replay would retain quotes chosen for a different inventory.

### D7. Reset queue priority on a price change or size increase

The simulator treats a successful price change or increase in **open** size
as a cancel and fresh placement. Join the back of the target level under D2,
reset its age for price-time priority and restart D5's window. We assume a
same-price reduction keeps the position and original age. This allows a maker
to give up size while preventing a small placeholder order from reserving
priority for a later increase. The simulator leaves the resting order intact
after a failed modify. Keep the strategy's `cloid` and report
the outcome through the existing modify contract.
[Order actions][orders] · [Gateway contract][gateway] · [Simulator tests][sim-tests]

The reported unchanged-`oid` reduction remains unverified
([VN-11](../venues/vendors.md#vn-11)). Order identity alone does not
establish relative fill order. The documented replacement class and
`always_place` behavior leave reduction priority unspecified
([HL-5](../venues/hyperliquid.md#hl-5),
[HL-18](../venues/hyperliquid.md#hl-18)). Venue-side `cloid` retention,
post-failure resting state and replacement `oid` policy remain open
([HL-19](../venues/hyperliquid.md#hl-19),
[HL-20](../venues/hyperliquid.md#hl-20),
[HL-21](../venues/hyperliquid.md#hl-21)).
Before L0, check on testnet with two equal-fee passive orders at one
price, with the first already outside the fee window. Reduce the first
and observe which receives a small execution. Repeat with a same-price
increase, then a move to a level with a known resting order.
Record order IDs, relative execution order and block times.
Expect the reduced order to stay first and each reset order
to follow the order already at its price.

### D8. Walk visible depth for our IOC and marketable GTC orders

For our arrivals, let the block's historical takers consume liquidity
first. Their order relative to our takers is unknown.
For each instrument, resting side and price, start with the displayed
quantity at block open, subtract the block's historical trades against
that level and floor at zero. Our takers share what remains in submission
order. A buy walks asks upwards through its limit, a sell walks bids
downwards through its limit. Execute at each visited level's price and
debit the available quantity there before the next order can use it.
Discard these debits after the block. D8 keeps the historical book
because replay ignores market impact for the small order sizes this
project uses (owner decision, 2026-11-24).

Stop at the limit or the edge of visible depth. Cancel an IOC's unfilled
remainder ([HL-16](../venues/hyperliquid.md#hl-16)). Rest a GTC's unfilled
remainder at its limit under D2 and D6
([HL-17](../venues/hyperliquid.md#hl-17)).
The immediate executions use the configured taker rate. Later fills of
the resting remainder use the configured maker rate. For an ALO that would
match on arrival, map the cancellation (`BadAloPx`) to
`RejectReason::WouldCross` in `hl-sim`. [HL-15](../venues/hyperliquid.md#hl-15)
Fee tiers, discounts and fee rounding belong to the simulator's fee model.
`hl-ledger` books the fee carried by each fill. [HL-30](../venues/hyperliquid.md#hl-30) ·
[RFC 0001, deferred fee calculation][core]

### D9. Measure the time an instrument spends at one tick

Use consecutive valid `bbo` observations on exchange time. Weight each
spread by the time it holds, clipped to the run boundaries. A spread is
one tick if the best ask is the next valid price above the best bid under
the instrument rules effective then. D9 uses the perpetual price grid's
significant-figure and decimal limits, including its integer-price exception
([HL-23](../venues/hyperliquid.md#hl-23),
[HL-24](../venues/hyperliquid.md#hl-24),
[HL-25](../venues/hyperliquid.md#hl-25)).

Report measured one-tick time, total time and duration without a valid
two-sided BBO. Stop carrying the last BBO at a gap. When there is no room
to improve the quote, queue position determines most fills.
[RFC 0006, D7][time]

### D10. Validate new orders at arrival

**Changed by RFC 0009 D4.** This decision previously placed dated
instrument rules in `hl-types`. The owner chose a pure `hl-rules` crate
for version lookup and validation.
[RFC 0009 D4](0009-instrument-identity.md#d4-supply-dated-instrument-rules-to-hl-sim)

**Our choice.** Require positive price and size. Use limit price times size
as notional and enforce the venue's 10 USD minimum for a new perpetual
order. Return `RejectReason::BelowMinNotional` for the venue's `MinTradeNtl`
error. The simulator lets exactly 10 USD pass this check, an inference from
the stated minimum. D8 supplies the ALO crossing check.
[HL-27](../venues/hyperliquid.md#hl-27)

With the dated rules in RFC 0009 D7, validate the submitted price on D9's
grid and the quantity against the `szDecimals` rule at the order's effect
time, for both new orders and modifies. Reject an invalid instruction.
Silently rounding it could change its queue or make it trade on arrival.
This decision owns validation behavior. RFC 0009 D4 owns the data,
code location and effective-date representation.
[RFC 0009 D7][run-rules] ·
[HL-23](../venues/hyperliquid.md#hl-23) ·
[HL-24](../venues/hyperliquid.md#hl-24) ·
[HL-25](../venues/hyperliquid.md#hl-25) ·
[HL-26](../venues/hyperliquid.md#hl-26)

### D11. Calibrate the fill settings against the first live stage

The fill settings and latency distributions are assumptions until they are
calibrated against live orders. Calibration replays the account's own
order actions against the market recording for the same period and compares
orders by `cloid`. Each run records the settings for this comparison in
the session file's `start` row ([D1](#d1-report-f0-and-f1-together)).
The operator's research protocol defines the procedure and thresholds.

### D12. Keep the research protocol in the research layer

`hl-run` drives one configuration. Measurements computed from that run's
inputs and outputs belong to the run. `hl-backtest` writes its session
file and JSON summary. The operator keeps a pre-registration that defines
which runs to make and how to judge them. `research/` compares runs and
applies thresholds that interpret a result or decide whether a configuration
may advance, even when the threshold uses only one run.

| Item | Owner |
| --- | --- |
| Run one configuration deterministically and write its session file and JSON summary ([RFC 0006, D9][reproducibility] and [RFC 0008, D5][paper]) | per run |
| In the session `start` row, record the fill settings, matching preset (if any), D6 setting and timing configuration. Include the seed when the run samples (D1) | per run |
| Count flagged fills (D6), measure one-tick time, total time and time without a valid two-sided BBO (D9), and report late-print misses ([RFC 0008, D2][late-prints]) | per run |
| Run the operator's sweeps of the fill settings (D1) and [order latency][sweep] | research |
| Pair results and report the selected grid cells and reruns for each D6 setting (D1) | research |
| Classify a result using its measured one-tick time and total time (D9) | research |
| Apply the operator's promotion rule for paper or live trading (D1) | research |
| Calibrate fill settings against live orders using the operator's research protocol (D11) | research |

Calling the command line once per configuration lets `research/` change
run sets and promotion rules independently of the run code shared by
paper trading and replay checks. Use exact decimal arithmetic to compare
decimal strings from JSON summaries and session files. For D11, record the comparison
method and calibration results as specified in the operator's research
protocol. Cite each run's session file for its settings.

## Implementation work

- **`hl-sim`** owns per-level queue state, price-time allocation and the
  simulator-only fill log. Test the worked example with `back` and with
  `powerprob` at `n = 2`. Test partial fills, a visible cap after a
  cancellation, overlapping BBO/L2 updates, hidden depth and a gap.
  Compare at-price and through-price prints in the placement block. Test
  a single print shared across better and equal prices and equal-price
  FIFO. Test the RFC 0006 race for cancels and modifies at both endpoints
  of `w` and with a fixed seed for `0 < w < 1`. Check the effect of `p`
  just before and at 400 ms, all three modify cases, a rejected modify and
  both D6 settings as the historical best reaches the order. Walk an IOC
  and a GTC across levels with earlier historical and simulated consumption. Check
  their remainders, fill prices, liquidity and fee rates, the crossing ALO,
  the 10 USD boundary and, once available, dated tick and size checks.
- **`hl-backtest`** runs one configuration through `hl-run` and writes
  its session file and JSON summary. Test that the D6 switch changes
  inventory and later strategy actions and that the report counts flagged
  fills. Check D9's one-tick, total and missing-BBO times with uneven BBO
  intervals and a gap. Check the late-print miss count and quantity. Repeat
  the same inputs and seed and check for byte-identical outputs under
  [RFC 0006, D9][reproducibility].

Funding, margin, liquidation and request-weight limits
([HL-33](../venues/hyperliquid.md#hl-33)) belong to a separate RFC.
F2 data coverage, testnet modify priority and L0 fit are validation
work at the stages named above.

[core]: 0001-core-types-and-interfaces.md
[run-rules]: 0009-instrument-identity.md#d7-supply-dated-rules-to-backtest-and-paper-runs
[decoding]: 0003-hyperliquid-market-data-decoding.md
[time]: 0006-event-and-time-model.md
[reproducibility]: 0006-event-and-time-model.md#d9-make-every-run-reproducible
[sweep]: 0006-event-and-time-model.md#latency-priors-and-sensitivity
[late-prints]: 0008-paper-trading.md#d2-an-action-needs-a-received-event-to-establish-its-effect
[paper]: 0008-paper-trading.md
[session]: 0008-paper-trading.md#d6-record-one-session-per-process-run
[sim]: ../../crates/hl-sim/src/lib.rs
[sim-tests]: ../../crates/hl-sim/src/tests.rs
[orders]: ../../crates/hl-types/src/order.rs
[gateway]: ../../crates/hl-gateway/src/lib.rs
[queue]: https://github.com/nkaz001/hftbacktest/blob/5f3ec40b2afb764e0fea112f941ed85523ef4e88/hftbacktest/src/backtest/models/queue.rs#L124-L240
[tutorial]: https://hftbacktest.readthedocs.io/en/latest/tutorials/Probability%20Queue%20Models.html
[defaults]: https://github.com/nkaz001/hftbacktest/blob/5f3ec40b2afb764e0fea112f941ed85523ef4e88/py-hftbacktest/src/lib.rs#L129-L145
[license]: https://github.com/nkaz001/hftbacktest/blob/5f3ec40b2afb764e0fea112f941ed85523ef4e88/LICENSE
