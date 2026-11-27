# RFC 0009: Instrument identity and venue metadata

- Status: D1–D7 accepted. The owner chose option (b) for D4 on 2026-11-26 and accepted [D7][d7] on 2026-11-27.
- Date: 2026-11-26
- Applies to: `hl-types`, `hl-codec`, `hl-rules`, `hl-loader`, `hl-run`, `hl-backtest`, `hl-live`, `hl-sim` and the future live gateway

## D1. Derive an id from the venue and the exact symbol

**Our choice.** `InstrumentId::derive(venue, symbol)` is the only source of
ids. Keep RFC 0004 D1's 32-bit FNV-1a over the UTF-8 bytes of the venue,
one ASCII colon, then the UTF-8 bytes of the symbol. Preserve case and every
byte of the symbol, including any colon in it. `InstrumentId::new` and
`get` wrap and unwrap a number that is already known. [RFC 0004 D1][loading]

For Hyperliquid, pass `hyperliquid-mainnet` or `hyperliquid-testnet` and
the complete coin name. The existing mainnet ids for `BTC` and `xyz:NVDA`
are `0x619a6e2d` and `0x0b9863d6`. Testnet `BTC` is `0x6704e12a`. Keeping
the inputs and hash preserves every existing id, including references in
ClickHouse, recordings and session files. [HL-55][hl55] · [Id tests][id-tests]

Select by exact name before using the number. The recording driver admits
the requested coin, checking each trade in a batch. The backtest CLI checks
the recording network and reports an error if the requested coin has no BBO.
The loader derives an id for every coin it loads from archive books,
perpetual fills and supported recording frames.
The codec's WS and archive callers choose which names to accept through D3.
The recorded-book test accepts every coin in its fixture. A registry keyed
by `(Network, coin)` would make each driver carry a Hyperliquid list even to
name a market on another venue.

The loader keeps the collision check in RFC 0004 D1. It compares each
derived id with its `(venue, coin)` owner in stored rows and in the same
batch. Seeing the same pair again is safe. A different pair with that id
stops the load before the market rows are written. The existing
`hl.instruments` columns hold the name, id, dex and ingest id. Dated rules
belong to D4. [Schema][schema] · [Loader][db]

## D2. Keep Hyperliquid names and metadata in `hl-codec`

**Our choice.** Keep `InstrumentId` and its venue-neutral operations in
`hl-types`. Put `Network` and its canonical venue names under
`hl_codec::hyperliquid`. Its `instrument_id(network, coin)` helper calls
`InstrumentId::derive`. Use `Network::from_url` for recording headers and
`Network::from_venue` as the inverse of `Network::venue` for persisted names.
Use `Network::short_name` and `Network::from_short_name` for the network
field in configurations and sessions. `dex(coin)` extracts the dex prefix
when writing instrument rows. Step 1 removes the unused asset id. D4 will
supply it with dated metadata in the Hyperliquid layer.
Signing uses the asset id for the network, coin and metadata version in D4.
Market data conversion needs only the coin name and network.
Keeping `Network` in the shared types would make every consumer depend on
Hyperliquid's naming decisions. [HL-40][hl40] · [HL-53][hl53] ·
[HL-54][hl54] · [HL-55][hl55]

On 2026-11-26, only codec tests and the recorded-book test called the two
event conversions. Only tests constructed an `Instrument` with an `asset_id`.
The table lists the uses at the time and their destinations. The last rows
cover callers added for session replay and paper trading.

| File | Use before the move | Destination and crate change |
| --- | --- | --- |
| `crates/hl-types/src/instrument.rs` | Defines `InstrumentId::hyperliquid`, `Network::venue`, `Instrument { id, network, coin, asset_id }`, `dex()`, `InstrumentRegistry` and `RegistryError`. Its tests also build registries. | Keep the raw id and generic hash tests in `hl-types`. Move network, id helper, metadata, dex and their test vectors to `hl-codec`. Retire the lookup maps, duplicate-key errors and their construction tests. |
| `crates/hl-types/src/lib.rs` | Re-exports the instrument types and errors. | Export only `InstrumentId` from this module through `hl-types`. Callers import Hyperliquid items from `hl-codec`. |
| `crates/hl-codec/src/hyperliquid/events.rs` | `market_events` looks up each coin on a network and rolls back the frame on `UnknownCoin`. | Keep conversion and rollback in `hl-codec`. Take the caller's resolver from D3. |
| `crates/hl-codec/src/hyperliquid/archive.rs` | `trade_events` looks up each crossed fill's coin on a network and skips unlisted names. | Keep the archive conversion in `hl-codec` and use D3's selected-name resolver. |
| `crates/hl-codec/src/hyperliquid/mod.rs` | Exports both conversions and `UnknownCoin` and documents the lookup. | Publish the Hyperliquid helpers here and document the new conversion signatures in `hl-codec`. |
| `crates/hl-run/src/recording.rs` | `RecordConverter::new` derives one id. `text_frame` builds BBO and trade events itself. Tests construct the converter with `Network`. | Import the codec network and id helper. Have `hl-run` call D3 and keep its receipt clock, coin selection, history cutoff, gaps and counters. |
| `crates/hl-backtest/src/main.rs` | Parses `--network`, defaults to `mainnet` and calls `InstrumentId::hyperliquid` for the strategy. | Use the codec network and id helper in `hl-backtest`. Preserve the short CLI spellings, errors and summary text. |
| `crates/hl-backtest/src/recording.rs` | `read_header` maps the URL host to `Network`. Header validation and `read_recordings` pass the network through to the converter. | Use the shared URL parser in `hl-codec`. Keep path errors and requested-network checks in `hl-backtest`. |
| `crates/hl-loader/src/sources.rs` | `Batch::instrument` derives ids and records names. The L2 and fills archive readers select mainnet. `network_of_url` chooses the recording network for metadata, state checks and frame loading. | Use the codec network, id helper and URL parser. |
| `crates/hl-loader/src/sources/rawlog_state.rs` | Stores `Network`, writes `network.venue()` and parses the two stored names back by hand. | Use `Network::venue` and its inverse in `hl-codec`. Keep v1 and v2 reads, v2 writes and unknown-name errors in `hl-loader`. |
| `crates/hl-loader/src/db.rs` | `add_instruments` checks id ownership and independently splits `coin` at a colon to fill `dex`. | Keep the collision check in `hl-loader`. Call the codec's `dex` when constructing an instrument row and write the empty string for no dex. |
| `crates/hl-loader/src/lib.rs`, `crates/hl-loader/src/rows.rs`, `crates/hl-loader/src/main.rs` | Pass the batch to `add_instruments`, define `InstrumentRow` and read id/coin labels for reports, respectively. | Keep the batch, database row and display lookup in `hl-loader`. |
| `crates/hl-codec/src/hyperliquid/ws/tests.rs` | Builds a BTC/PONS registry with numbered ids and dummy asset ids. | Use exact-name selection and derived ids in `hl-codec` tests. Keep the strict caller's mixed-batch rollback and wrong-network cases. |
| `crates/hl-codec/src/hyperliquid/archive/tests.rs` | Builds BTC/TRX entries and exercises selected and unlisted fills. | Use a network-scoped selector and derived ids in `hl-codec` tests. Check skipped rows, order and both timestamps. |
| `crates/hl-book/tests/recorded_book.rs` | Assigns fixture coins sequential ids, constructs a registry, then groups events by id. | Derive from each coin as it is seen. Keep the book/BBO comparison in the `hl-book` test using its existing codec dev-dependency. |
| `crates/hl-loader/src/sources/tests.rs` | Uses the id helper, both URL hosts, stored venue names and state fixtures. | Use codec helpers in `hl-loader` tests. Keep literal expected rows and legacy JSON to catch spelling changes. |
| `crates/hl-backtest/tests/recordings.rs` | Supplies header URLs, constructs the converter with `Network` and derives a strategy id. | Use the codec imports in `hl-backtest` tests. Keep the existing recordings, expected events and CLI output checks. |
| `crates/hl-run/src/session.rs` | `SessionStart` stores the network and the encoder writes its short name. | Store the codec `Network` and write `short_name()`. |
| `crates/hl-backtest/src/session.rs` | Parses the short name and derives the expected id during replay. | Use `Network::from_short_name` and `instrument_id`. |
| `crates/hl-backtest/tests/sessions.rs`, `crates/hl-run/tests/minimal_chain.rs` | Construct networks for session fixtures. | Use the codec `Network`. |
| `crates/hl-live/src/config.rs`, `crates/hl-live/src/lib.rs` | Parse the configured short name, derive the strategy id and pass both into the paper session start. | Use the codec parser and id helper in config. Pass the typed values through to the session. |
| `crates/hl-live/src/output.rs`, `crates/hl-live/src/scheduler_tests.rs` | Construct networks and ids for paper tests. | Use the codec `Network` and `instrument_id`. |
| `crates/hl-backtest/src/recording.rs` | A paper session supplies its declared network even for loopback or proxied recording URLs. | Keep the declared network authoritative. Use `Network::from_url` for standalone recordings. |

The recorder stores configured URLs and subscription text.
It supplies the recording header that readers classify. `hl-run`,
`hl-loader` and `hl-backtest` already depend on `hl-codec`. The `hl-book`
library, `hl-strategy`, `hl-sim` and `hl-ledger` consume the neutral id.

## D3. Convert each frame to market events once

**Our choice.** Keep RFC 0003's two layers. The decoder produces venue
records. The second layer in `hl_codec::hyperliquid` maps their fields
to `MarketEvent` through two functions.

```rust
fn market_events(
    frame: &WsFrame,
    resolve: impl Fn(&str) -> Result<Option<InstrumentId>, UnknownCoin>,
    ts_local: Timestamp,
    out: &mut Vec<MarketEvent>,
) -> Result<(), UnknownCoin>;

fn trade_events(
    block: &FillBlock,
    resolve: impl Fn(&str) -> Option<InstrumentId>,
    ts_local: Timestamp,
    out: &mut Vec<MarketEvent>,
);
```

The resolver captures the venue and the caller's selection. `Some(id)`
supplies the D1 id for an accepted name. `None` skips that name. A strict
WS caller can return `Err(UnknownCoin)` for an unexpected name. On error
`market_events` restores the output length from before the frame, including
when a later trade fails. `trade_events` applies selection to crossed rows.
Both keep input order, each event's exchange time and the local time
supplied by the caller. Control frames append nothing. The loader uses
the first layer directly for its full rows. [RFC 0003 D1 and D6][decoding] ·
[HL-83][hl83] · [HL-110][hl110]

`hl-run` supplies raw receipt time and the requested coin's precomputed id.
Its resolver skips other names, even within a trade batch. From the returned
events it keeps BBOs and applies the trade cutoff from
`hl_codec::hyperliquid` to each trade. The loader uses the same cutoff. An
unknown connection start admits trades. After a `connected` record, a trade
is live if its exchange time is at or after that record's raw wall time.
A disconnected source admits none. `hl-run` sets delivery time on admitted
events, emits gaps and keeps the counters, including skipped trades and
decode errors. Even a skipped frame updates the last complete receipt used
for the inclusive stop. Keeping the second field mapping in `hl-run` would
make the driver learn each new venue's frame shapes when feeds join the
strategy timeline. [HL-81][hl81] ·
[RFC 0008 D4 and D7][paper] · [RFC 0006 D6][time]

## D4. Supply dated instrument rules to `hl-sim`

**Venue facts.** The rule inputs and signing metadata are specified in
[HL-23][hl23], [HL-24][hl24], [HL-25][hl25], [HL-26][hl26],
[HL-53][hl53] and [HL-54][hl54].

**Our choice.** Key each metadata history by network and exact coin, and
associate it with the derived id. Each version carries an `effective_from`
exchange `Timestamp` (UTC), the venue's price-grid and `szDecimals` inputs,
and the asset id for that version. Starts are distinct and increasing. A version
applies from its start through the instant before the next start. At an
order's exchange effect time, use the last version whose start is at or
before that time. A run that needs rules before the first version reports
missing coverage. A metadata revision preserves the id. Replay uses the
same dated inputs as the run it reproduces. RFC 0007 D10 owns validation
on arrival. [RFC 0007 D9 and D10][fills]

| Option | `hl-codec` | `hl-sim` | Future live gateway |
| --- | --- | --- | --- |
| (a) Pass rule parameters as configuration | For each version, the Hyperliquid layer supplies the significant-figure limit, maximum price decimals, integer-price exemption and size decimals. [HL-23][hl23] · [HL-24][hl24] · [HL-25][hl25] · [HL-26][hl26] | The driver fills a simulator configuration record with these fields, the instrument id and `effective_from`. At the order's effect time, `hl-sim` selects the version and applies the Hyperliquid price and size rules to the submitted values. | The live adapter implements the same checks using the same parameters and signs with the asset id for that version. |
| (b) Share a pure rules crate | The Hyperliquid layer decodes metadata and passes the id, effective starts and rule inputs to a new workspace crate with no external dependencies. That crate looks up the version by id and time and checks the submitted price and size. | Call the rules crate with the order's instrument id, effect time, price and size. | Use the same rules crate for validation and get the corresponding asset id from the Hyperliquid metadata. |

**On 2026-11-26 the owner chose option (b), a pure rules crate.** Like the
crates listed under Dependencies in `AGENTS.md`, the rules crate takes no
external dependencies. Add it to that list when the crate is created.

Option (a) keeps the configuration as data, but `hl-sim` and the live
adapter must each implement the significant-figure rule and its integer
exemption. Option (b) adds a venue-specific workspace dependency to `hl-sim` so both
callers use one implementation of version lookup and validation. Both
options keep the codec and its external dependencies out of `hl-sim` and
the `OrderGateway` trait crate.

## D5. Name Binance USDⓈ-M futures instruments

**Venue interface.** The two symbol spellings used for the naming decision
are documented in [BN-9][bn9] and [BN-18][bn18].

**Our choice.** Set `venue` to `binance-usdm-futures`. It names the product
across the recorder's public and market sources and leaves spot a separate
venue name. Set `symbol` to the exact decoded text of payload field `s`,
preserving case. For example, hash `binance-usdm-futures:BTCUSDT` to
`0xc6d21d7b`. Subscription strings and recorder source names never enter
the hash. A decoder for another venue is separate work.

## D6. Implement after both paper trading changes merge

**Our choice.** First implement session writing and offline replay and expose
the recorder's feed connection as a library. Take the comparison baseline
from the build with both changes before changing instrument identity.

1. **Move identity and metadata.** Change `hl-types`, `hl-codec`, `hl-loader`,
   `hl-run`, `hl-backtest` and the `hl-book` integration test together.
   Introduce D3's resolver signatures so removing the old types leaves a
   buildable workspace. Preserve every existing id by passing the same bytes
   to the same hash. Pin D1's three vectors. Test the shared URL parser with
   a port and a lookalike host. Check both venue-name round trips, old state
   JSON, dex extraction and collisions within a batch and against stored
   names. Replay the eight files with the per-coin settings of the original
   BTC and `xyz:NVDA` backtests. Compare each complete JSON stdout byte for
   byte with its pre-step baseline and repeat each run. Timing diagnostics
   go to stderr. Check the existing ids and file format by replaying a
   session as specified in [RFC 0008 D8][paper].
2. **Use the codec conversion in `hl-run`.** Replace the second field mapping
   in `RecordConverter`. Exercise `hl-codec`, `hl-book`, `hl-run` and
   `hl-backtest` with a mixed-coin trade frame, strict rollback, archive
   selection, cutoff equality, a gap and a final ignored record. Compare
   emitted ids, timestamps, order and counters. Repeat both stdout
   comparisons and the paper session replay.
   This step is implemented.
3. **Pin the Binance naming rule in `hl-codec`** when reference-data work
   starts. Test D5's hash vector and distinguish changes in case or venue.
4. **Add the pure rules crate from D4.** This crate, `hl-rules`, is
   implemented. Its tests cover two versions of one id just before and
   at the change, and a time before coverage starts. Continue with the
   codec reader, simulator checks, session replay and driver inputs, in
   the order given in [D7][d7]. In future gateway tests, check that a
   synthetic action selects the matching network, rule version and asset id.

**Measurement.** The existing book/BBO comparison is the independent
check on the book events in step 2. [HL-137][hl137]

## D7. Supply dated rules to backtest and paper runs

**Our choice.** A new backtest or paper run may take a local perpetual rules
file. With a file, apply RFC 0007 D10. Without one, use today's simulator
checks and write `no grid check applied` to stderr. Requiring a file would
invalidate the existing sessions and eight-file baselines. A live metadata
lookup would make the result depend on the day of replay.
[RFC 0007 D10][d10] · [Baseline comparison][baseline]

Add `--rules-file <path>` to `hl-backtest run` and an optional top-level
`rules_file` path to the paper TOML. Resolve a relative CLI path from the working
directory. The paper CLI has the TOML path, so resolve a relative rules
path there against the config file's directory. Both drivers read the file
once before the run, use the same codec parser and record the same rule
inputs. If the supplied file cannot be read or validated, stop before
running the strategy. Separate parsers or a metadata refresh during capture
could give paper and replay different rules. [Backtest CLI][backtest-cli] ·
[Paper config][paper-config]

### Rules file

Use UTF-8 JSON, which the Hyperliquid layer of `hl-codec` already parses.
This complete file uses a synthetic coin, times and asset ids.

```json
{
  "instruments": [
    {
      "network": "testnet",
      "coin": "SYNTH",
      "versions": [
        {"effective_from": "100", "sz_decimals": 2, "asset_id": "17"},
        {"effective_from": "200", "sz_decimals": 3, "asset_id": "21"}
      ]
    }
  ]
}
```

The example gives the exact field names at each level. Every field is
required once. Reject invalid JSON, extra or duplicate fields, wrong types
and empty arrays. Object field order and JSON whitespace carry no meaning.

| Field | Accepted input |
| --- | --- |
| `instruments` | Array of histories. Each pair of `network` and `coin` occurs once. |
| `network`, `coin` | Network string `mainnet` or `testnet` and the exact coin string. Reject empty or all-whitespace names. Preserve case, prefixes and every byte of the decoded name (D1). |
| `versions` | Array in strictly increasing `effective_from` order. Reject equal or decreasing starts in the supplied order. |
| `effective_from` | Signed 64-bit decimal integer string in UTC Unix epoch nanoseconds, inclusive. |
| `sz_decimals` | One JSON integer digit from 0 through 6, the range accepted by `RuleVersion::new`. |
| `asset_id` | Unsigned 64-bit decimal integer string for this version. |

**On 2026-11-27 the owner chose `sz_decimals` for the rules file and the
session's `rules` array.** Every field in both formats then uses snake_case.

Integer strings use ASCII digits, with a leading minus only for negative
values. Zero is `"0"`. Reject leading zeros, plus signs, fractions,
exponents, whitespace and out-of-range values. The codec validates the
whole file, derives each id with `instrument_id(network, coin)` and rejects
a collision between different names or networks. Use `RuleVersion::new`
for each version and `RuleHistory::new` for each pair. Select the run's
exact network and coin, retain its versions for the session and install
its history in a `RuleBook` under the derived id. A supplied file missing
that pair is a setup error naming the pair.
[Identity helper][id-tests] · [Rule histories][histories]

**Venue inputs.** Use the register's price, size and asset-id entries.
The register gives no dated history or effective start for a saved
`meta` response. The recording drivers supply no dated rules either.
[HL-23][hl23] · [HL-24][hl24] · [HL-25][hl25] · [HL-26][hl26] ·
[HL-53][hl53] · [HL-54][hl54]

Give the Hyperliquid codec an offline importer for a saved perpetual
`meta` response. Map the venue's `szDecimals` field to `sz_decimals`.
Supply the network, dex and one explicit start. Build one version for
each perpetual using the same validation as the file. For a
builder dex, require the caller's `perp_dex_index` for the HL-54 calculation.
The register gives no way to recover that index from the response alone.
Without it the import fails. The caller can instead supply explicit asset
ids in the rules file. Using the file date or the first market event as a
start would assert coverage the caller never supplied. [HL-26][hl26] ·
[HL-53][hl53] · [HL-54][hl54]

### Check a placement or modification at its effect time

**Our choice.** The simulator queues an action in `OrderGateway::submit`
for `A = now + order_latency`. When a subsequent BBO or trade first has
exchange time strictly greater than A, `prove_actions` applies the action
before the simulator uses that event. On proof, `RuleBook::at(id, A)`
selects the last version whose start is at or before A. A strategy check
at submission would miss a change during order latency. The proving event
can arrive with an exchange time in the next version. An action still
awaiting evidence at the stop stays pending. [Simulator][sim-code] ·
[RFC 0008 D2][paper]

With rules enabled, look up coverage before returning any ordinary
placement or modify rejection. Both `place` and `modify` already receive
`effective_at`. A placement uses `order.instrument`. A modify finds the
resting order by `cloid` and uses its instrument and side with the proposed
`px` and `qty`. If the target is missing, pass the modify action's named
instrument through to check coverage before returning `UnknownOrder`.

Once coverage is known, keep the existing duplicate-cloid and order-type
checks. Pass the selected version to the shared `check_post_only`, which
already checks positivity, notional and crossing. After the notional
check, call `check_price` then `check_size` on the submitted values before
testing for a cross. Reject an off-grid price or an over-precise size at A
with `InvalidOrder`, encoded as `invalid_order`. Rounding could make the
order cross. A rejected modify leaves the resting order intact. Cancels
need no price or size check. [RFC 0007 D10][d10] · [HL-27][hl27]

`RuleError::MissingCoverage { ts_exch, first_start }` stops the run. Carry
the instrument id with the error. Name the network and coin, and give A
and the first start in epoch nanoseconds. Using the first version early
would invent coverage. A rejection would let the strategy act on a data
error. Today `place`, `modify` and `check_post_only` can return only a
`RejectReason` on failure, and `SimGateway::on_market_event` returns unit.
Carry a separate run error through `apply`, `prove_actions` and
`on_market_event` to `hl-run`'s `IncrementalDriver`. Both drivers stop
without a rejection for that action or a successful session footer.
[Rule errors][rule-errors] · [Replay driver][run-code]

### Rejection reason

| Option | Effect on the order report and its readers |
| --- | --- |
| (a) Reuse `RejectReason::InvalidOrder` | `rejected` and `modify_rejected` already encode `invalid_order`. The shared enum and session vocabulary suffice. A reader must check the submitted values against the rules to tell which failed. |
| (b) Add price and size reasons to `hl-types` | Add `OffGridPrice` for the two price precision errors and `OverPreciseSize` for `SizeDecimals`. Encode them as `off_grid_price` and `over_precise_size` in `hl-run`'s report rows. Add the corresponding `hl-backtest` count keys when those reasons occur so the no-rules summary keeps its bytes. |

**On 2026-11-27 the owner chose option (a).** Reject an off-grid price or
an over-precise size with the existing `RejectReason::InvalidOrder`.
The initial recommendation was (b). A non-conforming quote exposes a defect
in the strategy's rounding. Fix it when first seen. The review material
said the report row already carried the submitted price and size.
The current session writer puts `px` and `qty` on the corresponding
`action` row. Under (b), counts for specific reasons would appear only
when those reasons occurred. Option (a) keeps `hl-types`, the session's
report vocabulary and the summary's count keys as they are. A later
decision can add specific reasons. [Reject reasons][reject] ·
[Session writer][session-writer] · [Summary counts][summary-code]

### Record the versions in the session

**Our choice.** At `start`, record the selected instrument's complete
version list in start-time order, including each asset id. A paper driver
cannot know which versions later orders will use. A path or hash alone
cannot rebuild the list if the file disappears. Use the network, coin and
instrument already in `start`.

`Session::new` writes `sim.model` followed by `SessionStart.sim`'s
ordered string pairs. `SimConfig::parameters` writes
`order_latency`, `report_latency`, `maker_fee` and `min_order_notional` as
`order_latency_ns`, `report_latency_ns`, `maker_fee_bps` and
`min_order_notional_usd`, respectively. Both the backtest CLI and
`PaperConfig::from_toml` use `SimConfig::from_parameters`, which accepts
exactly those four names. After checking the raw hashes, `--check-session`
rebuilds the simulator from the recorded parameters and any embedded versions.
The checker compares the regenerated start in canonical bytes and replays
the decisions. [Session format][session-file] ·
[Session writer][session-writer] · [Checker][checker] · [Backtest CLI][backtest-cli]

| Consequence | (a) Additional string parameters in `sim` | (b) A separate `start.rules` field |
| --- | --- | --- |
| Schema and contents | Keep `schema_version` 1. Append three string pairs per version. Write `rule_0_effective_from_ns`, `rule_0_sz_decimals`, `rule_0_asset_id` in that order, then repeat for index 1 and so on. | Write v2 when rules are supplied. Append `rules` after `sim` as a nonempty array of version objects, with the three fields in the order and types shown in the file example. Write v1 when rules are absent. |
| Component settings and rebuild | `SimConfig::parameters` lists every setting, including all versions. Extend `from_parameters` to accept the four alone (grid check off) or those four plus complete, consecutive triples from zero. Reject partial, duplicate, unknown or malformed parameters. Bind the rebuilt versions to `start.instrument`. The paper TOML still takes only the four scalar keys in `[sim]`. | Keep `SimConfig::parameters` and `from_parameters` at exactly four. Specify dated rules as a separate run input alongside the instrument. The component still lists every scalar setting. Extend `SessionStart`, the writer and `parse_start` for v2. Validate and install the array separately when rebuilding the simulator. |
| Sessions written before D7 | Read and regenerate the four-pair v1 form with the grid check off. An old parser would reject the extra pairs in a rules-enabled session. | Read and regenerate a v1 start without a `rules` field, with the grid check off. An old parser would reject v2. |
| Eight-file baseline | Without a file, use the four settings and emit the original summary bytes for both coins. | Without a file, install no history and emit the original summary bytes for both coins. |

**On 2026-11-27 the owner chose option (b).** The `start` row uses
`schema_version` 2 with a `rules` array after `sim` for a run with rules,
and version 1 for a run without rules. Write each object's `effective_from`,
`sz_decimals` and `asset_id` in that order, with the types shown in the example.
`SimConfig::parameters` and `SimConfig::from_parameters` keep exactly four
parameters. The version list is a table. Encoding it as scalar simulator
settings would make every reader join the indexed parameter names back
together. Document both layouts and their replay checks in the session file
reference when implementing them.

`--check-session` takes only the session path and rebuilds the rules from
`start`. Regenerate the start from the settings actually installed. The
existing build-identity check also applies to old sessions. An untouched
session passes with its recorded commit and matching `dirty` flag. A binary
built at a different commit stops before replay.
[RFC 0008 D8][paper] · [Writing and checking][session-check]

### Implementation issues, in order

| Issue | Crates and work | Owner review |
| --- | --- | --- |
| 1. Read dated histories (implemented) | `hl-codec` validates the rules file with `hl-rules` and returns the selected pair's instrument id, ordered versions and `RuleBook`. | No special review. |
| 2. Check orders on proof (implemented) | `hl-sim` applies the checks and returns `InvalidOrder` for an off-grid price or an over-precise size. `hl-run` propagates a coverage error. | Yes (`hl-sim`). |
| 3. Record and rebuild the history (implemented) | `hl-run` writes schema version 2 with `start.rules` and `hl-backtest` rebuilds the history from it. Runs without rules, including paper, use the v1 layout. [Session format][session-file] | No special review. Both `hl-sim` parameter functions stay as written. |
| 4. Accept the file in both drivers (implemented) | `hl-backtest run --rules-file` and the paper TOML's `rules_file` load the selected history. Both drivers report when they run without a file. [Usage](../usage.md#dated-price-and-size-rules) | No special review. |
| 5. Import a saved `meta` response | The offline importer in `hl-codec` first needs a register entry for the `meta` response layout and how it names builder-dex coins. [Hyperliquid venue register](../venues/hyperliquid.md) | No special review. |

RFC 0007 D9's one-tick measurement will use `RuleVersion::next_price_above`.
Measuring time at a spread needs BBO intervals as well as rules. It is
later work. [RFC 0007 D9][fills]

### Tests the implementation must pass

- Parse the example, select the two versions and their asset ids, and keep
  histories for names on different networks distinct. Exercise the invalid
  file cases above. Convert synthetic saved metadata with an explicit start
  and compare the result to the equivalent file. A missing builder dex
  index fails the import.
- With rules, use otherwise valid placements and modifies to test an
  off-grid price and an over-precise size independently. Check `InvalidOrder`
  in the simulator and `invalid_order` in the `rejected` and `modify_rejected`
  reports with exchange time A, including when the order would also cross.
  After a rejected modify the resting order can still fill at its old price
  and size.
- In the example, submit at different times so that two orders for
  `1.001` at price `100` take effect at 199 and 200 ns. Prove both with
  the same later BBO. The first gets `invalid_order` for its size. The
  second passes. An event with exchange time equal to A cannot prove the
  action.
- Give an order an effect time of 99 ns and prove it with a later trade.
  Against the example file, both drivers stop with
  the instrument, 99 ns and 100 ns in the error, even for an unsupported
  placement or a modify with an unknown `cloid`. The order produces no
  rejection and the run produces no successful footer.
- Without a file, repeat both D6 eight-file runs (BTC and `xyz:NVDA`).
  Compare complete JSON stdout byte for byte to each pre-change baseline
  and check stderr for the no-grid message. Check an untouched pre-change
  session with `--check-session` at its recorded build. Write a session
  without rules using the legacy fixture's inputs and `BuildIdentity`.
  Check `schema_version` 1 and compare the session byte for byte with the
  fixture. Replay it with the new reader and keep the build-mismatch test.
  [Baseline comparison][baseline]
- With rules, write a backtest session and a paper session using the same
  history. Check `schema_version` 2 and the `rules` array after `sim` in
  each `start` row. Remove the rules file. Both sessions must pass
  `--check-session` with the embedded versions, regenerated reports and end
  rows. Check an open paper checkpoint the same way. Reject a malformed
  embedded history.

[d7]: #d7-supply-dated-rules-to-backtest-and-paper-runs
[d10]: 0007-fill-models.md#d10-validate-new-orders-at-arrival
[histories]: ../../crates/hl-rules/src/history.rs
[rule-errors]: ../../crates/hl-rules/src/error.rs
[sim-code]: ../../crates/hl-sim/src/lib.rs
[run-code]: ../../crates/hl-run/src/lib.rs
[reject]: ../../crates/hl-types/src/event.rs
[backtest-cli]: ../../crates/hl-backtest/src/main.rs
[summary-code]: ../../crates/hl-backtest/src/summary.rs
[paper-config]: ../../crates/hl-live/src/config.rs
[session-file]: ../session-file.md
[session-check]: ../session-file.md#writing-and-checking
[session-writer]: ../../crates/hl-run/src/session.rs
[checker]: ../../crates/hl-backtest/src/session.rs
[id-tests]: ../../crates/hl-codec/src/hyperliquid/identity.rs
[schema]: ../../schema/0001_init.sql
[db]: ../../crates/hl-loader/src/db.rs
[loading]: 0004-clickhouse-schema-and-loading.md
[decoding]: 0003-hyperliquid-market-data-decoding.md
[time]: 0006-event-and-time-model.md
[fills]: 0007-fill-models.md
[paper]: 0008-paper-trading.md
[baseline]: #d6-implement-after-both-paper-trading-changes-merge
[hl23]: ../venues/hyperliquid.md#hl-23
[hl24]: ../venues/hyperliquid.md#hl-24
[hl25]: ../venues/hyperliquid.md#hl-25
[hl26]: ../venues/hyperliquid.md#hl-26
[hl27]: ../venues/hyperliquid.md#hl-27
[hl40]: ../venues/hyperliquid.md#hl-40
[hl53]: ../venues/hyperliquid.md#hl-53
[hl54]: ../venues/hyperliquid.md#hl-54
[hl55]: ../venues/hyperliquid.md#hl-55
[hl81]: ../venues/hyperliquid.md#hl-81
[hl83]: ../venues/hyperliquid.md#hl-83
[hl110]: ../venues/hyperliquid.md#hl-110
[hl137]: ../venues/hyperliquid.md#hl-137
[bn9]: ../venues/binance.md#bn-9
[bn18]: ../venues/binance.md#bn-18
