"""Measure synthetic RFC 0002 connections through the isolated command line."""

import hashlib
import json
import struct
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "connection_lifetimes.py"
SECOND = 1_000_000_000
BASE = 1_735_689_600 * SECOND  # 2025-01-01T00:00:00Z
EXPIRED = "closed by the server: 1000 Expired"


def record(kind, payload, wall_s, mono_s=None):
    if not isinstance(payload, str):
        payload = json.dumps(payload)
    payload = payload.encode("utf-8")
    mono_s = wall_s if mono_s is None else mono_s
    return struct.pack("<IBqQ", len(payload), kind, BASE + wall_s * SECOND, mono_s * SECOND) + payload


def event(name, wall_s, mono_s=None, **fields):
    return record(1, {"event": name, **fields}, wall_s, mono_s)


class ConnectionLifetimesTest(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.temp = Path(temp.name)
        self.raw = self.temp / "sample"
        self.json_path = self.temp / "report.json"
        self.markdown_path = self.temp / "report.md"

    def recording(self, name, *records, opened=0, started=0, source="hl", root=None):
        path = (root or self.raw) / name
        path.parent.mkdir(parents=True, exist_ok=True)
        header = event("file_open", opened, opened - started, format=1, source=source,
                       process_start_wall_ns=BASE + started * SECOND,
                       recorder_version="synthetic", url="wss://test.invalid/ws")
        compressed = subprocess.run(["zstd", "-q", "-c"], input=header + b"".join(records),
                                    capture_output=True, check=True)
        path.write_bytes(compressed.stdout)
        return path

    def report(self, *args):
        result = subprocess.run(
            [sys.executable, "-I", str(SCRIPT), f"sample={self.raw}", *args,
             "--json", str(self.json_path), "--markdown", str(self.markdown_path)],
            cwd=self.temp, capture_output=True, text=True,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return json.loads(self.json_path.read_text())["labels"]

    def test_hourly_rotation_continues_the_connection(self):
        first = self.recording("20250101/hl-20250101T00-1735689600000.hlrec.zst",
                               event("connected", 3590))
        second = self.recording("20250101/hl-20250101T01-1735693200000.hlrec.zst",
                                event("disconnected", 3637, reason=EXPIRED), opened=3600)
        sample = self.report()["sample"]
        self.assertEqual(sample["end_counts"], {EXPIRED: 1})
        row, = sample["connections"]
        self.assertEqual(row["label"], "sample")
        self.assertEqual(row["source"], "hl")
        self.assertEqual(row["process_start_wall_ns"], BASE)
        self.assertEqual(row["connected_wall_ns"], BASE + 3590 * SECOND)
        self.assertEqual(row["end_wall_ns"], BASE + 3637 * SECOND)
        self.assertEqual(row["lifetime_ns"], 47 * SECOND)
        self.assertEqual(row["end_kind"], EXPIRED)

        markdown = self.markdown_path.read_text()
        self.assertNotIn(str(self.raw), markdown)
        self.assertEqual(len(sample["files"]), 2)
        for expected, entry in zip((first, second), sample["files"], strict=True):
            relative_path = expected.relative_to(self.raw).as_posix()
            digest = hashlib.sha256(expected.read_bytes()).hexdigest()
            self.assertEqual(entry["path"], relative_path)
            self.assertEqual(entry["sha256"], digest)
            self.assertIn(relative_path, markdown)
            self.assertIn(digest, markdown)

    def test_shutdown_ends_the_connected_session(self):
        self.recording("hl-01.hlrec.zst", event("connecting", 1),
                       event("connect_failed", 2, reason="unavailable"), event("connecting", 3),
                       event("connected", 4), event("shutdown", 13))
        sample = self.report()["sample"]
        self.assertEqual(sample["end_counts"], {"shutdown": 1})
        row, = sample["connections"]
        self.assertEqual((row["end_kind"], row["end_wall_ns"], row["lifetime_ns"]),
                         ("shutdown", BASE + 13 * SECOND, 9 * SECOND))

    def test_new_process_without_shutdown_uses_old_process_last_record(self):
        self.recording("hl-01.hlrec.zst", event("connected", 4), record(3, "last frame", 12))
        self.recording("hl-02.hlrec.zst", event("connected", 101, 1), event("shutdown", 104, 4),
                       opened=100, started=100)
        sample = self.report()["sample"]
        ended = "process ended without shutdown"
        self.assertEqual(sample["end_counts"], {ended: 1, "shutdown": 1})
        old, new = sample["connections"]
        self.assertEqual((old["process_start_wall_ns"], old["end_kind"], old["end_wall_ns"],
                          old["lifetime_ns"]), (BASE, ended, BASE + 12 * SECOND, 8 * SECOND))
        self.assertEqual((new["process_start_wall_ns"], new["connected_wall_ns"], new["lifetime_ns"]),
                         (BASE + 100 * SECOND, BASE + 101 * SECOND, 3 * SECOND))

    def test_open_at_end_is_measured_to_its_own_sources_last_record(self):
        self.recording("alpha-01.hlrec.zst", event("connected", 4), record(3, "last frame", 15),
                       source="alpha")
        self.recording("beta-01.hlrec.zst", event("connected", 7), event("shutdown", 29), source="beta")
        sample = self.report()["sample"]
        self.assertEqual(sample["end_counts"], {"open at end of input": 1, "shutdown": 1})
        self.assertCountEqual(
            [(row["source"], row["end_kind"], row["end_wall_ns"], row["lifetime_ns"])
             for row in sample["connections"]],
            [("alpha", "open at end of input", BASE + 15 * SECOND, 11 * SECOND),
             ("beta", "shutdown", BASE + 29 * SECOND, 22 * SECOND)],
        )

    def test_disconnect_reasons_verbatim_and_sorted_server_lifetimes(self):
        other_close = "closed by the server: 1001 Rolling  restart"
        reset = "read error: code 42 | peer reset"
        self.recording("hl-01.hlrec.zst",
                       event("connected", 1), event("disconnected", 10, reason=EXPIRED),
                       event("connected", 15), event("disconnected", 17, reason=EXPIRED),
                       event("connected", 20), event("disconnected", 25, reason=EXPIRED),
                       event("connected", 30), event("disconnected", 34, reason=other_close),
                       event("connected", 50), event("disconnected", 56, reason=reset))
        sample = self.report()["sample"]
        self.assertEqual([row["end_kind"] for row in sample["connections"]],
                         [EXPIRED, EXPIRED, EXPIRED, other_close, reset])
        self.assertEqual(sample["end_counts"], {EXPIRED: 3, other_close: 1, reset: 1})
        self.assertEqual(sample["server_closes"], {
            EXPIRED: {"lifetimes_ns": [2 * SECOND, 5 * SECOND, 9 * SECOND],
                      "min_ns": 2 * SECOND, "max_ns": 9 * SECOND},
            other_close: {"lifetimes_ns": [4 * SECOND], "min_ns": 4 * SECOND, "max_ns": 4 * SECOND},
        })

    def test_wall_clock_step_preserves_wall_times_and_monotonic_lifetime(self):
        self.recording("hl-01.hlrec.zst", event("connected", 25, 20),
                       event("disconnected", 18, 27, reason=EXPIRED))
        row, = self.report()["sample"]["connections"]
        self.assertEqual((row["connected_wall_ns"], row["end_wall_ns"], row["lifetime_ns"]),
                         (BASE + 25 * SECOND, BASE + 18 * SECOND, 7 * SECOND))

    def test_before_selects_whole_files_by_strict_file_open_time(self):
        first = self.recording("hl-01.hlrec.zst", event("connected", 59), event("shutdown", 65),
                               opened=58)
        # The filename says 00:00. The file_open record is exactly at the cutoff.
        self.recording("hl-20250101T00-1735689600000.hlrec.zst", event("connected", 68),
                       event("disconnected", 78, reason=EXPIRED), opened=60)
        sample = self.report("--before", "2025-01-01T00:01:00Z")["sample"]
        self.assertEqual([entry["path"] for entry in sample["files"]], [first.name])
        self.assertEqual(sample["end_counts"], {"shutdown": 1})
        row, = sample["connections"]
        self.assertEqual((row["end_wall_ns"], row["lifetime_ns"]), (BASE + 65 * SECOND, 6 * SECOND))

    def test_distinct_non_ping_sent_messages_per_label(self):
        btc = '{"method":"subscribe","subscription":{"type":"bbo","coin":"BTC"}}'
        eth = '{"method":"subscribe","subscription":{"type":"bbo","coin":"ETH"}}'
        self.recording("hl-01.hlrec.zst", event("connected", 1), record(4, btc, 2),
                       record(4, '{ "method": "ping" }', 3), record(2, eth, 4),
                       event("disconnected", 5, reason=EXPIRED), event("connected", 6),
                       record(4, btc, 7), event("shutdown", 8))
        paper = self.temp / "paper"
        self.recording("hl-01.hlrec.zst", event("connected", 1), record(4, eth, 2),
                       event("shutdown", 3), root=paper)
        labels = self.report(f"paper={paper}")
        self.assertEqual(labels["sample"]["sent_messages"], [btc])
        self.assertEqual(labels["paper"]["sent_messages"], [eth])
        self.assertEqual(labels["paper"]["end_counts"], {"shutdown": 1})
        markdown = self.markdown_path.read_text()
        self.assertNotIn(btc, markdown)
        self.assertNotIn(eth, markdown)


if __name__ == "__main__":
    unittest.main()
