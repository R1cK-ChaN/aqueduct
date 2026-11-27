"""Synthetic RFC 0002 recordings, exercised through the isolated CLI.

Run with python3 -I research/tests/test_feed_timing.py. Requires installed zstd.
"""

import base64
import hashlib
import json
import struct
import subprocess
import sys
import tempfile
import unittest
import zlib
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "feed_timing.py"
BASE_MS = 1_795_507_200_000
MILLISECOND = 1_000_000


def record(kind, payload, offset_ns=0, *, wall_base_ms=BASE_MS):
    if not isinstance(payload, bytes):
        payload = json.dumps(payload).encode()
    header = struct.pack("<IBqQ", len(payload), kind, wall_base_ms * MILLISECOND + offset_ns, offset_ns)
    return header + payload


def frame(channel, data, receive_ms):
    return record(2, {"channel": channel, "data": data}, receive_ms * MILLISECOND)


def bbo(block_ms, receive_ms, coin="BTC"):
    return frame("bbo", {"coin": coin, "time": block_ms, "bbo": [None, None]}, receive_ms)


def trades(block_times, receive_ms):
    return frame("trades", [{"coin": "BTC", "time": time} for time in block_times], receive_ms)


class FeedTimingTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.raw = self.root / "raw"
        self.raw.mkdir()
        self.json_path = self.root / "timing.json"
        self.markdown_path = self.root / "timing.md"
        self.header_metadata = {
            "event": "file_open", "format": 1, "source": "hl",
            "url": "wss://test.invalid/market/ws", "recorder_version": "synthetic",
            "process_start_wall_ns": BASE_MS * MILLISECOND,
        }
        self.header = record(1, self.header_metadata)

    def write_recording(self, records, name="hl-20261124T08-1795507200000.hlrec.zst"):
        path = self.raw / name
        compressed = subprocess.run(
            ["zstd", "--quiet", "--stdout", "--compress"], input=records,
            capture_output=True, check=True,
        ).stdout
        path.write_bytes(compressed)
        return path

    def run_script(self):
        return subprocess.run(
            [sys.executable, "-I", str(SCRIPT), str(self.raw),
             "--json", str(self.json_path), "--markdown", str(self.markdown_path)],
            capture_output=True, text=True, check=False,
        )

    def report(self):
        result = self.run_script()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return json.loads(self.json_path.read_text())

    def test_grouping_order_compression_and_determinism(self):
        a, b, c = BASE_MS, BASE_MS + 70, BASE_MS + 210
        compressor = zlib.compressobj(wbits=-15)
        diff = json.dumps({"c": "BTC", "t": a, "l": [[], []], "r": [[], []]}).encode()
        packed = base64.b64encode(compressor.compress(diff) + compressor.flush()).decode()
        def snapshot(time):
            return {"s": {"coin": "ETH", "time": time, "levels": [[], []]}}

        path = self.write_recording(self.header + b"".join([
            bbo(a, 200), bbo(a, 200, "ETH"),
            frame("activeAssetCtx", {}, 200), frame("pong", None, 200),
            trades([a, a], 200), frame("l2", snapshot(a), 201), bbo(b, 270),
            frame("l2", {"c": packed}, 271), trades([b, c], 420),
            frame("l2", snapshot(c), 420), bbo(c, 421), trades([c, c, c], 422),
        ]))
        for name in ("ignored.txt", "hl-open.hlrec.zst.part", "hl-cut.truncated.hlrec.zst"):
            (self.raw / name).write_bytes(b"incomplete input must not be read")

        data = self.report()
        self.assertEqual(len(data["files"]), 1)
        self.assertEqual(data["files"][0]["sha256"], hashlib.sha256(path.read_bytes()).hexdigest())
        self.assertEqual(data["coverage"]["frames"], 10)
        self.assertEqual(data["coverage"]["frames_by_channel"], {"bbo": 4, "l2": 3, "trades": 3})
        self.assertEqual(data["spacing_ms"]["all"]["n"], 2)
        self.assertEqual(data["spacing_ms"]["all"]["min"], 70)
        self.assertEqual(data["spacing_ms"]["all"]["max"], 140)
        self.assertEqual(data["spacing_ms"]["by_trades"]["2-4"]["n"], 1)
        self.assertEqual(data["spacing_ms"]["by_trades"]["1"]["n"], 1)
        self.assertEqual(data["order"]["blocks"], 3)
        self.assertEqual(data["order"]["contiguous_blocks"], 1)
        self.assertEqual(data["order"]["interleaved_blocks"], 2)
        self.assertEqual(data["order"]["late_frames"], 1)
        self.assertEqual(data["order"]["first_channel_patterns"], {
            "bbo > trades > l2": 1, "bbo > trades": 1, "trades > l2 > bbo": 1,
        })
        self.assertEqual(data["order"]["channel_runs"], {
            "bbo > trades > l2": 1, "bbo > trades": 1, "trades > l2 > bbo > trades": 1,
        })
        self.assertEqual(data["order"]["spread_ms"]["min"], 2)
        self.assertEqual(data["order"]["spread_ms"]["max"], 150)
        self.assertEqual(data["socket"]["pairs"], 9)
        self.assertEqual(data["socket"]["identical_wall_ns"], 3)
        expected_latency = [("bbo", 4, 200, 211), ("l2", 3, 201, 271), ("trades", 4, 200, 350)]
        for channel, count, minimum, maximum in expected_latency:
            latency = data["latency_ms"]["by_channel"][channel]
            self.assertEqual((latency["n"], latency["min"], latency["max"]), (count, minimum, maximum))
        json_output = self.json_path.read_bytes()
        markdown = self.markdown_path.read_text()
        self.assertIn("Inputs for the time model", markdown)
        self.assertIn(data["files"][0]["sha256"], markdown)
        self.assertEqual(self.report(), data)
        self.assertEqual(self.json_path.read_bytes(), json_output)
        self.assertEqual(self.markdown_path.read_text(), markdown)

    def test_preceding_second_excludes_simultaneous_frames(self):
        offsets = [0, 0, 500_000_000, 1_000_000_000, 1_000_000_001, 2_100_000_000]
        records = [record(2, {"channel": "bbo", "data": {
            "coin": "BTC", "time": BASE_MS + offset // MILLISECOND, "bbo": [None, None],
        }}, offset) for offset in offsets]
        self.write_recording(self.header + b"".join(records))
        report = self.report()
        self.assertEqual(report["socket"]["identical_wall_ns"], 1)
        self.assertEqual(report["socket"]["same_wall_ms"], 2)
        data = report["latency_ms"]
        self.assertEqual(data["activity_upper_bounds"], [0, 0, 2, 2])
        self.assertEqual([bucket["bbo"]["n"] for bucket in data["by_activity"]], [3, 0, 2, 0, 1])

    def test_subscription_replay_hour_rotation_and_process_restart(self):
        def subscribe(coin, receive_ms, process_start_ms=BASE_MS):
            return record(4, {"method": "subscribe", "subscription": {
                "type": "trades", "coin": coin,
            }}, receive_ms * MILLISECOND, wall_base_ms=process_start_ms)

        def trade(coin, block_ms, receive_ms, process_start_ms=BASE_MS):
            return record(2, {"channel": "trades", "data": [{
                "coin": coin, "time": block_ms,
            }]}, receive_ms * MILLISECOND, wall_base_ms=process_start_ms)

        first_block = BASE_MS + 3_599_600
        second_block = first_block + 67
        third_block = second_block + 67
        self.write_recording(b"".join([
            record(1, self.header_metadata, 3_599_000 * MILLISECOND),
            subscribe("BTC", 3_599_000), subscribe("ETH", 3_599_000),
            trade("BTC", BASE_MS + 1_000, 3_599_700),
            trade("BTC", first_block, 3_599_800),
            trade("ETH", BASE_MS + 2_000, 3_599_900),
        ]), f"hl-20261124T08-{BASE_MS + 3_599_000}.hlrec.zst")
        self.write_recording(b"".join([
            record(1, self.header_metadata, 3_600_000 * MILLISECOND),
            trade("BTC", second_block, 3_600_000),
            trade("ETH", third_block, 3_600_001), bbo(third_block, 3_600_001),
        ]), f"hl-20261124T09-{BASE_MS + 3_600_000}.hlrec.zst")
        restart = BASE_MS + 3_601_000
        restarted_header = {**self.header_metadata, "process_start_wall_ns": restart * MILLISECOND}
        self.write_recording(b"".join([
            record(1, restarted_header, wall_base_ms=restart),
            subscribe("BTC", 0, restart), trade("BTC", third_block, 100, restart),
            trade("BTC", restart, 200, restart),
            record(2, {"channel": "bbo", "data": {
                "coin": "BTC", "time": restart + 67, "bbo": [None, None],
            }}, 267 * MILLISECOND, wall_base_ms=restart),
        ]), f"hl-20261124T09-{restart}.hlrec.zst")

        data = self.report()
        self.assertEqual(len(data["files"]), 3)
        self.assertEqual(data["coverage"]["frames"], 9)
        self.assertEqual(data["coverage"]["segments"], 2)
        self.assertEqual(data["coverage"]["replay_frames"], 3)
        self.assertEqual(data["coverage"]["replay_memberships"], 3)
        self.assertEqual(data["socket"]["pairs"], 7)
        self.assertEqual(data["socket"]["identical_wall_ns"], 1)
        self.assertEqual(data["spacing_ms"]["all"]["n"], 6)
        self.assertEqual(data["spacing_ms"]["live"]["n"], 3)
        self.assertEqual(data["spacing_ms"]["live"]["min"], 67)
        self.assertEqual(data["spacing_ms"]["live"]["max"], 67)
        channels = data["latency_ms"]["by_channel"]
        self.assertEqual(channels["trades"]["n"], 7)
        self.assertEqual(channels["trades"]["min"], 200)
        self.assertEqual(channels["trades"]["max"], 3_598_700)
        self.assertEqual(channels["live_trades"]["n"], 4)
        self.assertEqual(channels["live_trades"]["min"], 200)
        self.assertEqual(channels["live_trades"]["max"], 333)

    def test_rejects_corrupt_records_without_writing_reports(self):
        bad_records = {
            "partial header": self.header + b"\x01\x00",
            "partial payload": self.header + struct.pack("<IBqQ", 10, 2, 0, 0) + b"{}",
            "unknown record kind": self.header + record(99, {}),
            "missing file_open": bbo(BASE_MS, 200),
            "invalid block time": self.header + bbo("invalid", 200),
            "invalid compressed l2": self.header + frame("l2", {"c": "%%%"}, 200),
        }
        for label, records in bad_records.items():
            with self.subTest(label=label):
                self.write_recording(records)
                result = self.run_script()
                self.assertNotEqual(result.returncode, 0)
                self.assertTrue(result.stderr)
                self.assertFalse(self.json_path.exists())
                self.assertFalse(self.markdown_path.exists())

    def test_rejects_truncated_zstd_stream(self):
        path = self.write_recording(self.header + bbo(BASE_MS, 200))
        path.write_bytes(path.read_bytes()[:-1])
        result = self.run_script()
        self.assertNotEqual(result.returncode, 0)
        self.assertTrue(result.stderr)
        self.assertFalse(self.json_path.exists())
        self.assertFalse(self.markdown_path.exists())

    def test_requires_input_directory_without_writing_reports(self):
        result = subprocess.run(
            [sys.executable, "-I", str(SCRIPT)], cwd=self.root, capture_output=True, text=True,
        )
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertIn("usage:", result.stderr)
        self.assertIn("required: root", result.stderr)
        self.assertEqual(list(self.root.iterdir()), [self.raw])

    def test_rejects_report_destinations_inside_input_directory(self):
        self.write_recording(self.header + bbo(BASE_MS, 200))
        recordings = self.root / "recordings"
        recordings.symlink_to(self.raw, target_is_directory=True)
        for json_path, markdown_path in (
            (self.raw / "timing.json", self.markdown_path),
            (self.json_path, recordings / "timing.md"),
        ):
            with self.subTest(json_path=json_path, markdown_path=markdown_path):
                result = subprocess.run(
                    [sys.executable, "-I", str(SCRIPT), str(recordings),
                     "--json", str(json_path), "--markdown", str(markdown_path)],
                    capture_output=True, text=True,
                )
                self.assertEqual(result.returncode, 2, result.stderr)
                self.assertIn("invalid report destination", result.stderr)
                self.assertFalse(json_path.exists())
                self.assertFalse(markdown_path.exists())

    def test_help_works_in_isolated_python(self):
        result = subprocess.run([sys.executable, "-I", str(SCRIPT), "--help"], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("--markdown", result.stdout)


if __name__ == "__main__":
    unittest.main()
