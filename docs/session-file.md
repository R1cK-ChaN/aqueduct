# Session file reference

Schema version 1 records one strategy run under RFC 0008 D6 and D8. `hl-run`
encodes the rows while its driver processes callbacks, actions, reports and
timers. Each driver supplies its build identity and raw manifest. `hl-backtest`
checks a sealed session or the latest midnight checkpoint of an open paper
session offline.

## Encoding and ordering

The file is UTF-8 JSON Lines with one object per line and a terminating newline.
The tables below give object fields in their output order. Objects are compact,
with JSON string escaping and no spaces between fields. Identical inputs and
run settings produce identical bytes.

| Type used below | Encoding |
| --- | --- |
| Text | JSON string. Enum spellings are lowercase with underscores. |
| Decimal | Exact fixed-point decimal string without an exponent or redundant fractional zeros. |
| Nanoseconds | Canonical signed decimal integer string, in epoch nanoseconds unless a field says duration. |
| Identifier or sequence | Canonical nonnegative decimal integer string. |
| Count | Nonnegative JSON integer. |
| Boolean | JSON `true` or `false`. |
| Optional value | Its normal type when present, otherwise JSON `null`. |
| Client order ID | JSON string containing `0x` and exactly 32 lowercase hexadecimal digits. |

The first row is `start`. A sealed session ends with `end`. The work rows `action`,
`report`, `gap`, `timer_set` and `timer_fired` share one consecutive `seq`
starting at `"0"`. Their order is the driver's work order. A delivered gap,
report or fired timer precedes the actions from its strategy callback. The
driver submits a callback's order actions in their returned order, then
registers its timer requests in their returned order. Ties between inputs,
reports and timers follow RFC 0008 D3.

## `start`

| Field in order | Type and meaning |
| --- | --- |
| `kind` | Text `"start"`. |
| `schema_version` | JSON integer `1`. |
| `commit` | Full Git commit hash of the build, or `"unknown"` when Git is unavailable. |
| `dirty` | Boolean indicating whether any tracked file differed from that commit when built. Git being unavailable sets this to `true`. |
| `driver` | Text `"backtest"` or `"paper"`. |
| `network` | Text `"mainnet"` or `"testnet"`. |
| `source` | Paper source name. Backtest writes `null`. |
| `coin` | Text containing the venue coin name. |
| `instrument` | Identifier of the internal instrument. |
| `process_start_wall_ns` | Paper process anchor in epoch nanoseconds. Backtest writes `null`. |
| `strategy_name` | Text used to select the strategy implementation. |
| `strategy_tag` | Identifier in the range 0 through 255, used when constructing client order IDs. |
| `strategy` | Object containing the component's ordered parameter pairs. Every value is a string. |
| `sim` | Object starting with text field `model`, followed by the simulator's ordered parameter pairs. Every value is a string. |

The component parameter functions define the contents and order of `strategy`
and the parameter fields after `sim.model`. They list every setting, including
defaults, and their parameter names carry units. The strategy's instrument is
the fixed `start.instrument` field. Rebuilding a component rejects missing,
unknown, duplicate or malformed parameters. A future component setting is
represented by that component's parameter list.

The initial host order table, simulator and timer queue are empty. Accounting
starts at zero. The current model is named `strict_trade_through` and the
backtest CLI selects `fixed_spread` as its strategy.

## `action`

Every action starts with `kind`, `seq`, `ts_local_ns`, `variant`, in that order.
`kind` is `"action"`, `seq` is a sequence string, and `ts_local_ns` is the
local submission time in nanoseconds. The remaining fields depend on
`variant`.

| Variant | Remaining fields in order |
| --- | --- |
| `place` | `instrument`, `cloid`, `side`, `px`, `qty`, `tif`, `reduce_only` |
| `modify` | `instrument`, `cloid`, `px`, `qty` |
| `cancel` | `instrument`, `cloid` |
| `cancel_all` | `instrument` |

`instrument` is an identifier. For `cancel_all`, `null` selects every
instrument. `cloid` is a client order ID. `side` is `"buy"` or `"sell"`.
`px` is a decimal price and `qty` is a decimal quantity of the instrument.
A modify's `qty` is its requested new open quantity. `tif` is `"alo"`,
`"gtc"` or `"ioc"`. `reduce_only` is a boolean.

## `report`

Every report starts with `kind`, `seq`, `ts_local_ns`, `ts_exch_ns`,
`instrument`, `cloid`, `variant`, in that order. `kind` is `"report"`.
`ts_local_ns` is the logical delivery time, and `ts_exch_ns` is the effect
exchange time, both in nanoseconds. `seq`, `instrument` and `cloid` use the
same types as their action fields.

| Variant | Remaining fields in order |
| --- | --- |
| `accepted` | `oid` |
| `rejected` | `reason` |
| `filled` | `tid`, `oid`, `side`, `px`, `qty`, `liquidity`, `fee` |
| `modified` | `px`, `qty` |
| `modify_rejected` | `reason` |
| `canceled` | `reason` |
| `cancel_rejected` | `reason` |

`oid` is the simulator's order identifier and `tid` is its fill identifier.
Both are integer strings. `side`, `px` and `qty` use the action field types.
A modified report's `qty` is the confirmed open quantity. `liquidity` is
`"maker"` or `"taker"`. `fee` is a decimal USD amount, positive for a cost
and negative for a rebate.

Rejection reasons are `would_cross`, `below_min_notional`, `invalid_order`,
`duplicate_client_order_id`, `unknown_order` and `unsupported`. Cancellation
reasons are `requested` and `venue`.

## `gap`

| Field in order | Type and meaning |
| --- | --- |
| `kind` | Text `"gap"`. |
| `seq` | Sequence string. |
| `cause` | Text `"disconnected"`, `"shutdown"`, `"dropped"`, `"process_change"` or `"truncated"`. |
| `instrument` | Identifier of the affected instrument. |
| `ts_local_ns` | Logical delivery time in nanoseconds. |
| `raw` | Raw position object. |

A raw position object contains `file_index`, then `record_index`, both
nonnegative integer strings counted from zero. `file_index` identifies the
entry in `end.manifest`. `record_index` counts every complete raw record in
that file, including `file_open` at zero, controls and records that produce
no event for the selected coin.

A process-change gap points to the new file's header. A truncation gap points
to the last complete record in that file, or its header when it has no later
complete record. The other causes point to their lifecycle records. A file's
recorder `file_seq` is separate from its manifest index.

## Timers

| Row kind | Fields in order |
| --- | --- |
| `timer_set` | `kind`, `seq`, `timer_id`, `ts_local_ns`, `deadline_ns`, `registration_order` |
| `timer_fired` | `kind`, `seq`, `timer_id`, `ts_local_ns` |

`kind` contains the row kind shown in the table. `seq`, `timer_id` and
`registration_order` are integer strings. Registration order starts at zero
and advances for every timer request. The times are epoch nanoseconds.

For `timer_set`, `ts_local_ns` is the requesting callback's time and
`deadline_ns` is the deadline it requested, including a deadline in the past.
For `timer_fired`, `ts_local_ns` is the actual logical firing time. A timer
requested in the past fires at the requesting callback's logical time after
higher-priority work at that timestamp. The end row counts timers whose
firing time is later than the inclusive stop.

## `diagnostic`

Paper writes one observation for each UTC minute containing a market callback.
It groups inputs by logical receipt time and includes feed gaps. Input ordinals
count converted market events from zero, in delivery order, including events
from the same frame. Mid-minute shutdown publishes the final group.

| Field in order | Meaning |
| --- | --- |
| `kind` | Text `"diagnostic"`. |
| `minute_start_ns` | Epoch nanoseconds at the start of the UTC minute. |
| `callbacks` | Number of market callbacks in the minute, as a JSON count. |
| `first_input`, `last_input` | First and last input ordinals in the minute. |
| `max_lag_ns`, `max_lag_input` | Longest time from the record's monotonic admission stamp to the start of its strategy callback, and that input's ordinal. |
| `max_duration_ns`, `max_duration_input` | Longest elapsed monotonic time around the strategy callback, and that input's ordinal. |
| `resident_memory_bytes` | Process resident memory in bytes when this observation is written. A JSON count from Linux `VmRSS`, or `null` when unavailable. |

Timestamps, durations and ordinals are integer strings. A tied maximum keeps
the first input. These rows have no decision `seq` and the verifier skips them.

## `checkpoint`

Paper writes one checkpoint for each UTC midnight strictly after process start
and through the inclusive stop, in increasing order.

| Field in order | Type and meaning |
| --- | --- |
| `kind` | Text `"checkpoint"`. |
| `cutoff_ns` | Exclusive midnight cutoff in epoch nanoseconds, as a string. |
| `last_seq` | The `seq` of the last work row strictly before the cutoff, as a sequence string, or `null` if there was none. |
| `last_raw` | Raw position of the last record needed for the prefix, or `null` if no record qualified. Same encoding as `end.last_raw`. |
| `manifest` | Ordered array of sealed raw files from `file_seq` zero through `last_raw.file_index`. Each entry has the same path, sequence and SHA-256 encoding as in `end.manifest`. An empty prefix has an empty array. |
| `late_print_misses` | Same `trades` count and decimal `qty` object as in `end`, cumulative through logical receipt times strictly before the cutoff. |

The raw extent includes headers and records that emit no event for the coin.
For these records the converter uses the logical stop time at that position.
A frame with several events belongs in the extent if its first event precedes
the cutoff. Replay releases only the events and work strictly before midnight,
including reports and timers due during silence. A late print received at
midnight first contributes to the following day's totals.

The writer captures the boundary once RFC 0008 D4 has closed earlier times.
It writes the checkpoint once it has written all work rows through `last_seq`
and every file in its manifest has sealed, then flushes and syncs the session
file. Work at or after midnight may already be on disk when the raw file seals.
On shutdown the writer publishes all due checkpoints before `end`. Backtest
sessions contain only the whole-run boundary at `end`.

## `end`

| Field in order | Type and meaning |
| --- | --- |
| `kind` | Text `"end"`. |
| `stop_ns` | Inclusive logical stop in nanoseconds. |
| `last_raw` | Raw position of the final complete record, or `null` when there was no raw file. |
| `manifest` | Ordered array of raw-file objects. |
| `pnl` | Complete accounting object described below. |
| `open_orders` | Count of orders in the strategy host's local order table. |
| `pending_actions` | Count of submitted actions awaiting qualifying market evidence. |
| `undelivered_reports` | Count of determined reports due after the stop. |
| `outstanding_timers` | Count of timers due after the stop. |
| `late_print_misses` | Object containing `trades`, then `qty`. `trades` is a count and `qty` is a decimal quantity of the selected instrument. |

Each manifest object contains `path`, `file_seq`, `sha256`, in that order.
`path` is text containing an absolute path or a path relative to the session
file's directory. `file_seq` is an integer string, or `null` for a legacy
recording. `sha256` is a string of 64 lowercase hexadecimal digits hashing
the stored file bytes, including compression. The manifest uses the raw
reader's replay order.

The current backtest ends at the final complete raw record's logical time.
It processes market inputs, reports and timers through that inclusive stop.
The recorded stop and last raw position are checked against the replay's
raw extent. A complete backtest session requires a nonempty manifest.

| `pnl` field in order | Type and meaning |
| --- | --- |
| `excluded` | Array containing the text `"funding"`. |
| `cash` | Decimal USD trading cash before fees. |
| `position` | Signed decimal quantity of the instrument. |
| `mark_px` | Optional decimal midpoint price. |
| `mark_ts_exch_ns` | Optional exchange time of the final usable midpoint, in nanoseconds. |
| `notional` | Decimal USD traded notional summed across both sides. |
| `fees` | Decimal USD sum of reported fees. |
| `gross` | Decimal USD final profit before fees. |
| `net` | Decimal USD final profit after fees. |
| `max_abs_position` | Decimal maximum absolute quantity held after a delivered fill. |
| `min_net` | Decimal USD minimum defined net profit, starting from zero. |
| `min_net_ts_exch_ns` | Optional first exchange time of the minimum negative net profit, in nanoseconds. |

The shared accounting adapter uses usable BBO midpoints received through the
stop and fills delivered through the stop. It processes them by exchange
time, marks before fills on a tie, preserving input order within each kind.
It verifies final net profit with an independent fixed-point calculation
from the delivered fills. These are the same account quantities used by the
ordinary stdout summary. The session also records trading cash and expresses
account times as nanosecond strings.

## Writing and checking

`hl-backtest run --session-out <path>` preserves the ordinary JSON summary
on stdout. It refuses an existing destination, writes `<path>.part`, and
renames the completed file to `<path>`.

`hl-backtest run --check-session <path>` accepts no additional run arguments.
The library validates the file structure, consecutive sequences and known
row kinds, and rejects duplicate JSON object fields. It requires the running
binary's full commit and dirty flag to match the session. A mismatch reports
both identities. Equal dirty flags permit a check and produce a warning that
they do not prove equal code.

The checker resolves the manifest and verifies every raw-file SHA-256 before
constructing the strategy and simulator. It then replays the raw files with
the recorded parameters through the inclusive stop. The strategy produces
new actions and the simulator produces new reports. The checker compares
every regenerated field, including extra or missing fields and rows, exact
decimal strings, accounting, raw positions and outstanding work. It reports
the first differing `seq`, or `start` or `end`, with the field, expected value
and replayed value. A match prints one line and exits 0. A failed check exits 1.

For paper, the manifest must be a single chain in sequence from file zero.
Every header must match the source and process anchor in `start`. The sealed
check requires complete raw files, a final raw `shutdown` record and an inclusive
`end.stop_ns` equal to its derived logical time. It checks these before
running the strategy. Paper seals its raw files before writing the footer,
syncing the session file and renaming `session.jsonl.part` to `session.jsonl`.
It then syncs the containing directory.

After a decision or boundary row matches field by field, the checker compares
its bytes, including field order, spacing and the terminating newline. It
reports the first row with an encoding difference even when its values match.

For an open paper session, pass `session.jsonl.part` to the same command.
The checker uses the last complete checkpoint and ignores subsequent bytes,
including an unfinished line. Until the first checkpoint it reports that
there is nothing to check yet. Before replay it verifies the build identity,
raw chain and hashes, consecutive UTC midnights since process start and that
each checkpoint manifest is a prefix of the next. It replays from the first
raw record, checks each checkpoint and compares every work row through the
last checkpoint's `last_seq`, including the absence of additional work before
the cutoff. A pass prints the UTC cutoff and the number of rows compared.

For a sealed paper session the checker also verifies the midnights through
the stop and checks the checkpoint manifests against the final manifest.
In both modes each checkpoint must match in value and in canonical bytes.
Its physical placement among the work rows can vary with raw file sealing.

The scheduling, gap and accounting decisions are specified in RFC 0008.
[RFC 0008](rfc/0008-paper-trading.md)
