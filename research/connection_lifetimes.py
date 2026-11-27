#!/usr/bin/env python3
"""Measure WebSocket connection lifetimes in complete RFC 0002 recordings."""

import argparse
from collections import Counter, defaultdict
from contextlib import ExitStack, closing
from datetime import UTC, datetime
import hashlib
from html import escape
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile

# Isolated Python omits the script directory. Import the reader beside this file.
sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from feed_timing import (
    NS_PER_SECOND, RecordingError, _file_header, iter_records, recording_paths, utc_timestamp,
)


OPEN_END = "open at end of input"
PROCESS_END = "process ended without shutdown"
SERVER_CLOSE = "closed by the server: "


def utc_nanoseconds(value):
    match = re.fullmatch(
        r"(\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2})(?:\.(\d{1,9}))?(?:Z|\+00:00)", value,
    )
    if not match:
        raise argparse.ArgumentTypeError("use a UTC timestamp such as 2026-11-27T07:00:00Z")
    try:
        seconds = datetime.fromisoformat(match[1]).replace(tzinfo=UTC)
    except ValueError as error:
        raise argparse.ArgumentTypeError(str(error)) from error
    return int(seconds.timestamp()) * NS_PER_SECOND + int((match[2] or "").ljust(9, "0"))


def group_files(root, before):
    """Read just the headers to select files and put each process in monotonic order."""
    groups = defaultdict(list)
    for path in recording_paths(root):
        with closing(iter_records(path)) as records:
            first = next(records, None)
            if first is None:
                raise RecordingError("empty recording", path)
            try:
                header = _file_header(first)
            except RecordingError as error:
                raise RecordingError(str(error), path, 1) from error
            if before is not None and first.recv_wall_ns >= before:
                continue
            key = (header["source"], header["process_start_wall_ns"])
            groups[key].append((first.recv_mono_ns, path))
    return groups


def is_ping(text):
    try:
        message = json.loads(text)
    except ValueError:
        return text.strip().lower() == "ping"
    return isinstance(message, dict) and message.get("method") == "ping"


def process_connections(label, source, start, paths, unfinished, sent_messages):
    """Pair lifecycle records in stream order, carrying the open across rotation."""
    rows = []
    connected = None

    def finish(end, kind):
        lifetime = end.recv_mono_ns - connected.recv_mono_ns
        if lifetime < 0:
            raise RecordingError("connection end precedes its monotonic start", path, end.record_index)
        rows.append({
            "label": label, "source": source, "process_start_wall_ns": start,
            "connected_wall_ns": connected.recv_wall_ns, "end_wall_ns": end.recv_wall_ns,
            "end_kind": kind, "lifetime_ns": lifetime,
        })

    for path in paths:
        with closing(iter_records(path)) as records:
            for record in records:
                last = record
                if record.kind == 4:
                    text = record.payload.decode("utf-8")
                    if not is_ping(text):
                        sent_messages.add(text)
                elif record.kind == 1:
                    metadata = json.loads(record.payload)
                    if not isinstance(metadata, dict) or not isinstance(metadata.get("event"), str):
                        raise RecordingError("expected a metadata event", path, record.record_index)
                    event = metadata["event"]
                    if event == "connected":
                        if connected is not None:
                            raise RecordingError("connected twice without an end", path, record.record_index)
                        connected = record
                    elif connected is not None and event in ("disconnected", "shutdown"):
                        kind = metadata.get("reason") if event == "disconnected" else "shutdown"
                        if not isinstance(kind, str):
                            raise RecordingError("missing disconnect reason", path, record.record_index)
                        finish(record, kind)
                        connected = None
    if connected is not None:
        finish(last, unfinished)
    return rows


def analyze(label, root, before):
    groups = group_files(root, before)
    # A later process for the same source bounds the previous process's observations.
    latest = {source: start for source, start in sorted(groups)}
    files = []
    connections = []
    sent_messages = set()
    for (source, start), group in sorted(groups.items()):
        paths = [path for _, path in sorted(group)]
        unfinished = OPEN_END if start == latest[source] else PROCESS_END
        connections.extend(process_connections(label, source, start, paths, unfinished, sent_messages))
        for path in paths:
            with path.open("rb") as stored:
                digest = hashlib.file_digest(stored, "sha256").hexdigest()
            files.append({"path": path.relative_to(root).as_posix(), "sha256": digest})

    counts = Counter(row["end_kind"] for row in connections)
    server = defaultdict(list)
    for row in connections:
        if row["end_kind"].startswith(SERVER_CLOSE):
            server[row["end_kind"]].append(row["lifetime_ns"])
    server_closes = {}
    for kind, ages in sorted(server.items()):
        ages.sort()
        server_closes[kind] = {"lifetimes_ns": ages, "min_ns": ages[0], "max_ns": ages[-1]}
    return {
        "files": files, "connections": connections, "end_counts": dict(sorted(counts.items())),
        "server_closes": server_closes, "sent_messages": sorted(sent_messages),
    }


def duration(ns):
    seconds = (ns + NS_PER_SECOND // 2) // NS_PER_SECOND
    hours, remainder = divmod(seconds, 3600)
    minutes, seconds = divmod(remainder, 60)
    return f"{hours:02d}:{minutes:02d}:{seconds:02d}"


def table(headers, rows):
    yield "| " + " | ".join(headers) + " |"
    yield "| " + " | ".join("---" for _ in headers) + " |"
    for row in rows:
        cells = (escape(str(cell), quote=False).replace("|", "&#124;").replace("\n", "&#10;")
                 .replace("\r", "&#13;") for cell in row)
        yield "| " + " | ".join(cells) + " |"
    yield ""


def render_markdown(report):
    lines = ["# WebSocket connection lifetimes", "", f"Script commit `{report['script_commit']}`.", ""]
    if report["before_utc"]:
        lines.extend([f"Includes complete files with `file_open` before `{report['before_utc']}`.", ""])
    lines.extend([
        "Wall times are UTC. Lifetimes use the process monotonic clock and are rounded to the nearest",
        "second (HH:MM:SS). For a process ending without shutdown or a connection open at the end",
        "of the input, the last record gives the observation end and a lower bound on the lifetime.",
        "",
    ])
    for label, sample in report["labels"].items():
        rows = sample["connections"]
        lines.extend([f"## {label}", "",
                      f"{len(sample['files'])} files, {len(rows)} connections, "
                      f"{len(sample['sent_messages'])} distinct non-ping sent messages.", "",
                      "### End kinds", ""])
        lines.extend(table(["End kind", "Connections"], sample["end_counts"].items()))
        lines.extend(["### Server closes", ""])
        if sample["server_closes"]:
            lines.extend(table(["End kind", "Minimum", "Maximum", "Lifetimes in ascending order"], (
                [kind, duration(stats["min_ns"]), duration(stats["max_ns"]),
                 ", ".join(map(duration, stats["lifetimes_ns"]))]
                for kind, stats in sample["server_closes"].items()
            )))
        else:
            lines.extend(["No server close was recorded.", ""])
        lines.extend(["### Connections", ""])
        lines.extend(table(
            ["Source", "Process start (UTC)", "Connected (UTC)", "End / last seen (UTC)", "End kind", "Lifetime"],
            ([row["source"], utc_timestamp(row["process_start_wall_ns"]), utc_timestamp(row["connected_wall_ns"]),
              utc_timestamp(row["end_wall_ns"]), row["end_kind"], duration(row["lifetime_ns"])] for row in rows),
        ))
        lines.extend(["### Input files", ""])
        lines.extend(table(["Path relative to label root", "SHA-256"], (
            [item["path"], item["sha256"]] for item in sample["files"]
        )))
    return "\n".join(lines)


def script_commit():
    script = Path(__file__).resolve()
    git = ["git", "-C", str(script.parent)]
    try:
        commit = subprocess.check_output([*git, "log", "-1", "--format=%H", "--", script.name],
                                         stderr=subprocess.DEVNULL, text=True).strip()
        if commit and subprocess.run([*git, "diff", "--quiet", commit, "--", script.name]).returncode == 0:
            return commit
    except (OSError, subprocess.CalledProcessError):
        pass
    return "uncommitted or unavailable"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("inputs", nargs="+", metavar="LABEL=ROOT", help="label and directory of recordings")
    parser.add_argument("--before", type=utc_nanoseconds, help="exclusive UTC file_open cutoff")
    parser.add_argument("--json", type=Path, default=Path("connection-lifetimes.json"))
    parser.add_argument("--markdown", type=Path, default=Path("connection-lifetimes.md"))
    args = parser.parse_args()
    roots = {}
    for item in args.inputs:
        label, _, directory = item.partition("=")
        if not label or not directory or label in roots:
            parser.error("each input must be LABEL=ROOT with a distinct, nonempty label")
        root = Path(directory).resolve()
        if not root.is_dir():
            parser.error(f"input directory does not exist: {root}")
        roots[label] = root
    destinations = (args.json.resolve(), args.markdown.resolve())
    if destinations[0] == destinations[1]:
        parser.error("JSON and Markdown need distinct output paths")
    for path in destinations:
        if path.is_dir() or ".hlrec." in path.name or any(path.is_relative_to(root) for root in roots.values()):
            parser.error(f"invalid report destination: {path}")
    try:
        report = {
            "script_commit": script_commit(),
            "before_utc": utc_timestamp(args.before) if args.before is not None else None,
            "labels": {label: analyze(label, root, args.before) for label, root in roots.items()},
        }
        outputs = (json.dumps(report, indent=2, sort_keys=True) + "\n", render_markdown(report))
        with ExitStack() as cleanup:
            staged = []
            for path, content in zip(destinations, outputs, strict=True):
                path.parent.mkdir(parents=True, exist_ok=True)
                with tempfile.NamedTemporaryFile(dir=path.parent, mode="w", encoding="utf-8", delete=False) as out:
                    stage = Path(out.name)
                    cleanup.callback(stage.unlink, missing_ok=True)
                    out.write(content)
                staged.append(stage)
            for stage, path in zip(staged, destinations, strict=True):
                stage.replace(path)
    except (OSError, ValueError) as error:
        parser.exit(1, f"{parser.prog}: {error}\n")


if __name__ == "__main__":
    main()
