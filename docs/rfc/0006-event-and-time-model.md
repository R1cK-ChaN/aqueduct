# RFC 0006: Event and time model

- Status: proposed for owner review
- Date: 2026-11-24
- Applies to: `hl-sim`, `hl-backtest`, paper trading and the replay export

The owner chose the original backtest timing rules on 2026-11-24. They use the market,
order and timer events from RFC 0001. The simulator models when an action
takes effect. The driver determines when the strategy can observe its
effect. [RFC 0001, D2 and D4][core]

[RFC 0008][paper] amends the initial path to use one received-data-only
timing rule for ordinary recording replay and paper trading. The changes
below identify the earlier decisions. Block models, archive synthesis and
latency distributions remain later research work under the same receipt
requirement. RFC 0008 defines the exact scheduling and clock rules.

## Decisions

### D1. Admit market events on receipt and keep both timestamps

Earlier D1 advanced the simulator on `ts_exch` before receipt. RFC 0008
replaces that decision with a receipt requirement shared by ordinary
recording replay and paper trading. Admit a market event only when it has
been received, then use its `ts_exch` for simulated matching and its
`ts_local` for strategy delivery. Exchange time retains its meaning
([HL-95](../venues/hyperliquid.md#hl-95)). A later exchange time in a
received event supplies evidence for an earlier action's effect, following
RFC 0008. The clock alone cannot supply that evidence.

This lets paper and replay make the same decision with the same available
information. Using exchange events from the rest of a recording in advance
would give replay evidence unavailable to paper. Market data and reports
change the strategy's visible state only when delivered. [RFC 0008][paper]

### D2. Break ties by kind of work, source and sequence

Earlier D2 scheduled exchange-side market work ahead of receipt. RFC 0008
replaces that precedence for the initial shared path. At one local time,
process each admitted market or gap input in RFC 0008 D5 file order, then
record and payload order. Apply the simulator's rules to that input, then
deliver it to the strategy. An
accepted selected-coin BBO or live trade can prove pending actions under
D4. Apply proven earlier submissions before matching that input, in effect
time and then submission order.

After all tied market inputs, service simulator wake-ups and deliver due
reports, then fire timers. Tied reports retain creation order and tied
timers retain registration order. After a callback, recheck earlier work
before proceeding to later work. RFC 0008 defines how to close one local
time, including zero delays and callbacks that create more work.

This preserves the sequence paper can observe and makes ties reproducible.
Sorting the complete recording by exchange time would admit evidence before
receipt. Later multi-source work retains stable source identifiers and
within-source sequence. D3's future block model also obeys the receipt
requirement. [RFC 0008][paper]

### D3. Use blocks in the later execution model

Earlier D3 made block selection the next execution model for every replay.
RFC 0008 defers it to the later block-aware research model. The initial
shared path uses the constant-delay effect time in D4. This allows a thin
paper runner to establish replay equivalence before block and fill models
are implemented. A block schedule alone cannot prove an action or release a
report before receipt. The following choices apply to that later model.
[RFC 0008][paper]

Use the complete list of block times when available. Hyperliquid executes
actions in blocks, and the official historical source is `explorer_blocks`.
[HL-1](../venues/hyperliquid.md#hl-1), [HL-118](../venues/hyperliquid.md#hl-118)
The archive is requester-pays. [HL-96](../venues/hyperliquid.md#hl-96)

All measured values in this RFC are provisional until the report is rerun
on at least a week of recordings.
[Feed-timing report][timing-report]

When the complete list is unavailable, keep the distinct observed block times.
The measured median interval between visible live blocks was 69 ms
([HL-121](../venues/hyperliquid.md#hl-121)). An inferred 18.38% of blocks had
no update for the recorded instruments
([HL-122](../venues/hyperliquid.md#hl-122)).

Use the live modal cadence of 67 ms as the nominal spacing. For each gap `g`
between consecutive observed block times, set `n = max(1, round(g / 67 ms))`
and insert `n - 1` synthetic blocks evenly spaced inside the gap. A fixed grid
across the whole run would erase the uneven spacing of the observed blocks.
[HL-121](../venues/hyperliquid.md#hl-121)

The later block model assumes that an action arriving at time `t` executes
in the first block with time `B >= t`, including equality. This boundary rule
approximates the venue's batch execution
([HL-1](../venues/hyperliquid.md#hl-1)). Preserve batch order.
Within each batch, process ALO and other non-cancel actions without a
GTC or IOC first, cancels second, and actions with a GTC or IOC last.
This follows the venue's class order.
[HL-3](../venues/hyperliquid.md#hl-3)

The proposer orders actions within each class, and a modify takes its
replacement's class. [HL-4](../venues/hyperliquid.md#hl-4), [HL-5](../venues/hyperliquid.md#hl-5)
A GTC in one batch can precede a cancel in a later batch of the same
block. [HL-2](../venues/hyperliquid.md#hl-2)

When batch boundaries are unknown, use the race share `w` from RFC 0007
D1. Count one eligible race for each cancel or modify that takes effect
in a block where a historical taker print would fill some of the resting
order if the print executed first. [RFC 0007, D1][fills]

- At `w = 0`, execute all of the block's historical takers before our
  action. A cancel avoids that fill only if it lands in an earlier block.
- At `w = 1`, treat our eligible actions and the block's historical takers
  as one batch. Apply the class order above and preserve submission order
  within each class. For the unknown order between our eligible action
  and those takers, place the action before all of them.
- For `0 < w < 1`, make one draw per eligible race from the run's seeded
  generator in D9's deterministic order. With probability `w`, apply the
  action before all of the block's historical takers. Otherwise apply it
  after all of them. Record the seed in the session file's `start` row.

Make no race draw at either endpoint. At `w = 1`, the one-batch
approximation favours the strategy because it lets our cancel beat a
taker that could have been in an earlier batch. Setting `w = 0` gives the
lower bound on avoiding those fills. [HL-2](../venues/hyperliquid.md#hl-2)
Report both presets and calibrate `w` from observed races at the first
live stage. [RFC 0007, D1 and D11][fills]

### D4. Model market-data, order and report latency separately

Earlier D4 sampled order delay to select an inclusion block, then added a
report-delay draw to the result's block time to schedule delivery.
RFC 0008 replaces them in the initial shared path with configured constants
and a received-evidence rule. This keeps ordinary replay and paper on one
model while private order and report timing remain unmeasured. Sampling a
distribution now would imply calibration we do not have.
[RFC 0008][paper]

| Latency | Initial shared path |
| --- | --- |
| Market data | Preserve receive order and derive logical `ts_local` from the recorded stamps using RFC 0008's clock rule. |
| Order | Set effect time to send time plus constant `order_latency`. An accepted selected-coin BBO or live trade with a strictly later exchange time proves the action. Equality leaves it pending. |
| Report | Deliver no earlier than effect time plus constant `report_latency` and the logical receipt time of the evidence that proves the result. |

The following distribution design remains later research work. Archive
synthesis supplies a simulated receipt sequence, after which the same
receipt requirement applies. Its values are assumptions to calibrate.

| Latency | Interval | How replay obtains it |
| --- | --- | --- |
| Market data | Block time to strategy delivery | Derive recorded `ts_local` using RFC 0008 D4. For archives without strategy receive time ([HL-120](../venues/hyperliquid.md#hl-120)), add a latency draw to the block time, then apply D5. |
| Order | Strategy send to the block that includes the action | Sample venue arrival by order class. Fit the arrival delay so the resulting send-to-block time, including the wait for D3's block, matches the configured distribution. |
| Report | Block where the result occurs to strategy delivery | Add a report-latency draw to the result's block time, then enforce the received-evidence lower bound from RFC 0008. |

In the existing measurements, market-data latency is receive wall time
minus block time. For a frame received at monotonic time `t`, activity is
the number of Hyperliquid `bbo`, `l2` and `trades` frames received in the
preceding second, `[t - 1 s, t)`.
Count each frame once. The bins are 0–69, 70–81, 82–92, 93–105 and 106+
frames. [HL-127](../venues/hyperliquid.md#hl-127)

When scheduling an archive frame at its block time, count deliveries in the
second just elapsed. This estimates activity at receipt without looking
ahead. Sample each channel separately from its latency distribution for that
bin. For trades, use the measured live-trade distribution after excluding
subscription-history batches ([HL-81](../venues/hyperliquid.md#hl-81),
[HL-130](../venues/hyperliquid.md#hl-130)).

| Channel | Overall median (ms) | Overall p99 (ms) | Quietest bin p99 (ms) | Busiest bin p99 (ms) | Source |
| --- | ---: | ---: | ---: | ---: | --- |
| `bbo` | 229.625 | 456.329 | 494.246 | 385.477 | [HL-128](../venues/hyperliquid.md#hl-128) |
| `l2` | 261.301 | 462.932 | 479.352 | 400.676 | [HL-129](../venues/hyperliquid.md#hl-129) |
| live `trades` | 248.989 | 473.335 | 514.846 | 416.36 | [HL-130](../venues/hyperliquid.md#hl-130) |

Reject sampling latency independently of activity. Each channel's median
changes little across bins, while its p99 is highest in the quietest bin.
The quietest bin also has a higher p99 than the busiest bin for each channel
within each sampled hour. [HL-128](../venues/hyperliquid.md#hl-128),
[HL-129](../venues/hyperliquid.md#hl-129),
[HL-130](../venues/hyperliquid.md#hl-130),
[HL-131](../venues/hyperliquid.md#hl-131)

Part of this association may be reverse causation. When delivery stalls,
fewer frames arrive in the preceding second, so the stalled frame can itself
fall into a low-activity bin. Independent sampling would spread the slow tail
evenly over time. Conditioning keeps it where it occurs in the recordings.

**Open question.** How well does exchange-side activity separate the latency
distributions? When rerunning the report, also condition latency on the count
of frames or trades whose block time falls in the preceding second. The
archive supplies this count directly, and a delivery stall does not lower it.
Switch D4 to the exchange-side measure if it separates the distributions at
least as well as the received-frame count.

For the later model, keep the order and report distributions in run
configuration. Start with the priors below. Testnet exercises order flow.
Calibrate the distributions against small live orders at the first live
stage. Use one order-latency distribution for cancels and ALO placements
and another for GTC and IOC orders. This approximates the venue's
cross-block priority for cancels and ALOs.
[HL-12](../venues/hyperliquid.md#hl-12)

### D5. Preserve FIFO on each connection

Earlier D5 described FIFO through sampled delivery times. RFC 0008 adds the
initial recording and paper path, which preserves receive order and uses
its logical-clock rule for local timestamps. Raw stamps remain available
for measurements. This keeps clock corrections from reordering strategy
inputs. Sampling fresh market-data latency would change the observed run.
[RFC 0008][paper]

For later archive synthesis, in frame order set
`ts_local = max(sampled_ts_local, previous_ts_local)` on the same connection.
The first frame uses its sampled time. Equal times retain source sequence,
including across file rotation on the same connection. Other connections
keep their own delivery times. This prevents a sampled delay from reversing
the source's frame order.

### D6. Merge the feeds the strategy sees by local time

Earlier D6 let Hyperliquid market events advance the simulator on exchange
time independently of receipt. RFC 0008 adds D1 and D4's requirements for
receipt and evidence of an action's effect. The initial path admits one
coin from one Hyperliquid source. For later multi-source work, deliver sources on one
strategy timeline ordered by `ts_local`, with D2 resolving ties. Only
received Hyperliquid market events can supply simulated matching evidence.
Using another feed's clock would not establish what happened to our
Hyperliquid order. Keep Binance's own exchange time for analysis
([BN-14](../venues/binance.md#bn-14)). The exchange clocks are independent,
so using Binance exchange time to order its updates against Hyperliquid could
give the strategy information before it arrived.

### D7. Establish opening state and route gap withdrawals through the strategy

Earlier D7 required complete-book startup for every replay and withdrew
simulated orders at gap markers. RFC 0008 scopes complete books to the later
replay export. The initial BBO strategy starts with zero inventory and no
orders, then quotes after its first usable two-sided BBO. Requiring L2
reconstruction would add work this strategy does not use.

The initial path keeps the recording reader's gap markers. `FeedGap`
reaches the strategy, which submits `CancelAll` through the normal delayed
action path. The simulator ignores `FeedGap`, so a pending cancel still
needs later received evidence. Immediate removal would hide this exposure.
Cancellation during an extended gap remains an open design question in
RFC 0008. [RFC 0008][paper]

For the later export, provide the full book for each included instrument
as of the segment start, before emitting its incremental updates.
Reconstruct the book from a snapshot at or before the boundary and any
intervening diffs. The strategy's opening
book contains only updates already delivered. Frames in flight keep their
delivery times. Starting from an empty book loses the levels that subsequent
position-based diffs refer to, as seen at hourly file boundaries in the
loader. [HL-71](../venues/hyperliquid.md#hl-71) ·
[RFC 0003, D3][decoding] · [loader file boundaries](0004-clickhouse-schema-and-loading.md#d8-hourly-files-continue-the-same-stream)

That export will emit `FeedGap` for each affected instrument on **every
disconnect**, regardless of duration, and for a data gap **longer than
5 seconds**. The export must place `GAP` markers at the start and end of
each such gap. Its withdrawal requests must follow the shared delayed
action path. A gap invalidates the reconstructed book until the next
full snapshot.
If the requested segment starts inside that gap, the snapshot is the
earliest valid start.
[RFC 0001, D4][core]

### D8. Reproduce the delivery order of a block's updates

The recorder stamps each WebSocket frame individually. Of 564,931 consecutive
frame pairs, 0 shared an exact receive timestamp and 85.45% fell in the same
millisecond. The evidence for arrival order comes from record order in the
raw files. [HL-126](../venues/hyperliquid.md#hl-126)

For later archive work without a delivery sequence, use `bbo`, then `l2`,
then `trades` within each block. Use this order to construct the source
sequence before applying D5. This order replaces the earlier assumption
that trades precede the book update. A block counts toward
the stated order only when every frame of the first channel arrived before
every frame of the second. [HL-123](../venues/hyperliquid.md#hl-123)

| Within-block order | Blocks in this order / blocks with both channels | Reversed | Mixed | Source |
| --- | ---: | ---: | ---: | --- |
| `bbo` before `trades` | 24,608 / 24,608 | 0 | 0 | [HL-123](../venues/hyperliquid.md#hl-123) |
| `bbo` before `l2` | 10,469 / 10,506 | 36 | 1 | [HL-123](../venues/hyperliquid.md#hl-123) |
| `l2` before `trades` | 3,419 / 3,421 | 0 | 2 | [HL-123](../venues/hyperliquid.md#hl-123) |

Earlier D8 used actual receive timestamps as delivery times. RFC 0008
clarifies that recorded replay preserves raw stamps and derives logical
delivery times from them. This preserves receive order across clock steps.
In the recordings, 348 of 81,179 block groups were interleaved
with frames from other blocks
([HL-124](../venues/hyperliquid.md#hl-124)). Across the sample, 1,875 frames
(1,803 of them trades) carried a block time older than the latest block
already received in their segment
([HL-125](../venues/hyperliquid.md#hl-125)).

The archive default places every frame of a block before every frame of the
next block and therefore does not reproduce these exceptions.

### D9. Make every run reproducible

Earlier D9 required byte-identical run outputs. RFC 0008 specifies which
outputs carry that guarantee. Identical inputs and configuration must yield
identical ordered decisions, reports and ledger values. Keep measured
processing lag in separate diagnostic records. A reviewer can then replay
a disputed fill without reproducing the machine's load.

The constant-delay paper path makes no random draws. For later models with
sampling, record the seed in the session file's `start` row and draw in
deterministic order. Their decision outputs must also reproduce with the
same seed.
[RFC 0008][paper] · [Repository determinism rule][rules]

## Latency priors and sensitivity

These priors belong to the later research models described in
D3 and D4. The initial RFC 0008 run records its configured constants.
Public-feed measurements alone cannot calibrate private order inclusion or
report timing. [RFC 0008][paper]

Use the following observations as initial calibration priors.
The venue's stated latencies and the unverified Tokyo observation
cover different parts of an order's journey. The split between
order inclusion and report delivery remains a configuration
assumption until both are measured.

| Observation | Interval and source |
| --- | --- |
| Median 0.2 s, 99th percentile 0.9 s | Venue-stated co-located order send to receipt of a committed response. [HL-13](../venues/hyperliquid.md#hl-13) |
| About 0.38 s | Venue's example end-to-end time for a cancel or ALO placement. [HL-14](../venues/hyperliquid.md#hl-14) |
| Median 0.88 s | Reported Tokyo market order send to fill, unverified. [HL-136](../venues/hyperliquid.md#hl-136) |

The Tokyo probe measured a 17 ms median round trip through API rejection,
before block inclusion. [HL-133](../venues/hyperliquid.md#hl-133) ·
[Tokyo latency probe][probe]

The operator's research protocol sets the order latencies for sensitivity
runs. [RFC 0007, D12][protocol]

## Implementation work

The shared driver implements action evidence, report delivery and receipt
order for ordinary replay. It closes each time before servicing its
deadlines and stops at the final complete record's logical time. Record
conversion applies the process-clock floor, and the file reader merges
numbered chains in sequence under RFC 0008 D4 and D5. Paper capture will
add the admission lock and live clock sampling. [Current driver][driver] · [RFC 0008][paper]

The following work remains for the later research models.

- **`hl-sim`** will select blocks, apply batch priority, model the latency
  distributions and enforce per-connection FIFO on sampled deliveries.
  When batch boundaries are unknown, use D3's race share `w`. Test a
  resting order, a cancel and a historical taker that would fill the order
  in the cancel's block. At `w = 0` the taker fills the order. At `w = 1`
  the cancel prevents the fill. For `0 < w < 1`, repeat a run with a fixed
  seed and check that the sequence of race outcomes repeats.
  Test D3's block-boundary inclusion rule while retaining the received-evidence
  requirement for action decisions and reports. [Current simulator][sim]
- **`hl-backtest`** will add the later block model and sampled deliveries
  behind the shared receipt requirement and apply D2 to ties. Record each
  run's latency settings and any sampling seed in the session file's
  `start` row. The current input requires nondecreasing local timestamps
  and allows decreasing exchange timestamps. Test interleaved feeds, a
  late frame for an earlier block, tied reports and timers, and byte-identical
  output from a repeated run. [Current driver][driver] · [RFC 0008, D6][session]
- **Replay export** will provide the segment-start books, source sequence and
  `FeedGap` events. Compare the opening book and later updates in a segment
  cut mid-stream with the same span of continuous replay. Check the gap
  threshold at equality and just above it, and a brief disconnect followed
  by a fresh snapshot.

[core]: 0001-core-types-and-interfaces.md
[decoding]: 0003-hyperliquid-market-data-decoding.md
[fills]: 0007-fill-models.md
[protocol]: 0007-fill-models.md#d12-keep-the-research-protocol-in-the-research-layer
[paper]: 0008-paper-trading.md
[session]: 0008-paper-trading.md#d6-record-one-session-per-process-run
[rules]: ../../AGENTS.md
[sim]: ../../crates/hl-sim/src/lib.rs
[driver]: ../../crates/hl-run/src/lib.rs
[probe]: ../../scripts/probes/README.md#latency-probe
[timing-report]: ../../research/reports/feed-timing-2026-11.md
