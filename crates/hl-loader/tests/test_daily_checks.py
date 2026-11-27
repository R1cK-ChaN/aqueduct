"""Opt-in daily-check CLI tests with synthetic rows and a disposable database.

Build and run from the repository root.

    cargo build -p hl-loader
    HLMM_TEST_CLICKHOUSE=1 python3 -I crates/hl-loader/tests/test_daily_checks.py

The shared harness uses the already installed ClickHouse 26.8 image, creates
its own loopback endpoint and credentials, and removes its container and
volumes even when setup fails. Every row in this file is synthetic.
"""

from datetime import datetime, timedelta, timezone
import hashlib
import json
from pathlib import Path
import runpy
import unittest


# -I omits the script directory from sys.path. Load the sibling explicitly.
DisposableClickHouseTest = runpy.run_path(
    str(Path(__file__).with_name("clickhouse_test.py"))
)["DisposableClickHouseTest"]
DAY = "2026-06-01"
DAY_START = datetime(2026, 6, 1, tzinfo=timezone.utc)
INSTRUMENT = 7
TABLES = (
    "l2_snapshot", "fills", "bbo", "ws_trades", "instruments",
    "ingest_log", "rawlog_state", "ingest_conflict", "dq_hourly", "dq_checks",
)


def timestamp(seconds):
    return (DAY_START + timedelta(seconds=seconds)).strftime("%Y-%m-%d %H:%M:%S.%f")[:-3]


def digest(file):
    return hashlib.sha256(file.encode()).hexdigest()


class DailyChecksTest(DisposableClickHouseTest):
    def setUp(self):
        self.clear_data()

    def clear_data(self):
        for table in TABLES:
            self.sql(f"TRUNCATE TABLE hl.{table}")

    def insert(self, table, rows):
        self.sql(
            f"INSERT INTO hl.{table} FORMAT JSONEachRow\n"
            + "\n".join(json.dumps(row) for row in rows)
        )

    def ingest(
        self, ingest_id, file, *, source="ws_public", status="done",
        first=0, last=86399, finished=None, counters=None,
    ):
        self.insert("ingest_log", [{
            "ingest_id": ingest_id,
            "source": source,
            # Deliberately reuse a name across distinct files.
            "file_name": "0.hlrec",
            "sha256": digest(file),
            "ts_min": timestamp(first),
            "ts_max": timestamp(last),
            "finished_at": timestamp(ingest_id if finished is None else finished),
            "status": status,
            "details": json.dumps({"counters": counters or {}}),
        }])

    def conflict(self, ingest_ids, *, table="l2_snapshot", at=100):
        self.insert("ingest_conflict", [{
            "ingest_id": ingest_ids[-1],
            "table_name": table,
            "instrument_id": INSTRUMENT,
            "ts_exch": timestamp(at),
            "row_key": f"synthetic-{at}",
            "ingest_ids": ingest_ids,
            "row_hashes": [111, 222],
        }])

    def recording(
        self, ingest_id, file, trades, *, run=None, file_seq=0,
        instrument=INSTRUMENT,
    ):
        self.ingest(
            ingest_id, file,
            first=min(trade[0] for trade in trades),
            last=max(trade[0] for trade in trades),
        )
        if run is not None:
            source, process_start = run
            self.insert("rawlog_state", [{
                "source": source,
                "process_start_wall_ns": process_start,
                "file_seq": file_seq,
                "sha256": digest(file),
                "state_json": "{}",
                "ingest_id": ingest_id,
            }])
        self.insert("ws_trades", [{
            "instrument_id": instrument,
            "source": "ws_public",
            "ts_exch": timestamp(at),
            "ts_recv": timestamp(at + 1),
            "tid": tid,
            "aggressor": "B",
            "px": "100",
            "sz": "1",
            "live": live,
            "ingest_id": ingest_id,
        } for at, tid, live in trades])

    def fills(self, trades, *, instrument=INSTRUMENT):
        self.insert("fills", [{
            "instrument_id": instrument,
            "source": "node_fills",
            "ts_exch": timestamp(at),
            "tid": tid,
            "side": side,
            "crossed": side == "B",
            "px": "100",
            "sz": "1",
            "ingest_id": 900,
        } for at, tid in trades for side in ("B", "A")])

    def fills_coverage(self):
        self.ingest(900, "fills", source="node_fills")

    def check_day(self, *, expected_code=0):
        result = self.run_cli("check", DAY, expected_code=expected_code)
        self.output = result.stdout
        self.results = self.rows(
            "SELECT check_name, subject, instrument_id, compared, violations, "
            "passed, max_rate, detail FROM hl.dq_checks FINAL "
            f"WHERE day = '{DAY}' ORDER BY check_name, subject, instrument_id"
        )
        self.assertIn(f"data-quality checks for {DAY} (UTC)", self.output)
        if expected_code == 0:
            self.assertIn("all alarm checks passed", self.output)
        else:
            self.assertIn("alarms:", self.output)
            self.assertIn("one or more data-quality checks failed", result.stderr)

    def assert_check(
        self, name, compared, violations, *, subject, instrument=0, passed=True,
    ):
        rows = [
            row for row in self.results
            if row["check_name"] == name and row["subject"] == subject
            and row["instrument_id"] == instrument
        ]
        self.assertEqual(len(rows), 1, (name, subject, instrument, self.results))
        row = rows[0]
        self.assertEqual(int(row["compared"]), compared, row)
        self.assertEqual(int(row["violations"]), violations, row)
        self.assertEqual(bool(row["passed"]), passed, row)
        verdict = "pass" if passed else "FAIL"
        self.assertRegex(self.output, rf"(?m)^{verdict}\s+{name}\s+{subject}\s")
        if row["detail"] and row["detail"] != "{}":
            self.assertIn(row["detail"], self.output)
        return json.loads(row["detail"] or "{}")

    def assert_recorded_fills(self, compared, violations, *, instrument=INSTRUMENT):
        self.assert_check(
            "fills_vs_ws_trades", compared, violations, subject="node_fills",
            instrument=instrument, passed=violations <= compared * 0.001,
        )
        row = next(
            row for row in self.results
            if row["check_name"] == "fills_vs_ws_trades"
            and row["instrument_id"] == instrument
        )
        self.assertEqual(row["max_rate"], 0.001)

    def test_latest_completed_load_replaces_errors_and_detail_counters(self):
        latest = {
            "truncated": 1,
            "disconnects": 2,
            "frames_dropped_by_recorder": 3,
            "diffs_without_snapshot": 4,
            "spot_rows_skipped": 5,
            "outcome_rows_skipped": 6,
        }
        self.ingest(10, "reloaded", counters={"book_errors": 2, "truncated": 20})
        self.ingest(20, "reloaded", counters=latest)
        # A failed retry cannot replace a completed load or contribute detail.
        self.ingest(
            30, "reloaded", status="failed",
            counters={"book_errors": 50, "disconnects": 50},
        )
        self.check_day()
        detail = self.assert_check("load_problems", 1, 0, subject="ws_public")
        self.assertEqual({key: int(value) for key, value in detail.items()}, latest)
        self.assertEqual(int(self.sql("SELECT count() FROM hl.ingest_log")), 3)

    def test_latest_load_orders_by_completion_time_then_ingest_id(self):
        self.ingest(200, "completion-order", finished=10, counters={"book_errors": 1})
        self.ingest(100, "completion-order", finished=20)
        self.ingest(300, "completion-tie", finished=30, counters={"decode_errors": 1})
        self.ingest(301, "completion-tie", finished=30)
        self.check_day()
        self.assert_check("load_problems", 2, 0, subject="ws_public")

    def test_latest_completed_load_is_selected_before_day_filter(self):
        self.ingest(10, "moved-span", first=100, last=200, counters={"book_errors": 1})
        self.ingest(20, "moved-span", first=86400, last=86500)
        self.ingest(30, "other-day-failure", status="failed", first=86400, last=86500)
        self.check_day()
        self.assertFalse(any(row["check_name"] == "load_problems" for row in self.results))
        self.assertRegex(self.output, r"(?m)^skip\s+load_problems\s+nothing to check$")

    def test_each_latest_error_counter_alarms(self):
        for index, counter in enumerate(("book_errors", "decode_errors", "block_gaps")):
            self.ingest(index * 10 + 1, counter)
            self.ingest(index * 10 + 2, counter, counters={counter: 1})
        self.check_day(expected_code=1)
        self.assert_check("load_problems", 3, 3, subject="ws_public", passed=False)

    def test_failed_files_count_once_per_source_and_pass_after_success(self):
        self.ingest(10, "failed", status="failed", counters={"book_errors": 10})
        self.ingest(20, "failed", status="failed", counters={"truncated": 10})
        self.ingest(30, "failed", source="node_fills")
        self.check_day(expected_code=1)
        detail = self.assert_check("load_problems", 1, 1, subject="ws_public", passed=False)
        self.assertEqual(detail, {})
        self.assert_check("load_problems", 1, 0, subject="node_fills")

        self.ingest(40, "failed")
        self.check_day()
        self.assert_check("load_problems", 1, 0, subject="ws_public")
        self.assert_check("load_problems", 1, 0, subject="node_fills")
        self.assertEqual(int(self.sql("SELECT count() FROM hl.ingest_log")), 4)

    def test_reload_conflicts_are_reported_and_preserved(self):
        self.ingest(10, "same-file")
        self.ingest(20, "same-file")
        self.conflict([10, 20])
        self.conflict([10, 20], table="bbo", at=200)
        self.check_day()
        detail = self.assert_check("key_conflicts", 2, 0, subject="all tables")
        self.assertEqual(int(detail["changed_by_reload"]), 2)
        self.assertIn("l2_snapshot", detail["tables"])
        self.assertIn("bbo", detail["tables"])
        self.assertEqual(int(self.sql("SELECT count() FROM hl.ingest_conflict")), 2)

    def test_single_ingest_conflicts_alarm(self):
        for ingest_ids in ([10], [10, 10]):
            with self.subTest(ingest_ids=ingest_ids):
                self.clear_data()
                self.ingest(10, "single-load")
                self.conflict(ingest_ids)
                self.check_day(expected_code=1)
                detail = self.assert_check(
                    "key_conflicts", 1, 1, subject="all tables", passed=False,
                )
                self.assertNotIn("changed_by_reload", detail)

    def test_different_file_and_unknown_ingest_conflicts_alarm(self):
        self.ingest(10, "first-file")
        self.ingest(20, "second-file")
        self.conflict([10, 20])
        self.conflict([10, 999], at=200)
        self.check_day(expected_code=1)
        detail = self.assert_check("key_conflicts", 2, 2, subject="all tables", passed=False)
        self.assertNotIn("changed_by_reload", detail)

        self.ingest(30, "first-file")
        self.conflict([10, 30], at=300)
        self.check_day(expected_code=1)
        detail = self.assert_check("key_conflicts", 3, 2, subject="all tables", passed=False)
        self.assertEqual(detail["changed_by_reload"], 1)

    def test_disjoint_runs_use_both_run_key_fields_and_instrument(self):
        first_run = ("site-a", 1000)
        for second_run in (("site-a", 2000), ("site-b", 1000)):
            with self.subTest(second_run=second_run):
                self.clear_data()
                early = [(100, 1, 1), (150, 2, 1), (200, 3, 1)]
                late = [(4000, 4, 1), (4050, 5, 1), (4100, 6, 1)]
                self.recording(10, "early", early, run=first_run)
                self.recording(20, "late", late, run=second_run)
                self.recording(30, "other-coin", [(1000, 7, 1), (1100, 8, 1)],
                               run=first_run, file_seq=1, instrument=8)
                self.fills([(at, tid) for at, tid, _ in early + late])
                self.fills([(50, 20), (1050, 21), (3999, 22), (4190, 23)])
                self.fills([(1000, 7), (1100, 8)], instrument=8)
                self.fills_coverage()
                self.check_day()
                self.assert_recorded_fills(6, 0)
                self.assert_recorded_fills(2, 0, instrument=8)
                self.assert_check("ws_trades_vs_fills", 6, 0,
                                  subject="ws_public", instrument=INSTRUMENT)

    def test_hourly_rotation_keeps_the_run_span_continuous(self):
        run = ("site-a", 1000)
        self.recording(10, "hour-zero", [(3550, 1, 1), (3590, 2, 1)], run=run)
        self.recording(20, "hour-one", [(3610, 3, 1), (3650, 4, 1)], run=run, file_seq=1)
        # A trade with the missing fill's ID at a different time cannot match.
        self.fills([(3550, 1), (3590, 2), (3600, 2), (3610, 3), (3650, 4)])
        self.fills_coverage()
        self.check_day(expected_code=1)
        self.assert_recorded_fills(5, 1)
        self.assert_check("ws_trades_vs_fills", 4, 0, subject="ws_public", instrument=INSTRUMENT)

    def test_legacy_files_each_have_their_own_recorded_span(self):
        self.recording(10, "legacy-first", [(100, 1, 1), (200, 2, 1)])
        self.recording(20, "legacy-second", [(4000, 3, 1), (4100, 4, 1)])
        self.fills([(50, 5), (100, 1), (200, 2), (1000, 6), (4000, 3), (4100, 4), (4200, 7)])
        self.fills_coverage()
        self.check_day()
        self.assert_recorded_fills(4, 0)

    def test_overlapping_run_spans_compare_each_fill_once(self):
        self.recording(10, "overlap-first", [(100, 1, 1), (150, 2, 1), (300, 3, 1)],
                       run=("site-a", 1000))
        self.recording(20, "overlap-second", [(200, 4, 1), (250, 5, 1), (400, 6, 1)],
                       run=("site-a", 2000))
        self.fills([(100, 1), (150, 2), (200, 4), (225, 7), (250, 5), (300, 3), (400, 6)])
        self.fills_coverage()
        self.check_day(expected_code=1)
        self.assert_recorded_fills(7, 1)

    def test_replay_matches_fills_without_extending_live_span(self):
        self.recording(10, "with-replay", [
            (50, 1, 0), (100, 2, 1), (150, 3, 0), (200, 4, 1), (250, 5, 0),
        ], run=("site-a", 1000))
        self.recording(20, "only-replay", [(25, 6, 0), (500, 7, 0)],
                       run=("site-a", 2000))
        self.fills([(50, 1), (100, 2), (150, 3), (200, 4), (250, 5), (25, 6), (500, 7)])
        self.fills_coverage()
        self.check_day()
        self.assert_recorded_fills(3, 0)
        self.assert_check("ws_trades_vs_fills", 7, 0, subject="ws_public", instrument=INSTRUMENT)

        self.fills([(175, 8)])
        self.check_day(expected_code=1)
        self.assert_recorded_fills(4, 1)

    def test_earlier_load_trades_keep_the_file_run_after_reload(self):
        for run in (("site-a", 1000), None):
            with self.subTest(run=run):
                self.clear_data()
                self.recording(10, "reloaded-recording", [(100, 1, 1)], run=run)
                self.recording(20, "reloaded-recording", [(200, 2, 1)], run=run)
                # The first trade still refers to ingest 10. State FINAL, when
                # present, has been replaced by ingest 20 for the same file.
                if run is not None:
                    states = self.rows("SELECT ingest_id FROM hl.rawlog_state FINAL")
                    self.assertEqual([int(row["ingest_id"]) for row in states], [20])
                self.fills([(100, 1), (150, 3), (200, 2)])
                self.fills_coverage()
                self.check_day(expected_code=1)
                self.assert_recorded_fills(3, 1)


if __name__ == "__main__":
    unittest.main(verbosity=2)
