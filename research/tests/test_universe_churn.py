"""Exercise the churn measurement with synthetic main-dex candle files."""

from datetime import UTC, datetime, timedelta
import json
from pathlib import Path
import runpy
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "universe_churn.py"


class UniverseChurnTest(unittest.TestCase):
    def test_eligibility_coverage_and_share(self):
        start = datetime(2026, 5, 11, tzinfo=UTC)
        anchors = [f"CORE{i:02d}" for i in range(29)]
        volumes = {coin: [100] * 21 for coin in anchors}
        volumes["A"] = [10] * 7 + [5] * 7 + [10] * 7
        volumes["B"] = [10] * 7 + [11] * 7 + [10] * 6 + [0]
        # Missing days and a zero final day each rule this coin out.
        volumes["PARTIAL"] = ([None] + [500] * 6) * 2 + [500] * 6 + [0]

        with tempfile.TemporaryDirectory() as tmp:
            raw = Path(tmp) / "raw"
            raw.mkdir()
            meta = [{"name": coin, "isDelisted": coin == "B"} for coin in volumes]
            (raw / "meta.json").write_text(json.dumps({"universe": meta}))
            for index, (coin, days) in enumerate(volumes.items()):
                candles = []
                # The trailing Monday falls outside the three complete weeks.
                for offset, volume in enumerate([*days, 999_999]):
                    if volume is not None:
                        day = start + timedelta(days=offset)
                        candles.append({"s": coin, "i": "1d", "t": int(day.timestamp()) * 1000,
                                        "v": str(volume), "h": "4", "l": "1", "c": "1"})
                (raw / f"candles_{index:03d}.json").write_text(
                    json.dumps({"coin": coin, "candles": candles})
                )
            result = subprocess.run(
                [sys.executable, "-I", str(SCRIPT), "analyse", tmp], cwd=tmp,
                capture_output=True, text=True, check=True,
            )
        report = json.loads(result.stdout)
        self.assertEqual((report["first_roster"], report["last_roster"]),
                         ("2026-05-18", "2026-06-01"))
        self.assertEqual(report["eligible_coins_min_max"], [30, 31])
        self.assertEqual(report["stable_coins"], anchors)
        strict = report["strict_top_30"]
        self.assertEqual(strict["entrants_per_transition"], 1)
        self.assertEqual(strict["coins_with_interruptions"], 1)
        self.assertEqual(strict["max_stints"], 2)
        self.assertEqual(report["median_previous_rank"], 31)
        self.assertEqual(report["monthly_top_30"]["changed_transitions"], 0)
        self.assertEqual(report["never_remove"], 31)
        self.assertEqual(report["retain_while_eligible"]["final_coins"], 30)
        self.assertEqual(report["notional_days"], 21)
        self.assertEqual(report["stable_notional"], 121_800)
        self.assertEqual(report["all_notional"], 140_564)
        self.assertEqual(report["stable_share_pct"], "86.6509")
        self.assertEqual(report["share_excluding_delisted_pct"], "86.9069")

    def test_exit_waits_for_tenure_and_consecutive_misses(self):
        simulate = runpy.run_path(str(SCRIPT))["hysteresis_rosters"]
        ranks = [{"A": rank} for rank in [30, 51, 51, 50, 51, 51, 31, 30]]
        rosters = simulate(ranks, exit_rank=50, miss_limit=2, min_tenure=4)
        self.assertEqual(["A" in roster for roster in rosters],
                         [True, True, True, True, True, False, False, True])


if __name__ == "__main__":
    unittest.main()
