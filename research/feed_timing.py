#!/usr/bin/env python3
"""Measure Hyperliquid feed timing from complete, read-only raw recordings."""

from __future__ import annotations

import argparse
import base64
from bisect import bisect_left
from collections import Counter, defaultdict, deque
from contextlib import ExitStack, contextmanager
from dataclasses import dataclass
from datetime import UTC, datetime
import hashlib
from itertools import chain, combinations, groupby, pairwise, permutations
import json
from operator import attrgetter
from pathlib import Path
import struct
import subprocess
import tempfile
from typing import BinaryIO, Iterable, Iterator
import zlib


HEADER = struct.Struct("<IBqQ")
MAX_PAYLOAD_LEN = 64 * 1024 * 1024
MAX_INFLATED_LEN = 16 * 1024 * 1024
NS_PER_MS = 1_000_000
NS_PER_SECOND = 1_000_000_000
HOUR_NS = 3600 * NS_PER_SECOND
CHANNELS = ("bbo", "l2", "trades")
TRADE_BANDS = ("0", "1", "2-4", "5+")
PERCENTILES = (("min", 0), ("p1", 10), ("p10", 100), ("p50", 500),
               ("p90", 900), ("p99", 990), ("p99.9", 999), ("max", 1000))
CONNECTION_EVENTS = {"connecting", "connected", "connect_failed", "disconnected", "shutdown"}


class RecordingError(ValueError):
    """A complete recording or a timing field could not be decoded safely."""

    def __init__(self, message: str, path: Path | None = None, record_index: int | None = None):
        self.path = path
        location = f"{path}: " if path is not None else ""
        if record_index is not None:
            location += f"record {record_index}: "
        super().__init__(location + message)


@dataclass(frozen=True, slots=True)
class Record:
    kind: int
    recv_wall_ns: int
    recv_mono_ns: int
    payload: bytes
    record_index: int


@dataclass(frozen=True, slots=True)
class Observation:
    """One physical market frame or one metadata event, in recording order."""

    path: Path
    stream_id: int
    recv_wall_ns: int
    recv_mono_ns: int
    channel: str | None = None
    event: str | None = None
    blocks: tuple[tuple[int, int], ...] = ()
    replay: bool = False


def recording_paths(root: Path) -> list[Path]:
    """Return complete files in source/day/hour/open-time filename order."""
    return sorted(
        path for path in root.rglob("*.hlrec.zst")
        if path.is_file() and not path.name.endswith(".truncated.hlrec.zst")
    )


@contextmanager
def _record_stream(path: Path) -> Iterator[BinaryIO]:
    """Use the installed zstd executable, checking its final checksum status."""
    with tempfile.TemporaryFile() as errors:
        process = subprocess.Popen(
            ["zstd", "-q", "-d", "-c", "--", str(path)],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=errors,
        )
        try:
            assert process.stdout is not None
            yield process.stdout
            status = process.wait()
            if status:
                errors.seek(0)
                detail = errors.read(4096).decode("utf-8", errors="replace").strip()
                raise RecordingError(f"zstd failed ({status}): {detail}", path)
        finally:
            if process.stdout is not None:
                process.stdout.close()
            if process.poll() is None:
                process.terminate()
            process.wait()


def iter_records(path: Path) -> Iterator[Record]:
    """Validate RFC 0002 framing without expanding a received frame into rows."""
    path = Path(path)
    with _record_stream(path) as stream:
        record_index = 0
        while header := stream.read(HEADER.size):
            record_index += 1
            if len(header) != HEADER.size:
                raise RecordingError("truncated record header", path, record_index)
            length, kind, wall_ns, mono_ns = HEADER.unpack(header)
            if kind not in (1, 2, 3, 4):
                raise RecordingError(f"unknown record kind {kind}", path, record_index)
            if length > MAX_PAYLOAD_LEN:
                raise RecordingError(
                    f"payload exceeds {MAX_PAYLOAD_LEN} bytes", path, record_index,
                )
            payload = stream.read(length)
            if len(payload) != length:
                raise RecordingError("truncated record payload", path, record_index)
            yield Record(kind, wall_ns, mono_ns, payload, record_index)


def _object(value: object, field: str) -> dict:
    if not isinstance(value, dict):
        raise RecordingError(f"{field} must be an object")
    return value


def _integer(value: object, field: str, minimum: int, maximum: int) -> int:
    if type(value) is not int or not minimum <= value <= maximum:
        raise RecordingError(f"{field} must be an integer in [{minimum}, {maximum}]")
    return value


def _json(payload: bytes | str) -> object:
    try:
        return json.loads(payload)
    except (ValueError, UnicodeDecodeError) as error:
        raise RecordingError(f"invalid JSON: {error}") from error


def _block(data: object, coin_field: str = "coin", time_field: str = "time") -> tuple[str, int]:
    data = _object(data, "market data")
    coin = data.get(coin_field)
    if not isinstance(coin, str) or not coin:
        raise RecordingError(f"{coin_field} must be a nonempty string")
    millis = _integer(data.get(time_field), time_field, 0, ((1 << 63) - 1) // 1_000_000)
    return coin, millis


def _inflate_l2(packed: object) -> object:
    if not isinstance(packed, str):
        raise RecordingError("l2.data.c must be a base64 string")
    try:
        compressed = base64.b64decode(packed, validate=True)
        inflater = zlib.decompressobj(-zlib.MAX_WBITS)
        payload = inflater.decompress(compressed, MAX_INFLATED_LEN + 1)
    except (ValueError, zlib.error) as error:
        raise RecordingError(f"invalid compressed l2 payload: {error}") from error
    if len(payload) > MAX_INFLATED_LEN or inflater.unconsumed_tail:
        raise RecordingError("inflated l2 payload exceeds the size limit")
    if not inflater.eof or inflater.unused_data:
        raise RecordingError("compressed l2 payload is incomplete or has trailing data")
    return _json(payload)


def _market_frame(
    payload: bytes,
) -> tuple[str, tuple[tuple[int, int], ...], set[str]] | None:
    envelope = _object(_json(payload), "received frame")
    channel = envelope.get("channel")
    if not isinstance(channel, str):
        raise RecordingError("received frame channel must be a string")
    if channel not in CHANNELS:
        return None
    data = envelope.get("data")
    if channel == "trades":
        if not isinstance(data, list):
            raise RecordingError("trades data must be an array")
        counts: dict[int, int] = {}
        coins = set()
        for trade in data:
            coin, millis = _block(trade)
            coins.add(coin)
            counts[millis] = counts.get(millis, 0) + 1
        return channel, tuple(counts.items()), coins
    if channel == "l2":
        data = _object(data, "l2 data")
        variants = [name for name in ("s", "u", "c") if data.get(name) is not None]
        if len(variants) != 1:
            raise RecordingError("l2 data must have exactly one of s, u, c")
        variant = variants[0]
        if variant == "s":
            coin, millis = _block(data[variant])
        else:
            diff = _inflate_l2(data[variant]) if variant == "c" else data[variant]
            coin, millis = _block(diff, "c", "t")
    else:
        coin, millis = _block(data)
    return channel, ((millis, 0),), {coin}


def _file_header(record: Record) -> dict:
    metadata = _object(_json(record.payload), "file_open metadata")
    if record.kind != 1 or metadata.get("event") != "file_open":
        raise RecordingError("first record must be file_open metadata")
    _integer(metadata.get("format"), "format", 1, 1)
    source = metadata.get("source")
    if not isinstance(source, str) or not source:
        raise RecordingError("file_open source must be a nonempty string")
    _integer(
        metadata.get("process_start_wall_ns"), "process_start_wall_ns",
        -(1 << 63), (1 << 63) - 1,
    )
    return metadata


def iter_observations(paths: list[Path]) -> Iterator[Observation]:
    """Keep hourly continuity, splitting known gaps and recorder restarts.

    Trade memberships are aggregated within their physical frame. A batch
    spanning several block times therefore remains one observation. The first
    nonempty batch for an outgoing trade subscription is marked as replay.
    """
    process_start = None
    previous_hour = None
    previous_mono = None
    stream_id = 0
    pending_trade_coins: set[str] = set()
    for path in paths:
        records = iter_records(path)
        record = None
        try:
            record = next(records, None)
            if record is None:
                raise RecordingError("empty recording")
            header = _file_header(record)
            if header["source"] != "hl":
                continue
            hour = record.recv_wall_ns // HOUR_NS
            new_process = header["process_start_wall_ns"] != process_start
            missing_hour = previous_hour is not None and hour > previous_hour + 1
            if new_process or missing_hour:
                stream_id += 1
                previous_mono = None
                pending_trade_coins.clear()
            process_start = header["process_start_wall_ns"]
            previous_hour = hour
            while record is not None:
                event = None
                channel = None
                blocks = ()
                replay = False
                if previous_mono is not None and record.recv_mono_ns < previous_mono:
                    stream_id += 1
                previous_mono = record.recv_mono_ns
                if record.kind == 1:
                    metadata = _object(_json(record.payload), "metadata")
                    event = metadata.get("event")
                    if not isinstance(event, str):
                        raise RecordingError("metadata event must be a string")
                    if event == "file_open" and record.record_index != 1:
                        raise RecordingError("file_open metadata must be the first record")
                    if event in CONNECTION_EVENTS:
                        stream_id += 1
                        pending_trade_coins.clear()
                    elif event == "dropped":
                        _integer(metadata.get("count"), "dropped count", 0, (1 << 64) - 1)
                        stream_id += 1
                elif record.kind == 4:
                    try:
                        sent = _json(record.payload)
                    except RecordingError:
                        sent = None
                    if isinstance(sent, dict) and sent.get("method") == "subscribe":
                        subscription = sent.get("subscription")
                        if isinstance(subscription, dict) and subscription.get("type") == "trades":
                            coin = subscription.get("coin")
                            if isinstance(coin, str):
                                pending_trade_coins.add(coin)
                elif record.kind == 2:
                    market = _market_frame(record.payload)
                    if market is not None:
                        channel, blocks, coins = market
                        if channel == "trades" and coins:
                            replay = bool(coins & pending_trade_coins)
                            pending_trade_coins.difference_update(coins)
                if event is not None or channel is not None:
                    yield Observation(
                        path, stream_id, record.recv_wall_ns, record.recv_mono_ns,
                        channel, event, blocks, replay,
                    )
                record = next(records, None)
        except RecordingError as error:
            if error.path is not None:
                raise
            index = record.record_index if record is not None else 1
            raise RecordingError(str(error), path, index) from error
        finally:
            records.close()


def nearest_rank(ordered, per_mille):
    index = max(0, (len(ordered) * per_mille + 999) // 1000 - 1)
    return ordered[index]


def quantiles(values, units_per_ms=1):
    ordered = sorted(values)
    row = {"n": len(ordered)}
    for label, per_mille in PERCENTILES:
        value = nearest_rank(ordered, per_mille) if ordered else None
        if value is not None and units_per_ms != 1:
            value /= units_per_ms
        row[label] = value
    return row


def trade_band(count):
    if count < 2:
        return str(count)
    return "2-4" if count < 5 else "5+"


def add_intervals(by_trades, times_and_counts):
    for (earlier, trades), (later, _) in pairwise(times_and_counts):
        by_trades[trade_band(trades)].append(later - earlier)


def spacing_summary(raw, live):
    live_gaps = list(chain.from_iterable(live.values()))
    histogram = Counter(live_gaps)

    def mode(candidates):
        # On a frequency tie use the shorter interval.
        return max(candidates, key=lambda ms: (histogram[ms], -ms), default=None)

    cadence = mode(histogram)
    double_mode = mode(ms for ms in histogram if 3 * cadence <= 2 * ms < 5 * cadence)

    def missing(gaps):
        # Round each elapsed gap to a positive number of modal block periods.
        hidden = sum(max(0, (2 * gap + cadence) // (2 * cadence) - 1) for gap in gaps)
        steps = len(gaps) + hidden
        return {"inferred_missing": hidden, "missing_share": hidden / steps if steps else None}

    return {
        "all": quantiles(chain.from_iterable(raw.values())),
        "by_trades": {band: quantiles(gaps) for band, gaps in raw.items()},
        "live": quantiles(live_gaps),
        "live_by_trades": {band: quantiles(gaps) for band, gaps in live.items()},
        "live_histogram_ms": dict(sorted(histogram.items())),
        "cadence_ms": cadence, "double_mode_ms": double_mode, **missing(live_gaps),
        "missing_by_trades": {band: missing(gaps) for band, gaps in live.items()},
    }


class BlockTiming:
    """Accumulate across continuous recording segments without bridging a gap."""

    def __init__(self):
        self.raw_intervals = {band: [] for band in TRADE_BANDS}
        self.live_intervals = {band: [] for band in TRADE_BANDS}
        self.patterns = Counter()
        self.channel_runs = Counter()
        self.pairs = {pair: {"forward": 0, "reverse": 0, "mixed": 0}
                      for pair in combinations(CHANNELS, 2)}
        self.spreads = []
        self.interleaved = 0

    def finish_segment(self, blocks):
        raw_times = []
        live_times = []
        for time_ms, arrivals in sorted(blocks.items()):
            first, last = arrivals[0], arrivals[-1]
            self.spreads.append(last[1] - first[1])
            if last[0] - first[0] + 1 != len(arrivals):
                self.interleaved += 1
            first_index, last_index = {}, {}
            runs = []
            trades = 0
            live_trades = 0
            has_live = False
            for index, _, channel, count, replay in arrivals:
                first_index.setdefault(channel, index)
                last_index[channel] = index
                if not runs or channel != runs[-1]:
                    runs.append(channel)
                trades += count
                if not replay:
                    has_live = True
                    live_trades += count
            self.patterns[" > ".join(first_index)] += 1
            self.channel_runs[" > ".join(runs)] += 1
            for (a, b), counts in self.pairs.items():
                if a not in first_index or b not in first_index:
                    continue
                if last_index[a] < first_index[b]:
                    counts["forward"] += 1
                elif last_index[b] < first_index[a]:
                    counts["reverse"] += 1
                else:
                    counts["mixed"] += 1
            raw_times.append((time_ms, trades))
            if has_live:
                live_times.append((time_ms, live_trades))
        add_intervals(self.raw_intervals, raw_times)
        add_intervals(self.live_intervals, live_times)

    def order_summary(self, late):
        return {
            "blocks": len(self.spreads), "interleaved_blocks": self.interleaved,
            "contiguous_blocks": len(self.spreads) - self.interleaved,
            "spread_ms": quantiles(self.spreads, NS_PER_MS),
            "first_channel_patterns": dict(self.patterns),
            "channel_runs": dict(self.channel_runs),
            "pairs": {"/".join(pair): counts for pair, counts in self.pairs.items()},
            "late_frames": sum(late.values()), "late_by_channel": dict(late),
        }


def latency_summary(cells, activities):
    ordered = sorted(activities)
    bounds = [nearest_rank(ordered, p) for p in (200, 400, 600, 800)]
    labels = (*CHANNELS, "live_trades")
    overall = defaultdict(list)
    by_activity = [defaultdict(list) for _ in range(5)]
    by_hour = defaultdict(lambda: defaultdict(list))
    extremes = defaultdict(lambda: defaultdict(list))
    for (hour, activity, channel, replay), latencies in cells.items():
        bucket = bisect_left(bounds, activity)
        sample_labels = (channel, "live_trades") if channel == "trades" and not replay else (channel,)
        for label in sample_labels:
            overall[label].extend(latencies)
            by_activity[bucket][label].extend(latencies)
            by_hour[hour][label].extend(latencies)
            if bucket in (0, 4) and label != "trades":
                extremes[hour][label, bucket].extend(latencies)

    def rows(group):
        return {label: quantiles(group[label], NS_PER_MS) for label in labels}

    def extremes_by_channel(hour):
        result = {}
        for label in ("bbo", "l2", "live_trades"):
            tails = {}
            for name, bucket in (("q1", 0), ("q5", 4)):
                samples = sorted(extremes[hour][label, bucket])
                tails[f"{name}_n"] = len(samples)
                tails[f"{name}_ms"] = nearest_rank(samples, 990) / NS_PER_MS if samples else None
            result[label] = tails
        return result

    hours = {hour: datetime.fromtimestamp(hour * 3600, UTC).strftime("%Y-%m-%d %H:00")
             for hour in sorted(by_hour)}
    return {
        "by_channel": rows(overall), "activity_upper_bounds": bounds,
        "by_activity": [rows(group) for group in by_activity],
        "by_utc_hour": {name: rows(by_hour[hour]) for hour, name in hours.items()},
        "live_tail_by_hour": {name: extremes_by_channel(hour) for hour, name in hours.items()},
    }


def utc_timestamp(ns):
    seconds, fraction = divmod(ns, NS_PER_SECOND)
    return datetime.fromtimestamp(seconds, UTC).strftime("%Y-%m-%dT%H:%M:%S") + f".{fraction:09d}Z"


def analyze(root):
    paths = recording_paths(root)
    files = {}
    frame_counts = Counter(dict.fromkeys(CHANNELS, 0))
    negative = Counter(dict.fromkeys(CHANNELS, 0))
    late = Counter(dict.fromkeys(CHANNELS, 0))
    timing = BlockTiming()
    activities = []
    cells = defaultdict(list)
    pairs = identical = same_millisecond = segments = elapsed_ns = 0
    replay_frames = replay_memberships = mixed_frames = 0
    first_wall = last_wall = None

    for _, segment in groupby(iter_observations(paths), key=attrgetter("stream_id")):
        blocks = defaultdict(list)
        window = deque()
        tied_mono = 0
        frame_index = 0
        latest_block = -1
        first_mono = last_mono = previous_wall = None
        for obs in segment:
            if obs.event == "file_open":
                with obs.path.open("rb") as stored:
                    digest = hashlib.file_digest(stored, "sha256").hexdigest()
                files[obs.path] = {"path": obs.path.relative_to(root).as_posix(),
                                   "sha256": digest, "frames": 0}
            if obs.channel is None:
                continue
            wall, mono, channel = obs.recv_wall_ns, obs.recv_mono_ns, obs.channel
            if first_mono is None:
                first_mono = mono
                segments += 1
            else:
                pairs += 1
                identical += wall == previous_wall
                same_millisecond += wall // NS_PER_MS == previous_wall // NS_PER_MS
            first_wall = wall if first_wall is None else min(first_wall, wall)
            last_wall = wall if last_wall is None else max(last_wall, wall)

            # [t - 1 s, t). Earlier records stamped at t do not enter this frame's bucket.
            while window and window[0] < mono - NS_PER_SECOND:
                window.popleft()
            if mono != last_mono:
                tied_mono = 0
            activity = len(window) - tied_mono
            activities.append(activity)
            window.append(mono)
            tied_mono += 1
            files[obs.path]["frames"] += 1
            frame_counts[channel] += 1
            if obs.replay:
                replay_frames += 1
                replay_memberships += len(obs.blocks)
            mixed_frames += len(obs.blocks) > 1

            if any(time_ms < latest_block for time_ms, _ in obs.blocks):
                late[channel] += 1
            for time_ms, trades in obs.blocks:
                latest_block = max(latest_block, time_ms)
                latency = wall - time_ms * NS_PER_MS
                negative[channel] += latency < 0
                cells[wall // HOUR_NS, activity, channel, obs.replay].append(latency)
                blocks[time_ms].append((frame_index, mono, channel, trades, obs.replay))
            frame_index += 1
            last_mono, previous_wall = mono, wall
        if first_mono is not None:
            elapsed_ns += last_mono - first_mono
            timing.finish_segment(blocks)

    if not activities:
        raise RecordingError(f"no bbo, l2 or trades frames in complete hl recordings under {root}")
    return {
        "files": list(files.values()),
        "coverage": {
            "frames": len(activities), "frames_by_channel": dict(frame_counts),
            "segments": segments, "observed_hours": elapsed_ns / HOUR_NS,
            "first_receive_utc": utc_timestamp(first_wall), "last_receive_utc": utc_timestamp(last_wall),
            "replay_frames": replay_frames, "replay_memberships": replay_memberships,
            "mixed_time_frames": mixed_frames, "negative_latency_by_channel": dict(negative),
        },
        "spacing_ms": spacing_summary(timing.raw_intervals, timing.live_intervals),
        "order": timing.order_summary(late),
        "socket": {"pairs": pairs, "identical_wall_ns": identical, "same_wall_ms": same_millisecond},
        "latency_ms": latency_summary(cells, activities),
    }


def _report_number(value: int | float | None) -> str:
    """Keep aggregate tables readable while preserving small timing spreads."""
    if value is None:
        return "—"
    if isinstance(value, int):
        return f"{value:,}"
    return f"{value:,.3f}".rstrip("0").rstrip(".")


def _activity_labels(bounds: list[int]) -> list[str]:
    return [
        f"0–{bounds[0]}",
        *("empty" if lower == upper else f"{lower + 1}–{upper}"
          for lower, upper in zip(bounds, bounds[1:])),
        f"{bounds[-1] + 1}+",
    ]


def model_inputs(report: dict) -> list[str]:
    """Derive provisional replay rules from the measured aggregate evidence."""
    spacing = report["spacing_ms"]
    cadence = spacing["cadence_ms"]
    if cadence is None:
        spacing_input = (
            "Block spacing remains unsettled because the live sample contains "
            "too few distinct block times to estimate a cadence."
        )
    else:
        missing = spacing["missing_share"]
        missing_text = "unavailable" if missing is None else f"{missing:.2%}"
        spacing_input = (
            f"Use {cadence} ms as the provisional nominal spacing for unseen blocks, "
            f"with a visible live median of {_report_number(spacing['live']['p50'])} ms "
            f"and an inferred missing share of {missing_text}."
        )

    order = report["order"]
    majority_constraints = []
    for pair, counts in order["pairs"].items():
        first, second = pair.split("/")
        total = counts["forward"] + counts["reverse"] + counts["mixed"]
        if counts["forward"] * 2 > total:
            majority_constraints.append((first, second))
        elif counts["reverse"] * 2 > total:
            majority_constraints.append((second, first))
    compatible_orders = [
        candidate for candidate in permutations(CHANNELS)
        if all(candidate.index(first) < candidate.index(second)
               for first, second in majority_constraints)
    ]
    if len(compatible_orders) == 1:
        channel_order = " → ".join(compatible_orders[0])
        order_input = (
            f"Preserve file order for recorded feeds and use {channel_order} as the "
            f"empirical default for archive replay, allowing the measured exceptions "
            f"({_report_number(order['interleaved_blocks'])} interleaved blocks and "
            f"{_report_number(order['late_frames'])} late frames)."
        )
    else:
        order_input = (
            "Preserve file order for recorded feeds while the sample leaves a complete "
            "three-channel ordering rule for archive replay unsettled."
        )

    latency = report["latency_ms"]
    bounds = latency["activity_upper_bounds"]
    buckets = latency["by_activity"]
    live_channels = (("bbo", "bbo"), ("l2", "l2"), ("trades", "live_trades"))
    supported = []
    for channel, key in live_channels:
        rows = [bucket[key] for bucket in buckets]
        if min(row["n"] for row in rows) < 100:
            continue
        tails = [row["p99"] for row in rows]
        change = tails[-1] - tails[0]
        if change == 0 or any((b - a) * change < 0 for a, b in pairwise(tails)):
            continue
        hours = [hour[key] for hour in latency["live_tail_by_hour"].values()
                 if min(hour[key]["q1_n"], hour[key]["q5_n"]) >= 100]
        if len(hours) >= 2 and all((h["q5_ms"] - h["q1_ms"]) * change > 0 for h in hours):
            supported.append(channel)
    if supported:
        latency_input = (
            f"Sample {', '.join(supported)} latency separately in preceding-second frame-count "
            f"bins {' / '.join(_activity_labels(bounds))}. Each channel's p99 trend across "
            "the five bins also appears within individual UTC hours."
        )
    else:
        latency_input = (
            "The sample does not settle activity conditioning. No channel shows a consistent "
            "p99 trend across five populated bins and at least two UTC hours."
        )
    return [spacing_input, order_input, latency_input]


def render_markdown(report: dict) -> str:
    """Render a self-contained aggregate report without retaining market frames."""
    lines = ["# Hyperliquid feed timing", ""]
    coverage = report["coverage"]
    spacing = report["spacing_ms"]
    order = report["order"]
    latency = report["latency_ms"]
    socket = report["socket"]
    quantile_keys = ["n", *(label for label, _ in PERCENTILES)]

    def channel_rows(group):
        for channel in CHANNELS:
            yield channel, group[channel]
        if group["trades"]["n"] != group["live_trades"]["n"]:
            yield "live trades", group["live_trades"]

    def table(headers: list[str], rows: list[list[str]]) -> None:
        lines.append("| " + " | ".join(headers) + " |")
        lines.append("| " + " | ".join("---" for _ in headers) + " |")
        lines.extend("| " + " | ".join(row) + " |" for row in rows)
        lines.append("")

    def quantile_table(label: str, rows: Iterable[tuple[str, dict]]) -> None:
        table(
            [label, "N", "Min", "p1", "p10", "p50", "p90", "p99", "p99.9", "Max"],
            [[name, *(_report_number(row[key]) for key in quantile_keys)] for name, row in rows],
        )

    lines.extend([
        f"Script commit `{report['script_commit']}`. The input contains "
        f"{len(report['files'])} complete files, {_report_number(coverage['frames'])} "
        f"market frames and {_report_number(coverage['segments'])} continuous segments, "
        f"covering {_report_number(coverage['observed_hours'])} observed hours from "
        f"{coverage['first_receive_utc']} to {coverage['last_receive_utc']}.",
        "",
        "## Method and coverage",
        "",
        "The sample uses bbo, l2 and trades. Observed hours sum first-to-last frame "
        "spans within continuous segments. Hourly files retain continuity, while "
        "recorded connection gaps and process restarts split segments.",
        "",
        "Each physical frame contributes once to activity and socket-read statistics. "
        "A trade frame contributes once per distinct block time to block and latency "
        "measurements. The spacing split counts individual trades in the leading block. "
        "Quantiles use nearest rank and timing tables use milliseconds.",
        "",
        f"The first nonempty trade batch following each recorded trade subscription is "
        f"treated as recent-history replay, covering {_report_number(coverage['replay_frames'])} "
        f"frames and {_report_number(coverage['replay_memberships'])} frame/block memberships. "
        f"The sample has {_report_number(coverage['mixed_time_frames'])} trade frames with "
        "multiple block times. Rows labelled live omit those replay batches.",
        "",
        "Latency is receive wall time minus exchange block time. Activity counts market "
        "frames in the monotonic interval [t−1 second, t), excluding all timestamp ties "
        "at t. Block receive spread uses last minus first monotonic time. Record order "
        "resolves arrival ties.",
        "",
    ])
    table(
        ["Channel", "Frames", "Frames behind the latest block", "Negative latency observations"],
        [[channel, _report_number(coverage["frames_by_channel"][channel]),
          _report_number(order["late_by_channel"][channel]),
          _report_number(coverage["negative_latency_by_channel"][channel])]
         for channel in CHANNELS],
    )
    lines.extend([
        "Negative delays are retained. Absolute one-way delay calibration is unresolved.",
        "",
        "## Block spacing",
        "",
    ])
    quantile_table("Sample", [("All intervals", spacing["all"]), ("Live intervals", spacing["live"])])
    quantile_table(
        "Trades in preceding block",
        [(f"{sample} / {count}", rows[count])
         for sample, rows in (("All", spacing["by_trades"]), ("Live", spacing["live_by_trades"]))
         for count in ("0", "1", "2-4", "5+")],
    )
    missing_share = spacing["missing_share"]
    share_text = "unavailable" if missing_share is None else f"{missing_share:.2%}"
    cadence_text = (
        "The live modal cadence is unavailable."
        if spacing["cadence_ms"] is None
        else f"The live modal cadence is {_report_number(spacing['cadence_ms'])} ms."
    )
    second_mode_text = (
        "No live interval lies between 1.5 and 2.5 nominal periods."
        if spacing["double_mode_ms"] is None
        else f"The most frequent gap in the 1.5–2.5-period band is {spacing['double_mode_ms']} ms."
    )
    lines.extend([
        f"{cadence_text} {second_mode_text} "
        f"Rounding each live interval to the nearest positive multiple of the modal "
        f"cadence gives {_report_number(spacing['inferred_missing'])} inferred missing "
        f"block steps and a missing share of {share_text}. The denominator is the sum "
        "of inferred block steps. The shortest interval wins a modal tie. This estimates "
        "visibility within the recorded instruments from the assumed cadence.",
        "",
    ])
    table(
        ["Trades in preceding live block", "Inferred missing steps", "Missing share"],
        [[count, _report_number(row["inferred_missing"]),
          "—" if row["missing_share"] is None else f"{row['missing_share']:.2%}"]
         for count, row in spacing["missing_by_trades"].items()],
    )
    lines.extend([
        "## Delivery order and socket reads",
        "",
        f"Across {_report_number(order['blocks'])} recorded block groups, "
        f"{_report_number(order['contiguous_blocks'])} arrive as a contiguous run and "
        f"{_report_number(order['interleaved_blocks'])} are interrupted by other market frames. "
        f"{_report_number(order['late_frames'])} frames contain a block time older than "
        "the latest block already received in their segment.",
        "",
    ])
    lines.extend([
        "Channel patterns collapse adjacent repeats so several instruments on one "
        "channel form one run, while later returns to a channel remain visible.",
        "",
    ])
    table(["Channel-run pattern", "Block groups"], [
        [pattern, _report_number(count)]
        for pattern, count in sorted(order["channel_runs"].items(), key=lambda item: (-item[1], item[0]))
    ])
    table(["Channel pair A / B", "All A before all B", "All B before all A", "Mixed"], [
        [pair, *(_report_number(counts[key]) for key in ("forward", "reverse", "mixed"))]
        for pair, counts in order["pairs"].items()
    ])
    lines.extend([
        "The proposed channel order must satisfy pairwise majorities of these full "
        "channel ranges, with mixed groups included in each denominator, and yield "
        "one compatible permutation.",
        "",
    ])
    quantile_table("Within-block receive spread", [("All block groups", order["spread_ms"])])
    pairs = socket["pairs"]

    def pair_count(count):
        return f"{count:,} ({count / pairs:.2%})" if pairs else "0 (no pairs)"

    lines.extend([
        f"Among {pairs:,} consecutive market-frame pairs, {pair_count(socket['identical_wall_ns'])} "
        f"share an exact receive timestamp and {pair_count(socket['same_wall_ms'])} fall in the "
        "same millisecond. The recorder stamps each WebSocket frame individually. Socket-read "
        "boundaries cannot be recovered from those stamps.",
        "",
        "## Latency",
        "",
    ])
    quantile_table("Channel", channel_rows(latency["by_channel"]))
    bounds = latency["activity_upper_bounds"]
    labels = _activity_labels(bounds)
    lines.extend([
        "Activity boundaries are the pooled physical-frame 20th, 40th, 60th and 80th "
        f"percentiles, with inclusive upper bounds {', '.join(map(str, bounds))}. "
        "Equal counts stay in the same bin. A separate live-trades row appears only "
        "when the group contains subscription history.",
        "",
    ])
    quantile_table("Activity bin / channel", [
        (f"Q{index + 1} ({labels[index]}) / {label}", row)
        for index, bucket in enumerate(latency["by_activity"])
        for label, row in channel_rows(bucket)
    ])
    quantile_table("UTC receive hour / channel", [
        (f"{hour} / {label}", row)
        for hour, rows in sorted(latency["by_utc_hour"].items())
        for label, row in channel_rows(rows)
    ])
    lines.extend([
        "Within-hour live p99 comparisons use the same pooled activity boundaries. "
        "Latency cells show Q1 → Q5 in milliseconds. The smallest extreme count "
        "covers both bins across the three channels.",
        "",
    ])
    table(["UTC receive hour", "Smallest extreme N", "bbo", "l2", "live trades"], [
        [hour, _report_number(min(
            rows[channel][count]
            for channel in ("bbo", "l2", "live_trades")
            for count in ("q1_n", "q5_n")
        )), *(
            f"{_report_number(rows[channel]['q1_ms'])} → {_report_number(rows[channel]['q5_ms'])}"
            for channel in ("bbo", "l2", "live_trades")
        )]
        for hour, rows in sorted(latency["live_tail_by_hour"].items())
    ])
    lines.extend([
        "A channel supports conditioning here when p99 moves consistently across the five "
        "bins and its Q1-to-Q5 direction holds in each sampled hour (at least two). "
        "Comparisons use bins with at least 100 observations for p99. These recordings "
        "measure the association with received traffic over this period.",
        "",
        "## Input files",
        "",
    ])
    table(["Path", "Market frames", "SHA-256"], [
        [f"`{str(item['path']).replace('|', '&#124;')}`", _report_number(item["frames"]), f"`{item['sha256']}`"]
        for item in report["files"]
    ])
    lines.extend(["## Inputs for the time model", ""])
    lines.extend(f"- {sentence}" for sentence in report["model_inputs"])
    return "\n".join(lines) + "\n"


def script_commit():
    """Identify the commit containing this script, including after a report-only commit."""
    script = Path(__file__).resolve()
    git = ["git", "-C", str(script.parent)]
    try:
        revision = subprocess.check_output(
            [*git, "log", "-1", "--format=%H", "--", script.name], text=True,
            stderr=subprocess.DEVNULL,
        ).strip()
        if revision and not subprocess.run(
            [*git, "diff", "--quiet", revision, "--", script.name], check=False,
        ).returncode:
            return revision
    except (OSError, subprocess.CalledProcessError):
        pass
    return "uncommitted or unavailable"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path,
                        help="directory of complete *.hlrec.zst recordings (write both reports outside it)")
    parser.add_argument("--json", type=Path, default=Path("feed-timing.json"),
                        help="aggregate JSON destination (default feed-timing.json)")
    parser.add_argument("--markdown", type=Path, default=Path("feed-timing.md"),
                        help="Markdown report destination (default feed-timing.md)")
    args = parser.parse_args()
    root = args.root.resolve()
    destinations = (args.json.resolve(), args.markdown.resolve())
    if not root.is_dir():
        parser.error(f"input directory does not exist: {root}")
    if destinations[0] == destinations[1]:
        parser.error("JSON and Markdown need distinct output paths")
    for path in destinations:
        if path.is_relative_to(root) or ".hlrec." in path.name or path.is_dir():
            parser.error(f"invalid report destination: {path}")
    try:
        report = analyze(root)
        report["script_commit"] = script_commit()
        report["model_inputs"] = model_inputs(report)
        outputs = (json.dumps(report, indent=2, sort_keys=True) + "\n", render_markdown(report))
        # Stage both completed reports before replacing earlier results.
        with ExitStack() as cleanup:
            staged = []
            for path, content in zip(destinations, outputs):
                path.parent.mkdir(parents=True, exist_ok=True)
                with tempfile.NamedTemporaryFile(dir=path.parent, mode="w", encoding="utf-8",
                                                 delete=False) as output:
                    stage = Path(output.name)
                    cleanup.callback(stage.unlink, missing_ok=True)
                    output.write(content)
                staged.append(stage)
            for stage, dest in zip(staged, destinations):
                stage.replace(dest)
    except (OSError, RecordingError) as error:
        parser.exit(1, f"{parser.prog}: {error}\n")


if __name__ == "__main__":
    main()
