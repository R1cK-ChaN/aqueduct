#!/usr/bin/env python3
"""Measure weekly roster churn from a Hyperliquid daily candle snapshot."""

import argparse
from collections import Counter
from datetime import UTC, date, datetime, timedelta
from decimal import Decimal
from itertools import pairwise, product
import json
from pathlib import Path
import statistics
from tempfile import NamedTemporaryFile
import time
from typing import NamedTuple
import urllib.error
import urllib.request


DAY = timedelta(days=1)
WEEK = timedelta(weeks=1)
TOP = 30


class Candle(NamedTuple):
    volume: Decimal
    notional: Decimal


def fetch_candles(raw: Path, start_ms: int, end_ms: int) -> None:
    """Collect missing files for the given range using the saved meta response."""
    if end_ms < start_ms:
        raise ValueError("end_ms precedes start_ms")
    universe = json.loads((raw / "meta.json").read_bytes())["universe"]
    for index, asset in enumerate(universe):
        coin = asset["name"]
        path = raw / f"candles_{index:03d}.json"
        if path.exists():
            continue
        request = urllib.request.Request(
            "https://api.hyperliquid.xyz/info",
            data=json.dumps({"type": "candleSnapshot", "req": {
                "coin": coin, "interval": "1d", "startTime": start_ms, "endTime": end_ms,
            }}).encode(),
            headers={"Content-Type": "application/json"},
        )
        for attempt in range(5):
            try:
                with urllib.request.urlopen(request, timeout=30) as response:
                    candles = json.load(response)
                break
            except urllib.error.HTTPError as error:
                if error.code != 429 or attempt == 4:
                    raise
                time.sleep(10 * (attempt + 1))
        with NamedTemporaryFile(mode="w", dir=raw, delete=False) as temp:
            json.dump({"coin": coin, "candles": candles}, temp)
        Path(temp.name).replace(path)
        time.sleep(1.5)


def read_snapshot(raw: Path):
    """Use the meta index, including delisted assets and empty candle files."""
    universe = json.loads((raw / "meta.json").read_bytes())["universe"]
    daily, delisted = {}, set()
    for index, asset in enumerate(universe):
        coin = asset["name"]
        path = raw / f"candles_{index:03d}.json"
        record = json.loads(path.read_bytes())
        if record["coin"] != coin or coin in daily:
            raise ValueError(f"{path.name} does not match the meta index")
        per_day = {}
        for row in record["candles"]:
            day = datetime.fromtimestamp(row["t"] // 1000, UTC).date()
            if day in per_day or row["s"] != coin or row["i"] != "1d":
                raise ValueError(f"{path.name} has a duplicate day or unexpected candle")
            volume = Decimal(row["v"])
            price = (Decimal(row["h"]) + Decimal(row["l"]) + Decimal(row["c"])) / 3
            per_day[day] = Candle(volume, volume * price)
        daily[coin] = per_day
        if asset.get("isDelisted"):
            delisted.add(coin)
    return daily, delisted


def weekly_rankings(daily: dict[str, dict[date, Candle]]):
    days = {day for series in daily.values() for day in series}
    if not days:
        raise ValueError("snapshot contains no candles")
    first, last = min(days), max(days)
    monday = first + timedelta(days=(-first.weekday()) % 7) + WEEK
    weeks = {}
    while monday - DAY <= last:
        window = [monday - offset * DAY for offset in range(7, 0, -1)]
        scores = {}
        for coin, series in daily.items():
            if all(day in series for day in window) and series[window[-1]].volume > 0:
                scores[coin] = sum(series[day].notional for day in window)
        order = sorted(scores, key=lambda coin: (-scores[coin], coin))
        weeks[monday] = {coin: rank for rank, coin in enumerate(order, start=1)}
        monday += WEEK
    if not weeks:
        raise ValueError("snapshot contains no complete Monday-to-Sunday window")
    return weeks


def hysteresis_rosters(rankings, exit_rank, miss_limit, min_tenure):
    """Count weeks already served before allowing removal at the next refresh."""
    joined, misses = {}, Counter()
    history = []
    for week, ranks in enumerate(rankings):
        for coin in list(joined):
            if coin not in ranks or ranks[coin] > exit_rank:
                misses[coin] += 1
                if misses[coin] >= miss_limit and week - joined[coin] >= min_tenure:
                    del joined[coin], misses[coin]
            else:
                misses[coin] = 0
        for coin, rank in ranks.items():
            if rank <= TOP:
                joined.setdefault(coin, week)
        history.append(set(joined))
    return history


def coverage(rosters):
    stints = Counter(rosters[0])
    adds, changed = [], 0
    for previous, current in pairwise(rosters):
        entrants = current - previous
        stints.update(entrants)
        adds.append(len(entrants))
        changed += current != previous
    return {
        "transitions": len(adds),
        "entrants": sum(adds),
        "entrants_per_transition": round(statistics.mean(adds), 2) if adds else None,
        "changed_transitions": changed,
        "coins_ever": len(stints),
        "coins_with_interruptions": sum(count > 1 for count in stints.values()),
        "max_stints": max(stints.values(), default=0),
        "peak_coins": max(map(len, rosters)),
        "final_coins": len(rosters[-1]),
    }


def analyse(raw: Path):
    daily, delisted = read_snapshot(raw)
    weeks = weekly_rankings(daily)
    rankings = list(weeks.values())
    top30 = [{coin for coin, rank in week.items() if rank <= TOP} for week in rankings]
    top15 = [{coin for coin, rank in week.items() if rank <= 15} for week in rankings]
    previous_ranks = []
    for ranks, (previous, current) in zip(rankings, pairwise(top30)):
        previous_ranks.extend(ranks[coin] for coin in current - previous if coin in ranks)

    monthly = {}
    for monday, roster in zip(weeks, top30):
        monthly.setdefault(monday.replace(day=1), roster)
    retained = set()
    while_eligible = []
    for ranks, roster in zip(rankings, top30):
        retained = (retained & ranks.keys()) | roster
        while_eligible.append(retained)

    stable = set.intersection(*top30)
    start, stop = min(weeks) - WEEK, max(weeks)
    totals = {
        coin: sum(candle.notional for day, candle in series.items() if start <= day < stop)
        for coin, series in daily.items()
    }
    stable_total = sum(value for coin, value in totals.items() if coin in stable)
    total = sum(totals.values())
    active_total = sum(value for coin, value in totals.items() if coin not in delisted)
    active_stable = sum(totals[coin] for coin in sorted(stable - delisted))
    strict = coverage(top30)
    return {
        "roster_weeks": len(weeks), "first_roster": str(min(weeks)), "last_roster": str(stop),
        "eligible_coins_min_max": [min(map(len, rankings)), max(map(len, rankings))],
        "strict_top_30": strict, "strict_top_15": coverage(top15),
        "entrants_with_previous_rank": len(previous_ranks),
        "median_previous_rank": statistics.median(previous_ranks) if previous_ranks else None,
        "hysteresis": [
            {"exit_rank": rank, "weeks_outside": misses, "min_tenure": tenure,
             **coverage(hysteresis_rosters(rankings, rank, misses, tenure))}
            for rank, misses, tenure in product((40, 45, 50), (1, 2, 3), (0, 4, 8))
        ],
        "monthly_top_30": coverage(list(monthly.values())),
        "never_remove": strict["coins_ever"],
        "retain_while_eligible": coverage(while_eligible),
        "stable_coins": sorted(stable),
        "notional_start": str(start), "notional_end": str(stop - DAY),
        "notional_days": (stop - start).days,
        "stable_notional": round(stable_total), "all_notional": round(total),
        "stable_share_pct": format(100 * stable_total / total, ".4f") if total else None,
        "share_excluding_delisted_pct": (
            format(100 * active_stable / active_total, ".4f") if active_total else None
        ),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    report = commands.add_parser("analyse", help="print aggregate results from local files")
    report.add_argument("snapshot", type=Path, help="directory containing raw/")
    fetch = commands.add_parser("fetch", help="fetch missing candles for a saved meta snapshot")
    fetch.add_argument("snapshot", type=Path, help="directory containing raw/meta.json")
    fetch.add_argument("start_ms", type=int, help="first UTC timestamp in milliseconds")
    fetch.add_argument("end_ms", type=int, help="last UTC timestamp in milliseconds")
    args = parser.parse_args()
    if args.command == "analyse":
        print(json.dumps(analyse(args.snapshot / "raw"), indent=2))
    else:
        fetch_candles(args.snapshot / "raw", args.start_ms, args.end_ms)


if __name__ == "__main__":
    main()
