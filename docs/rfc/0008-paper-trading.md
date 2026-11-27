# RFC 0008: Paper trading with one timing rule for backtest and paper

- Status: accepted (2026-11-26)
- Date: 2026-11-25
- Applies to: `hl-live`, `hl-backtest`, `hl-run`, `hl-sim` and the capture library in `hl-recorder`

This RFC specifies running `hl-live` on its own public market-data connection.
The strategy can quote, learn that an order was accepted, and receive fills, all
against `hl-sim`. The process holds no key and sends no order. The test of
this stage is to replay the process's own recording and reproduce each
action, each report and the final account. A match checks the live loop.
Whether the simulated fills resemble a real account is a later experiment.
[Public feed access, HL-41][hl41]

## D1. Use the same evidence in both drivers

**Venue interface.** The register defines the exchange timestamp,
subscription history and trade batches. [HL-95][hl95] · [HL-81][hl81] · [HL-82][hl82]

**Measurements.** The Tokyo sample put median public-feed delay at
229.625 ms for BBO (best bid and offer) and 248.989 ms for live trades. In
that sample, 1,875 frames carried an exchange time earlier than one already
received. Separately, the eight-file BTC backtest on 2026-11-25 used the
earlier replay order. Its reader counted 5,149 `exchange_time_out_of_order`
events. The timing of a private fill report relative to the public trade
frame is unmeasured. [HL-124][hl124] · [HL-125][hl125] · [HL-128][hl128] ·
[HL-130][hl130]

**Our choice.** Both drivers give the simulator market events in receipt
order. The earlier backtest sorted a second copy by exchange time and could
tell the strategy about a fill before the process had received the trade.
Keeping that shortcut would make it hard to tell whether a difference from
live trading came from the drivers' evidence or from the simulated venue.
Replaying paper with the same rule lets us test the drivers on their own.
[Current replay][driver] · [RFC 0006][time]

Start with one coin and `FixedSpreadQuoter` at the settings of that BTC run.
Use a half-spread of `6.0`, size `0.00042`, `order_latency` 400 ms,
`report_latency` 250 ms, maker fee 1.5 bps and minimum order notional `10`.
These are run settings. The fee and minimum have separate register entries.
Keeping the strategy, configuration and existing strict trade-through fill
rule fixed makes the effect of a timing change measurable. The queue and
taker models in RFC 0007 belong to later research.
[HL-141](../venues/hyperliquid.md#hl-141) · [HL-27][hl27] · [RFC 0007][fills]

## D2. An action needs a received event to establish its effect

**Simulator assumption.** An order sent at local decision time `t` takes
effect at modeled exchange time `A = t + order_latency`. A qualifying event
is a decoded BBO or an admitted live trade for this coin and connection. Call
its exchange time `E` and its logical receipt time `R` (D4). Use trades in
payload order. At a known connection start, the existing filter admits each
trade with `trade.time >= connected.recv_wall_ns`, in common units.
The single-coin recording path takes no evidence from subscription history,
other coins, control frames, gaps or timers. At the simulator
interface, a qualifying event proves pending actions across instruments
using a single modeled exchange clock. [Recording reader][reader] ·
[HL-81][hl81] · [HL-82][hl82]

The simulator and strategy handle each qualifying event in receipt order.

1. Apply all previously submitted actions with `A < E`, oldest `A` first,
   then in submission and batch order. The first such event proves the
   action. Stamp an acceptance, rejection, modify or cancellation with `A`.
   For post-only checks use the newest **already received** BBO by exchange
   time at or before `A`, breaking ties by receipt order. The proving event
   is still to be applied. A missing opposite quote passes the simulator's
   crossing check. [Current simulator][sim] · [HL-15][hl15]
2. Apply the event itself. A BBO updates the simulated best prices if it is
   at least as recent as the stored BBO. A trade can hit the remaining size
   of a currently resting order whose placement or last successful modify
   has `A < E`. Under the existing matching rule an opposite trade must go
   strictly through the price. Spend its size once, best price then oldest
   first, at our resting prices. Stamp each resulting fill with the trade's
   `E`. [Current simulator][sim] · [Matching priority, HL-6][hl6]
3. Give the market event to the strategy at `R`, even when its `E` is older
   than the last event's. Submit its actions at `R`. An event with `E = A`
   leaves a pending action in the queue. A trade received at that boundary
   while the action is pending faces the previously resting orders. A
   zero-delay response to a quote cannot use that same quote as proof.
   A quiet market or disconnect can leave an action pending indefinitely.
   Only later qualifying data can establish the outcome.

This boundary gives a receipt-order answer even when channels interleave.
A late trade can still fill an eligible resting order. It cannot fill a
price introduced after the trade or revive an order already canceled. This
can miss a fill against an earlier version. Revising a cancel already
reported to the strategy would also require revising its subsequent orders.
We keep the observed decision history and record these misses in the
`late_print_misses` counter below.
Global exchange-time sorting or inferring an empty block in quiet periods
would require information the paper process does not have. [HL-124][hl124] ·
[HL-125][hl125]

**Missed fills from late prints.** When a successful cancel (including
cancel-all) or modify supersedes a resting version at effect time `S`,
`hl-sim` saves the version's instrument, side, price, matching priority and
remaining open quantity. It records the effect time `A` of placement or
the last successful modify and the superseding effect time `S`. For each
later admitted trade in the same instrument, the version qualifies if
`A < E <= S`, it has quantity left and the opposite-side trade goes strictly
through its price. Under D2 step 3, a trade with `E = S` would have faced
the old version if it had arrived while the cancel or modify was pending.

Spend one copy of the trade's size across qualifying superseded versions
in step 2's order, best price then oldest first. At each version take the
smaller of the unspent trade size and its saved quantity and subtract it
from both. Later prints can claim only the version's remainder. If the
total allocated for this print is positive, increment `late_print_misses.trades`
once and add that total to `late_print_misses.qty` in fixed-point units of
the selected coin. These allocations affect only the counter and its saved
quantities. They produce no fill, report or ledger entry.

Keep each superseded version with quantity left until the run ends,
including across file rotations, reconnects and daily checkpoints. Any
admitted print through the inclusive stop can still carry an `E` inside an
earlier interval. Keeping that interval covers the print regardless of how
the trades channel orders its frames.

**Report delivery.** Let `X` be the effect time (`A` for an action outcome,
`E` for a fill). The first possible delivery is `max(X + report_latency, R)`.
For the same client order also take the maximum with its previous report's
scheduled delivery, so its reports stay in the order produced. Across
orders deliver by due time, then creation sequence. The exchange time in the
report stays `X`. `next_wakeup` can advertise a determined report's deadline.
`poll_events(now)` releases due reports and never proves a pending action.
Thus an already determined report can arrive during silence. Releasing one
solely at `X + report_latency` would sometimes reveal an unseen trade.
[Gateway ordering contract][gateway] · [RFC 0001, D8 and original D10][core]

### Worked example of one bid, its acknowledgement and a partial fill

**Test inputs.** Follow the bid in `quoter_trades_against_the_simulator`.
All times below are milliseconds from the fixture origin. The BBO
`0.0250 / 0.0260` has exchange time 0 and arrives at 250. At that instant
the strategy places 500 at `(0.0250 + 0.0260) / 2 - 0.0002 = 0.0253`.
The fixture sets order latency to 100 and report latency to 50. The next
input is a sell of 200 at `0.0251`, exchange time 1,000, received at
`1,000 + 250 = 1,250`. These are illustrative test values. [Fixture][example]

| Report for the bid | Effect on the simulated venue | Delivery with exchange-time sorting | Shared-rule delivery |
| --- | --- | --- | --- |
| `Accepted` | `250 + 100 = 350` | `350 + 50 = 400` | The trade first proves `1,000 > 350`. `max(400, 1,250) = 1,250`. |
| `Filled` | At 1,000 the sell trades through `0.0253`. It fills `min(500, 200) = 200` at our bid. | `1,000 + 50 = 1,050` | `max(1,050, 1,250) = 1,250`. The acknowledgement was created first. |

At 1,250 the strategy sees the trade, then the acknowledgement, then the
fill for this bid. Its open size is now `500 - 200 = 300`. The old driver
had already delivered both reports by the time it delivered the trade.

## D3. Give the live loop a deterministic order of work

**Our choice.** The shared driver always takes the earliest local timestamp
with work. At each timestamp it runs the following steps in order.

1. Process all market and gap inputs in raw file sequence, record position,
   then event position within the payload. For each, do D2's simulator work
   on that input before its market callback. The host routes each callback's
   actions immediately, in the order returned.
2. Service simulator wake-ups. Make already determined reports due at this
   time available, then deliver them in D2 order. A market callback tied
   with a report therefore still sees the previous order state.
3. Fire due strategy timers in registration order. A timer requested in the
   past is due at the requesting callback's time. After each callback,
   recheck higher-priority work at this timestamp before taking the next
   timer. Process work due now before advancing time.

The host uses the timestamp of the delivered event for `ctx.now()` and for
actions. Letting the first thread to wake win a tie could change whether the
quoter still sees a modify as pending. We assume zero strategy computation
time on this timeline. A slow callback increases the measured lag in D6.
Charging that elapsed time to the next order would make machine load a
strategy input.
[Strategy host][host] · [RFC 0006, D2][time]

## D4. Close a timestamp before firing its deadlines

**Our choice.** Keep the RFC 0002 wall and monotonic stamps. Within a
process let `P = process_start_wall_ns + recv_mono_ns`, an epoch time that
advances with the monotonic clock. For a market event use
`R = max(recv_wall_ns, P, E, previous_R)`, omitting `previous_R` at start.
A lifecycle input uses the same rule without `E`. These are nanoseconds.
The reader applies the process-clock floor before raising local time to
the preceding event's local time. This floor also lets the planned live
loop close a time even if the wall clock steps back. Keep raw wall
time for feed-latency measurements and the connection cutoff. For a legacy
file lacking a process anchor, replay derives its anchor from the
`file_open` wall stamp minus its monotonic stamp.
[RFC 0002, D2][raw] · [Recording reader][reader]

Gap conversion uses the lifecycle record's stamps, the new header's
stamps and anchor for a process change, and the last complete record's
stamps for truncation. A header-only truncation uses the header. The
inclusive stop uses the final complete record's wall and process-clock
times, raised to the preceding event's local time. Clock arithmetic that
exceeds the timestamp range fails the run.

Use one admission lock for the planned paper source. The socket reader takes it
as soon as a read completes, stamps and appends the complete raw item to
the ordered queue, then releases it. Lifecycle records use that same path.
The scheduler takes the **same lock** to sample the anchored monotonic
clock and take the whole admitted prefix. It decodes that prefix before
declaring any time `T` closed. If the sample is **strictly greater** than
`T`, every future admission has `R > T`. It can now finish the market work
at `T` and release tied reports and timers. A later decoded input with
`R > T` also closes `T` by receive order. Wake at a deadline and resample
if the clock still equals `T`. Hold the lock only for handoff, leaving
decoding, disk IO and strategy callbacks outside it.

A reader paused just after stamping still holds the lock. The scheduler
must see its frame before closing `T`. A reader paused before admission
receives its stamp when it acquires the lock. Merely polling an empty queue
and firing a timer could skip a frame already stamped `T` in another
thread. A forward wall-clock step can put `R` ahead of the anchored clock.
Tied deadlines then wait for a strictly later input or for the anchored
clock to pass `R`. That wait is processing lag. The strategy still sees the
scheduled timestamp. Replay can look at the next input time or the end of
the recording to close a timestamp without sleeping. Neither driver needs
scheduler wake-up records in the raw format.

## D5. Run the paper connection inside `hl-live`

**Our choice.** The process that makes the decisions must capture the
frames it actually saw. Reading a separate recorder's stream would couple
their availability and substitute another process's receive times.

| Owner | Responsibility in the planned paper path |
| --- | --- |
| `hl-recorder` library | Expose the existing public-feed client, subscriptions, reconnect and clock code with an admission callback. Reuse `LogWriter` for hourly rotation, compression and sealing. The independent recorder keeps its current sink and RFC 0002 output. |
| `hl-run` | Use `hl-codec` to decode records incrementally. Select the coin, track connection state, apply the history cutoff, handle gaps and convert raw stamps to D4 local time. Run the D2 scheduling steps and order strategy callbacks, report deliveries and timers as in D3. Encode the D6 session rows and compute the ledger summary. |
| `hl-backtest` | Discover, order, read and decompress the raw files. Own the command line, including `--check-session`, its manifest checks and its field-by-field comparison. |
| `hl-live` | Own the socket instance, raw writer, real-time pacing, `StrategyHost`, selected gateway and session file. Paper selects `SimGateway`. Future live trading uses this process's strategy, order-state and session-logging paths with a live gateway. |
| `hl-strategy`, `hl-sim`, `hl-ledger` | Keep the quoter and its local order table in the host, simulated matching in the gateway, and fixed-point accounting in the ledger. The host sees changes to its orders only through its normal action and report paths. |

We call the library `hl-run` because it drives one strategy run from the
first input to the final ledger summary. It does no IO and reads no clock.
Both drivers feed it stamped records and their raw positions in receipt
order. `hl-backtest` reads them from disk and uses lookahead or end of input
to close a timestamp. `hl-live` supplies admitted frames and lifecycle
records from its own socket connection and the clock sample taken under
the D4 admission lock. Both depend on `hl-run` and neither depends on the
other. We rejected keeping this code in `hl-backtest` because it would
make the trading process depend on the offline program and its file reader.

`hl-run` depends directly on `hl-types`, `hl-codec`, `hl-strategy`,
`hl-gateway`, `hl-sim` and `hl-ledger`. Its only direct external dependency
is the workspace's existing `serde_json` for metadata and JSON encoding.

Within a numbered recorder process `hl-backtest` must traverse files by
`file_seq`, even if a wall-clock step makes a later file appear to have
opened earlier. Reject sequence holes and duplicates. For ordinary runs
spanning processes, merge the heads of the separate chains by header wall
time and path, keeping each chain in sequence. Legacy unnumbered files
retain their independent boundaries. A paper session always has one
numbered chain. [RFC 0002, D2][raw] · [Current file reader][files]

`hl-live` uses the recorder library for capture. The scheduling changes in
this RFC use `hl-sim`'s existing market-input and wake-up methods, along with
the existing `hl-types`, `Strategy`, `OrderGateway` and RFC 0002 raw
recording format.
[Recorder][recorder] · [Current `hl-live`][live] · [RFC 0001][core] · [RFC 0002][raw]

Use one admitted stream for the raw writer and decoder. If the bounded
ingress queue drops a frame, omit it from both and publish the ordinary
`dropped` count through that ordered stream before the next accepted frame
or shutdown. Preserve the order of lifecycle markers even when data slots
are full. Losing a required marker or failing a write leaves the session
incomplete. Two queues that lose different frames could let the strategy
act on evidence absent from disk. Raw files contain the ordinary recorder frames
and metadata. Our orders, reports and measurements go in the session file.
Mixing our outputs into the market recording would compromise its use as
an independent replay input. [RFC 0002, D1–D5][raw]

## D6. Record one session per process run

The [session file reference](../session-file.md) specifies the version-one
row layouts, units and component parameter objects.

**Our choice.** Give each run its own directory, even after a restart
within the hour. Write `session.jsonl.part` beside its RFC 0002 raw tree
and seal it as `session.jsonl`. Use UTF-8 JSON Lines, one object per line
with a `kind`, in append order and with fixed field order. Write fixed-point
values as exact decimal strings without exponent or redundant fractional
zeros. Use decimal integer strings for nanosecond timestamps, sequences and
numeric IDs. The strategy's order ID (`cloid`) uses `0x` and 32 lowercase
hex digits. The simulator's order ID is `oid` and its fill ID is `tid`.
An absent optional value is `null`. Actions, reports, gaps and timers share a
single consecutive `seq`. Ordinary JSON tools can read the result. A binary
dump of Rust structs would tie the history to a build. [Order IDs][orders]

| Kind | Fields and meaning |
| --- | --- |
| `start` | Schema version, full Git commit, network, source, coin and instrument ID, process start, strategy name and client-order-ID strategy tag, half-spread and quantity. Include the simulator model name, order and report latencies with units, maker fee in bps and minimum notional. Materialize all defaults. The host, simulated orders and timers start empty, the ledger starts at zero and this model uses no random draws. |
| `action` | Local submission time, variant and all fields. A place includes `cloid`, instrument, side, price, quantity, time-in-force and `reduce_only`. A modify includes instrument, `cloid`, price and new open size. A cancel names instrument and `cloid`. A cancel-all includes its optional instrument. |
| `report` | Local **delivery** time, effect exchange time, instrument, `cloid`, variant and all variant fields. Include accepted `oid`, modified price and size, cancellation or rejection reason, or the entire fill (`tid`, `oid`, side, price, quantity, liquidity and fee). |
| `gap`, `timer_set`, `timer_fired` | Record each delivered gap's cause, instrument, time and raw record position. Record a timer's ID, requesting time, requested deadline and registration order, then its actual logical firing time if delivered. Their positions among actions and reports matter. |
| `diagnostic` | For a market callback, record admission-to-callback lag using its raw monotonic stamp. Measure callback start-to-finish separately on the monotonic clock. Associate samples with their input or callback ordinal. These observations have no decision `seq` and are excluded from replay equality. |
| `checkpoint` | For a run crossing UTC midnight, record the exclusive midnight cutoff, last decision `seq` before it (or `null`), and the raw extent that covers it. Identify the last raw position by `file_seq` and record index. Include the sealed files' paths and SHA-256 through that extent, and cumulative `late_print_misses` (integer `trades`, decimal string `qty`) for trades with logical receipt time `R` strictly before the cutoff. D8 defines when this prefix is ready to compare. |
| `end` | Inclusive stop time and last raw record, ordered manifest of sealed relative file paths, `file_seq` and SHA-256, final ledger result, number of locally open orders and counts of unproved actions, undelivered reports and outstanding timers. Include cumulative `late_print_misses` (`trades` and `qty`) through the inclusive stop. |

Compute the session's final ledger from the usable BBO midpoints in its
input and the fills **delivered by its stop**. The shared accounting adapter
sorts these entries by exchange time, marks before fills on a tie, stable
within each kind. This is a valuation after the run and cannot feed a
decision. Compare trading cash and the complete `pnl` object, including position,
mark price/time, notional, fees, gross and net profit, maximum absolute
position, minimum net and its first time, and the funding exclusion. Keep
the independent net check. Reordering only the paper ledger by receipt
could change its inventory or profit extremes. [Accounting adapter][pnl]

## D7. Start, reconnect and stop at reproducible boundaries

**Our choice.** A fresh process starts recording before connecting, even
mid-hour. It has no opening quote, orders or inventory. The quoter can
first place orders when it receives a two-sided BBO. A complete run that
ends before then records the gap or shutdown actions it actually produced
and a flat ledger. Borrowing the recorder's prior hour would give the
strategy an opening state it never observed. Full-book reconstruction
belongs to the later depth export in RFC 0006 D7. [Quoter][quoter] ·
[RFC 0002][raw] · [RFC 0006][time]

On disconnect or a recorded drop, deliver the reader's `FeedGap` at its
local time. The strategy requests `CancelAll` through the normal action
path. `hl-sim` ignores the gap and retains resting orders and its cached
best prices. A cancel can therefore remain unproved across a disconnect.
Reconnect and resubscribe on the same session, reinstate the `connected`
history cutoff, process eligible trades, and resume quoting on a fresh
two-sided BBO. Hourly rotation carries connection and order state forward.
**Open question.** Should the simulator invalidate its book or withdraw
orders differently when data is missing? This path keeps today's behavior
so the timing experiment has one change to explain. [Reader][reader] ·
[Simulator][sim] · [Quoter][quoter] · [HL-81][hl81]

On a clean stop, stop accepting new frames and publish the ordinary raw
`shutdown` after the last admitted item. Its derived local time is the
inclusive cutoff. Process its gap and all D3 work due through that time,
then freeze. Leave later reports, timers and actions awaiting evidence in
the end counts. Draining until idle could manufacture a cancel after the
feed ends or run a recurring timer forever. A plain recording replay ends
at its final complete record's logical time. An internal process boundary
in a multi-file backtest still emits the reader's gap. [Recording reader][reader]

Seal the raw files before publishing the session footer and renaming the
partial session file. A crash, truncated chain, missing footer or failed
write leaves an incomplete session whose prefix can be inspected. Start
the next run from empty state with its own files. Treating a restart as a
continuation would require restoring orders, pending reports and timers
as well as the book. [RFC 0002, D2–D3][raw]

## D8. Replay the recording and compare decisions in order

The offline check and its first-difference output are documented in the
[session file reference](../session-file.md#writing-and-checking).

**Offline check.** At the recorded commit, run the planned
`hl-backtest run --check-session <session.jsonl>`. For a sealed session,
verify the source, process anchor, sequence from file zero, file hashes,
complete footer and stop record. The command uses the ordinary decoder and
driver on exactly those raw files with the `start` parameters, from the
first record through the inclusive cutoff. A session with no BBO yet can
still be checked against the startup behavior in D7. Recompute
connection boundaries, gaps, timers, strategy actions, simulator reports and
ledger from those inputs. Supplying the recorded actions or reports as input
would allow the check to pass despite a different strategy decision.

Compare the ordered `seq`, kind, timestamps and **every** D6 action, report,
gap and timer field, then the end counts and each ledger field, with exact
fixed-point equality. At both `checkpoint` and `end`, recompute and compare
`late_print_misses.trades` and `late_print_misses.qty` exactly for the D6
cutoff. A missing or extra row, a tie in a different order, or a one-unit
fee difference fails. Report the first differing sequence,
field, expected and replayed values. Exclude only the observational timing
samples from this comparison. A matching PnL or a matching count alone
could hide two different order histories.

For the daily check on a still-running process, use the `checkpoint`
in the open session log. Capture its extent only after D4 has closed all
times before UTC midnight and D3 has finished the callbacks due before
that boundary. Publish and flush it once those raw files have sealed and
the corresponding decision rows have been written. A later raw frame alone
cannot show that the strategy has finished a slow callback.

Verify the checkpoint's chain from file zero and its hashes. Replay **from
session start** to the recorded raw extent, releasing all work strictly
before the cutoff, even if the market went quiet. Compare those decision
rows through the checkpoint's `last_seq`, including the absence of extra rows. Work
at midnight belongs to the next day. A completed session uses the inclusive
`end` check above, including its final ledger.
Check restarted sessions separately. This preserves the opening orders
for each day's comparison.

**Expected effect, to be measured.** The receipt floor can delay a fill
report. At the next BBO the strategy may still attempt to modify an order
the simulator has filled, or leave a placement alone while its acknowledgement
is outstanding. The changed actions can change later fills and PnL in
either direction. Cancels and repricings often follow an adverse price
move, so we expect the missed fills at the old prices to be mostly losing
ones. Omitting them biases both backtest and paper PnL upward by an unknown
amount. Every `hl-backtest run` summary reports `late_print_misses`, including
`{"trades":0,"qty":"0"}` when no fill was missed. Its count and quantity
measure the trading affected by this bias. [Quoter][quoter]

**Limits against live trading.** These are simulator assumptions and future
measurements. Agreement with its own replay supplies no observation of a
private order. Testnet follows paper to exercise signing and order flow.
Measure trading behavior at the small live stage (L0).
[RFC 0007, D11][fills]

| Assumption in paper | Where it will be measured |
| --- | --- |
| A strict trade-through stands in for queue position, partial fills and same-time cancel/modify races. | At L0 match real and replayed orders by `cloid`, compare fill sizes and which cancels beat trades, including prints received late. Use the queue calibration in RFC 0007 D11. [HL-6][hl6] · [HL-8](../venues/hyperliquid.md#hl-8) |
| Constant order/report latency and the public receipt floor stand in for private order timing. | At L0 timestamp send, reported effect and private receipt alongside the public frame. Measure order and report delays and how often the private report comes first. [Exchange time, HL-95][hl95] |
| Only the simulator's validation and rejection reasons run. It charges a configured maker fee and models no request budget. | Testnet exercises request and error handling. At L0 reconcile actual rejections, request-limit responses and fees to the order log under normal flow. [HL-15][hl15] · [HL-27][hl27] · [HL-30](../venues/hyperliquid.md#hl-30) · [HL-32](../venues/hyperliquid.md#hl-32) · [HL-33](../venues/hyperliquid.md#hl-33) |
| Our hypothetical quote never changes anyone else's orders or the market feed. | At L0 compare the actual quote's fills and subsequent market moves with replay at the same sends. The gap estimates model error. An individual counterparty's alternative reaction remains unobservable. [RFC 0007, D6 and D11][fills] |

## Implementation order

1. The record converter, replay driver and typed ledger summary are in
   `hl-run`. File IO and JSON formatting remain in `hl-backtest`. The
   eight-file runs reproduced the previous output bytes.

   The incremental D3 driver implements the D2 timing rule and the
   inclusive D7 replay stop. Its tests cover the worked example, strict
   equality, zero delays, tied callbacks, pending work at the cutoff and
   missed late prints.
   For a bid with `0.4` left, prove its cancel with a BBO, then receive a
   through-price sell of `0.6` with `A < E <= S`. Expect one missed trade,
   missed quantity `"0.4"` and no fill report or ledger entry for the print.
   Repeat for a successful modify. In separate runs of the cancel case, a
   late print with `E = S` counts as one missed trade of `"0.4"`, and a late
   print strictly after the cancel's effect time leaves both totals at zero.
   Test one trade shared across superseded orders and consecutive trades
   exhausting a saved size.
   Rerun the eight files from 2026-11-25 with the original `hl-backtest run`
   commands and settings for BTC and `xyz:NVDA`, one coin per run. Compare
   against both the exchange-time-sorted backtests and the runs that added
   PnL to their summaries. Compare input counts, report times where traces
   are available, actions, fills, rejection reasons, open orders and ledger
   results. Measure the deltas and repeat each new run to check determinism.

   Replay applies the D4 process-clock floor and uses D5 file
   ordering by merged chain heads. Its hand-computed tests cover a
   backward wall-clock step across rotation, legacy anchors, gap and stop
   stamps, raw-wall cutoff and latency comparisons, interleaved chains
   and checked clock arithmetic. The summary reports the event count and
   maximum raise from the process clock.
2. The paper stage has four parts.

   1. `hl-run` encodes session rows and `hl-backtest` writes sessions from
      ordinary recordings and compares a fresh offline replay.
   2. The recorder exposes its feed connection as a library and preserves
      its recording output. This work is independent of the offline check.
   3. `hl-live` implements the paper process, admission lock and minute
      diagnostics. The offline checks replay its sessions.
      Loopback tests cover the stamp/handoff boundary, equal stamps, a quiet
      timer, queue drops, reconnects and mid-hour restarts. The owner decides
      when to run a session against the public feed.
   4. Paper accounting uses compact marks and fills. The paper process
      writes midnight checkpoints for daily prefix checks. Tests check that
      a missed print with pre-midnight `E` received at midnight first appears
      in the next day's totals. The owner runs a local paper session across
      UTC midnight and checks both the open and sealed session.

   Risk controls, a watchdog, fault drills, daily reports and a systemd unit
   each need a measured reason before implementation.

[core]: 0001-core-types-and-interfaces.md
[raw]: 0002-raw-recording-format.md
[time]: 0006-event-and-time-model.md
[fills]: 0007-fill-models.md
[driver]: ../../crates/hl-run/src/lib.rs
[reader]: ../../crates/hl-run/src/recording.rs
[files]: ../../crates/hl-backtest/src/recording.rs
[example]: ../../crates/hl-run/tests/minimal_chain.rs
[pnl]: ../../crates/hl-run/src/pnl.rs
[orders]: ../../crates/hl-types/src/order.rs
[gateway]: ../../crates/hl-gateway/src/lib.rs
[sim]: ../../crates/hl-sim/src/lib.rs
[host]: ../../crates/hl-strategy/src/host.rs
[quoter]: ../../crates/hl-strategy/src/fixed_spread.rs
[recorder]: ../../crates/hl-recorder/src/lib.rs
[live]: ../../crates/hl-live/src/lib.rs
[hl6]: ../venues/hyperliquid.md#hl-6
[hl15]: ../venues/hyperliquid.md#hl-15
[hl27]: ../venues/hyperliquid.md#hl-27
[hl41]: ../venues/hyperliquid.md#hl-41
[hl81]: ../venues/hyperliquid.md#hl-81
[hl82]: ../venues/hyperliquid.md#hl-82
[hl95]: ../venues/hyperliquid.md#hl-95
[hl124]: ../venues/hyperliquid.md#hl-124
[hl125]: ../venues/hyperliquid.md#hl-125
[hl128]: ../venues/hyperliquid.md#hl-128
[hl130]: ../venues/hyperliquid.md#hl-130
