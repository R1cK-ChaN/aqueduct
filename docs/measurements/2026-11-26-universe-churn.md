# Public dataset universe churn, 2026-11-26

How often would a liquidity-ranked roster interrupt a coin's recording
history? Which main-dex perpetuals stayed in the top 30 throughout the sample?

Redacted on 2026-11-27 to remove the collector's role and the original script names.

## Inputs

On 2026-11-26 a `{"type":"meta"}` request to Hyperliquid's public info
endpoint (`https://api.hyperliquid.xyz/info`) returned the main-dex
universe. The response in `raw/meta.json` contained 235 perpetuals, including
56 marked delisted. Each `candleSnapshot` request used the coin's name and
interval `1d` for 2026-05-11 through 2026-11-24 UTC. Each
`raw/candles_NNN.json` holds the coin name and the returned candles, indexed
by the coin's zero-based position in `meta.json`. Of the 235 files, 188
contained candles. The collection site was unrecorded.

The fetch and analysis steps of `research/universe_churn.py` use the saved
metadata. To reproduce the results offline, run the analysis outside
the checkout and pass the directory containing `raw/`. [Research script](../../research/universe_churn.py)

```sh
python3 -I /path/to/research/universe_churn.py analyse /path/to/snapshot
```

## Method

Approximate each UTC day's traded notional as
`volume × (high + low + close) / 3`. Each Monday, rank coins by the sum for
the previous seven complete UTC days, highest first. Break ties by coin
name. A coin needs a candle on all seven days and positive volume on
Sunday. Between 175 and 183 coins qualified per week.

The 28 weekly rosters ran from 2026-05-18 through 2026-11-23. Count an
entry whenever a coin joins or rejoins across one of the 27 transitions.
A stint is an unbroken run of weeks on the roster. A coin has interrupted
coverage if it leaves and later returns.

## Results

The strict top 30 changed on all 27 transitions, with 110 entries
(4.07 per week). It selected 71 distinct coins. Of these, 31 had
interrupted coverage. The maximum was six stints for a single coin.
The median previous-week rank was 38 across the 106 entries with a prior
rank. The strict top 15 averaged 2.63 entries per week.

The delayed-exit rules admitted a top-30 coin immediately. A coin could
leave after a minimum tenure of four or eight weeks once it had spent two
or three consecutive weeks outside the top 45 or 50. An ineligible week
counted towards exit. Across those eight rules, 5–11 coins had interrupted
coverage and peak roster size ranged from 47 to 54. A peak of 46 applied
when the top-45 rule required two misses and had no minimum tenure.

Refreshing on the first sampled Monday of each calendar month, starting
on 2026-05-18, left 14 coins with interrupted coverage. Keeping every coin
ever admitted grew the roster to 71. A variant that removed coins only on
loss of eligibility peaked and finished at 70.

The 14 coins in the strict top 30 every week were BTC, ETH, SOL, HYPE,
XRP, ZEC, DOGE, SUI, AAVE, NEAR, TAO, PUMP, XPL and kPEPE. They accounted
for **92.3%** of approximate main-dex notional across the 196 ranked days
from 2026-05-11 through 2026-11-22. The denominator includes every
available candle in that window. Excluding coins marked delisted at the
fetch date gives 92.4%.

## Limits

The candle formula uses one price per day and can change the order of
coins near a cutoff. The fixed 14 were identified using all 28 weeks.
Their share describes this sample. Roster membership measures gaps caused
by selection alone. Recorder uptime and feed gaps also affect actual
coverage.

## Register entries that cite this note

- [HL-148](../venues/hyperliquid.md#hl-148)
