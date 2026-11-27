"""Opt-in CLI tests using a new disposable ClickHouse container.

Build the loader and run from the repository root.

    cargo build -p hl-loader
    HLMM_TEST_CLICKHOUSE=1 python3 -I crates/hl-loader/tests/test_rawlog_carry.py

The image clickhouse/clickhouse-server:26.8 must already be installed. This
test creates its own container on a dynamically selected loopback port,
uses temporary in-memory data and removes the container and its volumes.
It takes no database URL or credentials from the surrounding environment.
Every recording is generated from synthetic frames in this file.
"""

import hashlib
import json
from pathlib import Path
import runpy
import struct
import unittest


# -I omits the script directory from sys.path. Load the sibling explicitly.
DisposableClickHouseTest = runpy.run_path(
    str(Path(__file__).with_name("clickhouse_test.py"))
)["DisposableClickHouseTest"]
START_MS = 1_700_000_000_000
HEADER = struct.Struct("<IBqQ")
MARKET_TABLES = ("l2_snapshot", "fills", "bbo", "ws_trades")
BINARY_COLUMNS = {
    "fills": ("user", "builder", "cloid", "hash", "liquidated_user"),
    "ws_trades": ("buyer", "seller", "hash"),
}
WRITTEN_TABLES = MARKET_TABLES + (
    "instruments",
    "ingest_log",
    "rawlog_state",
    "ingest_conflict",
)


def synthetic_records():
    """Three recording parts with a stale positional diff after a snapshot."""

    def record(kind, received, payload):
        return (
            kind,
            (START_MS + received) * 1_000_000,
            received * 1_000_000,
            json.dumps(payload).encode(),
        )

    def level(price, size="1", orders=1):
        return {"px": str(price), "sz": size, "n": orders}

    def bbo(received, block, bid, ask, size="1", orders=1):
        data = {
            "coin": "MON",
            "time": START_MS + block,
            "bbo": [level(bid, size, orders), level(ask, size, orders)],
        }
        return record(2, received, {"channel": "bbo", "data": data})

    def diff(received, block, removed, bids=(), asks=()):
        updates = [
            [{"p": str(price), "s": size} for price, size in side]
            for side in (bids, asks)
        ]
        change = {
            "c": "MON",
            "t": START_MS + block,
            "l": updates,
            "r": [removed, removed],
        }
        return record(2, received, {"channel": "l2", "data": {"u": change}})

    def trade(received, traded, tid):
        data = {
            "coin": "MON",
            "side": "B",
            "px": "100",
            "sz": "1",
            "time": START_MS + traded,
            "tid": tid,
            "hash": "0x" + "00" * 32,
            "users": ["0x" + "01" * 20, "0x" + "02" * 20],
        }
        return record(2, received, {"channel": "trades", "data": [data]})

    snapshot = {
        "coin": "MON",
        "time": START_MS + 10,
        "levels": [
            [level(price) for price in (100, 99, 98)],
            [level(price) for price in (101, 102, 103)],
        ],
    }
    first = [
        record(
            1,
            0,
            {
                "event": "file_open",
                "url": "wss://api.hyperliquid.xyz/ws",
                "source": "synthetic",
                "process_start_wall_ns": START_MS * 1_000_000,
            },
        ),
        record(1, 1, {"event": "connected"}),
        trade(2, -1, 1),
        bbo(11, 10, 100, 101),
        record(2, 12, {"channel": "l2", "data": {"s": snapshot}}),
    ]
    second = [
        # The snapshot already includes this deletion. Applying it again
        # leaves only two levels, so the next deletion of position 2 fails.
        diff(13, 10, [2]),
        trade(14, 10, 2),
        diff(21, 20, [2], [(97, "2")], [(104, "2")]),
        bbo(22, 20, 100, 101),
        diff(31, 30, [0], [(98, "3")], [(103, "3")]),
        bbo(32, 30, 99, 102),
    ]
    third = [
        trade(33, 30, 3),
        diff(41, 40, [1], [(96, "4")], [(105, "4")]),
        bbo(42, 40, 99, 102),
        diff(51, 50, [0, 2], [(98, "5"), (95, "6")], [(103, "5"), (106, "6")]),
        bbo(52, 50, 98, 103, size="5", orders=0),
        trade(53, 50, 4),
    ]
    records = first + second + third
    cuts = (0, len(first), len(first) + len(second), len(records))
    return records, cuts


def encode(records):
    return b"".join(
        HEADER.pack(len(payload), kind, wall_ns, mono_ns) + payload
        for kind, wall_ns, mono_ns, payload in records
    )


class RawlogCarryTest(DisposableClickHouseTest):
    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        cls.records, cls.cuts = synthetic_records()
        cls.original_open = json.loads(cls.records[0][3])
        cls.next_chain = 0

    def setUp(self):
        self.clear_data()

    def clear_data(self):
        for table in WRITTEN_TABLES:
            self.sql(f"TRUNCATE TABLE hl.{table}")

    def chain(
        self, *, legacy=False, whole=False, source=None, process_start=None, clock_step=False
    ):
        type(self).next_chain += 1
        number = type(self).next_chain
        source = source or f"synthetic-{number}"
        process_start = process_start or self.original_open["process_start_wall_ns"] + number
        cuts = (0, len(self.records)) if whole else self.cuts
        paths = []
        for file_seq, (begin, end) in enumerate(zip(cuts, cuts[1:])):
            header = {
                **self.original_open,
                "source": source,
                "process_start_wall_ns": process_start,
            }
            if not legacy:
                header["file_seq"] = file_seq
            contents = self.records[begin:end]
            if begin == 0:
                contents = contents[1:]
            opened_at = self.records[begin][1] - 1
            if clock_step:
                opened_at = self.records[0][1] - file_seq * 1_000_000_000
            records = [
                (1, opened_at, self.records[begin][2], json.dumps(header).encode()),
                *contents,
            ]
            path = self.directory / f"chain-{number}-part-{file_seq}.hlrec"
            path.write_bytes(encode(records))
            paths.append(path)
        return paths, source, process_start

    def load(self, paths, *flags, expected_code=0):
        return self.cli("load", "rawlog", *flags, *paths, expected_code=expected_code)

    def market_rows(self):
        result = {}
        for table in MARKET_TABLES:
            binary = BINARY_COLUMNS.get(table, ())
            excluded = ", ".join(("ingest_id", *binary))
            projection = f"* EXCEPT ({excluded})"
            projection += "".join(f", hex({column}) AS {column}" for column in binary)
            result[table] = sorted(
                json.dumps(row, sort_keys=True)
                for row in self.rows(f"SELECT {projection} FROM hl.{table} FINAL")
            )
        return result

    def baseline(self):
        paths, _, _ = self.chain(whole=True, legacy=True)
        loaded = self.load(paths)[0]
        self.assertEqual(loaded["status"], "loaded")
        self.assertEqual(loaded["counters"].get("stale_diffs_skipped", 0), 1)
        self.assertEqual(loaded["counters"].get("book_errors", 0), 0)
        self.assertEqual(loaded["counters"].get("diffs_without_snapshot", 0), 0)
        expected = self.market_rows()
        self.assertEqual(len(expected["l2_snapshot"]), 5)
        self.assertEqual(len(expected["bbo"]), 5)
        self.assertEqual(len(expected["ws_trades"]), 4)
        self.assert_book_rows()
        self.clear_data()
        return expected

    def assert_book_rows(self):
        books = self.rows(
            "SELECT toInt32(toUnixTimestamp64Milli(ts_exch) - "
            f"{START_MS}) AS block, bid_px AS bids, ask_px AS asks "
            "FROM hl.l2_snapshot FINAL ORDER BY ts_exch"
        )
        self.assertEqual(
            books,
            [
                {"block": 10, "bids": [100, 99, 98], "asks": [101, 102, 103]},
                {"block": 20, "bids": [100, 99, 97], "asks": [101, 102, 104]},
                {"block": 30, "bids": [99, 98, 97], "asks": [102, 103, 104]},
                {"block": 40, "bids": [99, 97, 96], "asks": [102, 104, 105]},
                {"block": 50, "bids": [98, 97, 95], "asks": [103, 104, 106]},
            ],
        )

    def table_counts(self):
        return {table: int(self.sql(f"SELECT count() FROM hl.{table}")) for table in WRITTEN_TABLES}

    def state_rows(self, source, process_start):
        return self.rows(
            "SELECT * FROM hl.rawlog_state FINAL "
            f"WHERE source = '{source}' AND process_start_wall_ns = {process_start} "
            "ORDER BY file_seq"
        )

    def assert_carried(self, messages):
        self.assertEqual(len(messages), 3)
        for message, book_rows, stale in zip(messages, (1, 2, 2), (0, 1, 0)):
            self.assertEqual(message["status"], "loaded")
            self.assertEqual(message["rows"].get("l2_snapshot", 0), book_rows)
            self.assertEqual(message["counters"].get("stale_diffs_skipped", 0), stale)
            self.assertEqual(message["counters"].get("book_errors", 0), 0)
            self.assertEqual(message["counters"].get("diffs_without_snapshot", 0), 0)
        for message in messages[1:]:
            self.assertGreater(message["counters"].get("trades_live", 0), 0)
            self.assertEqual(message["counters"].get("trades_replayed", 0), 0)

    def test_migration_applies_once(self):
        self.assertEqual(
            self.first_migration["applied"],
            ["0001_init", "0002_quality", "0003_rawlog_state"],
        )
        self.assertEqual(self.cli("migrate"), [{"applied": []}])
        self.assertEqual(self.sql("SELECT count() FROM hl.schema_migrations").strip(), "3")

    def test_waiting_writes_nothing_and_no_carry_is_logged(self):
        paths, source, process_start = self.chain()
        before = self.table_counts()
        waiting = self.load([paths[1]], expected_code=2)[0]
        self.assertEqual(waiting["status"], "waiting_for_predecessor")
        self.assertEqual(waiting["source"], source)
        self.assertEqual(waiting["process_start_wall_ns"], process_start)
        self.assertEqual(waiting["missing_file_seq"], 0)
        self.assertEqual(self.table_counts(), before)

        loaded = self.load([paths[1]], "--no-carry")[0]
        self.assertEqual(loaded["status"], "loaded")
        self.assertNotIn("l2_snapshot", loaded["rows"])
        self.assertGreater(loaded["counters"]["diffs_without_snapshot"], 0)
        self.assertEqual(loaded["counters"]["trades_live"], 1)
        self.assertEqual(loaded["counters"].get("trades_replayed", 0), 0)
        self.assertEqual(self.sql("SELECT live FROM hl.ws_trades FINAL WHERE tid = 2").strip(), "1")
        logs = self.rows("SELECT details FROM hl.ingest_log WHERE status = 'done'")
        self.assertEqual(json.loads(logs[0]["details"])["carry"], "missing_predecessor")
        self.assertEqual(len(self.state_rows(source, process_start)), 1)

        self.load([paths[0]])
        carried = self.load([paths[1]], "--force", "--no-carry")[0]
        self.assertEqual(carried["counters"].get("diffs_without_snapshot", 0), 0)
        self.assertEqual(carried["counters"].get("stale_diffs_skipped", 0), 1)
        self.assertEqual(carried["counters"].get("book_errors", 0), 0)
        self.assertGreater(carried["counters"]["trades_live"], 0)
        self.assertEqual(carried["rows"]["l2_snapshot"], 2)

    def test_individual_numbered_files_and_force_preserve_all_rows(self):
        expected = self.baseline()
        paths, source, process_start = self.chain()
        messages = [self.load([path])[0] for path in paths]
        self.assert_carried(messages)
        self.assertEqual(self.market_rows(), expected)
        logged = self.sql(
            "SELECT JSONExtractUInt(details, 'counters', 'stale_diffs_skipped') "
            f"FROM hl.ingest_log WHERE ingest_id = {messages[1]['ingest_id']}"
        )
        self.assertEqual(int(logged), 1)
        previous = self.state_rows(source, process_start)
        self.assertEqual(len(previous), 3)
        self.assertEqual(
            [message["status"] for message in self.load(paths)],
            ["already_loaded"] * 3,
        )

        forced = self.load([paths[1]], "--force")[0]
        self.assertEqual(forced["status"], "loaded")
        self.assertEqual(forced["counters"].get("stale_diffs_skipped", 0), 1)
        self.assertEqual(forced["counters"].get("book_errors", 0), 0)
        self.assertEqual(forced["rows"].get("l2_snapshot", 0), 2)
        self.assertEqual(self.market_rows(), expected)
        current = self.state_rows(source, process_start)
        self.assertEqual(current[0], previous[0])
        self.assertEqual(current[2], previous[2])
        self.assertEqual(current[1]["state_json"], previous[1]["state_json"])
        self.assertEqual(current[1]["sha256"], previous[1]["sha256"])
        self.assertGreater(int(current[1]["ingest_id"]), int(previous[1]["ingest_id"]))
        self.assertEqual(int(current[1]["ingest_id"]), forced["ingest_id"])

    def test_retry_after_a_partial_load_with_the_old_l2_token(self):
        paths, _, _ = self.chain(whole=True)
        digest = hashlib.sha256(paths[0].read_bytes()).hexdigest()
        # An interrupted load wrote an L2 chunk under the old decoder's token.
        self.sql(
            "INSERT INTO hl.l2_snapshot (ts_exch) "
            f"SETTINGS insert_deduplication_token = '{digest}:hl.l2_snapshot:0' "
            f"VALUES (fromUnixTimestamp64Milli({START_MS + 10}))"
        )
        loaded = self.load(paths)[0]
        self.assertEqual(loaded["counters"]["stale_diffs_skipped"], 1)
        count = self.sql(
            f"SELECT count() FROM hl.l2_snapshot WHERE ingest_id = {loaded['ingest_id']}"
        )
        self.assertEqual(int(count), 5)

    def test_retry_updates_unknown_start_trades_written_with_the_old_token(self):
        paths, _, _ = self.chain()
        digest = hashlib.sha256(paths[1].read_bytes()).hexdigest()
        # Seed a trade of the same instrument from a separate recording chain.
        other_paths, _, _ = self.chain()
        self.load([other_paths[0]])
        # An earlier loader wrote trade 2 as a snapshot and stopped before logging done.
        self.sql(
            "INSERT INTO hl.ws_trades SELECT * REPLACE ("
            f"fromUnixTimestamp64Milli({START_MS + 10}) AS ts_exch, "
            f"fromUnixTimestamp64Nano({(START_MS + 14) * 1_000_000}) AS ts_recv, "
            "toUInt64(2) AS tid) FROM (SELECT * FROM hl.ws_trades WHERE tid = 1) "
            f"SETTINGS insert_deduplication_token = '{digest}:hl.ws_trades:0'"
        )
        live = "SELECT live FROM hl.ws_trades FINAL WHERE tid = 2"
        self.assertEqual(self.sql(live).strip(), "0")
        self.load([paths[1]], "--no-carry")
        self.assertEqual(self.sql(live).strip(), "1")

    def test_reversed_numbered_and_legacy_files_are_ordered(self):
        expected = self.baseline()
        for legacy in (False, True):
            with self.subTest(legacy=legacy):
                self.clear_data()
                paths, source, process_start = self.chain(legacy=legacy)
                messages = self.load(list(reversed(paths)))
                self.assertEqual([message["file"] for message in messages], [str(path) for path in paths])
                self.assert_carried(messages)
                self.assertEqual(self.market_rows(), expected)
                self.assertEqual(len(self.state_rows(source, process_start)), 0 if legacy else 3)

    def test_already_loaded_legacy_predecessor_still_supplies_memory_state(self):
        expected = self.baseline()
        paths, _, _ = self.chain(legacy=True, clock_step=True)
        self.load([paths[0]])
        # Wall-clock steps must preserve process order. An overlapping glob
        # must not apply the middle file's positional diffs twice.
        repeated = [paths[2], paths[1], paths[0], paths[1]]
        messages = self.load(repeated)
        self.assertEqual(
            [message["status"] for message in messages],
            ["already_loaded", "loaded", "already_loaded", "loaded"],
        )
        for message in (messages[1], messages[3]):
            self.assertGreater(message["rows"].get("l2_snapshot", 0), 0)
            self.assertGreater(message["counters"].get("trades_live", 0), 0)
        self.assertEqual(self.market_rows(), expected)
        forced = self.load(repeated, "--force")
        self.assertEqual(
            [message["status"] for message in forced],
            ["loaded", "loaded", "already_loaded", "loaded"],
        )
        self.assertEqual(self.market_rows(), expected)

    def test_source_and_process_start_both_isolate_chains(self):
        paths, source, process_start = self.chain()
        self.load([paths[0]])
        other_source, _, _ = self.chain(source=source + "-other", process_start=process_start)
        other_process, _, _ = self.chain(source=source, process_start=process_start + 1_000_000)
        before = self.table_counts()
        for other in (other_source, other_process):
            waiting = self.load([other[1]], expected_code=2)[0]
            self.assertEqual(waiting["status"], "waiting_for_predecessor")
            self.assertEqual(self.table_counts(), before)

    def test_waiting_during_force_retains_the_files_previous_state(self):
        paths, source, process_start = self.chain()
        self.load(paths[:2])
        self.sql(
            "ALTER TABLE hl.rawlog_state DELETE "
            f"WHERE source = '{source}' AND file_seq = 0 SETTINGS mutations_sync = 2"
        )
        previous = self.state_rows(source, process_start)
        counts = self.table_counts()
        waiting = self.load([paths[1]], "--force", expected_code=2)[0]
        self.assertEqual(waiting["status"], "waiting_for_predecessor")
        self.assertEqual(self.state_rows(source, process_start), previous)
        self.assertEqual(self.table_counts(), counts)

    def test_incomplete_completion_is_detected_and_repairable(self):
        paths, source, process_start = self.chain()
        loaded = self.load([paths[0]])[0]
        self.sql(
            "ALTER TABLE hl.ingest_log DELETE "
            f"WHERE ingest_id = {loaded['ingest_id']} SETTINGS mutations_sync = 2"
        )
        # State is safe for successors once market-data insertion finished,
        # even when the final success-log write was interrupted.
        successor = self.load([paths[1]])[0]
        self.assertGreater(successor["rows"]["l2_snapshot"], 0)
        retried = self.load([paths[0]])[0]
        self.assertEqual(retried["status"], "loaded")
        self.assertGreater(retried["ingest_id"], loaded["ingest_id"])

        self.sql(
            "ALTER TABLE hl.rawlog_state DELETE "
            f"WHERE source = '{source}' AND file_seq = 0 SETTINGS mutations_sync = 2"
        )
        missing = self.load([paths[0]], expected_code=2)[0]
        self.assertEqual(missing["status"], "failed")
        self.assertIn("state", missing["error"])
        repaired = self.load([paths[0]], "--force")[0]
        self.assertEqual(repaired["status"], "loaded")
        states = self.state_rows(source, process_start)
        self.assertEqual(int(states[0]["ingest_id"]), repaired["ingest_id"])
        self.assertEqual(self.load([paths[0]])[0]["status"], "already_loaded")


if __name__ == "__main__":
    unittest.main(verbosity=2)
