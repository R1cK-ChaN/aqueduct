# WebSocket connection lifetimes, 2026-11-24 to 2026-11-27

How long after connecting did the Hyperliquid public WebSocket send a close
with code 1000 and reason `Expired`? How did the other observed connections end?

Redacted on 2026-11-27 to remove the workstation location and recorder symbol names.

## Sites and inputs

The sample contains 94 complete files for source `hl` from a recorder in
Tokyo and two paper runs on a workstation outside the recording region.
The times below show the first `connected` and the last observation for each label, in UTC.
The report lists every input path relative to its label's root, the SHA-256
of its compressed bytes, and the script revision. [Input manifest and connection rows](../../research/reports/connection-lifetimes-2026-11.md)

| Label | Recording or paper run | Files | Connections | First connected (UTC) | Last observed (UTC) |
| --- | --- | ---: | ---: | --- | --- |
| tokyo-recorder | Tokyo recorder | 75 | 35 | Nov 24 07:08:09 | Nov 27 06:59:59 |
| workstation-paper-1 | `1795680047189612964-0000` | 4 | 2 | Nov 26 08:00:47 | Nov 26 11:00:47 |
| workstation-paper-2 | `1795695429795949524-0000` | 15 | 5 | Nov 26 12:17:09 | Nov 27 02:00:00 |

The Tokyo inputs start with `hl-20261124T07-1795504088995.hlrec.zst` and
end with `hl-20261127T06-1795759200074.hlrec.zst`.

## Method

From the repository root, set `TOKYO_RECORDER_ROOT` to the Tokyo source
directory containing the dated subdirectories, and `PAPER_ROOT` to the
parent of the two named paper runs. The command needs Python 3.12 and
the `zstd` executable. [Measurement script](../../research/connection_lifetimes.py)

```sh
python3 -I research/connection_lifetimes.py \
  "tokyo-recorder=$TOKYO_RECORDER_ROOT" \
  "workstation-paper-1=$PAPER_ROOT/1795680047189612964-0000" \
  "workstation-paper-2=$PAPER_ROOT/1795695429795949524-0000" \
  --before 2026-11-27T07:00:00Z \
  --json target/connection-lifetimes.json \
  --markdown research/reports/connection-lifetimes-2026-11.md
```

Select each complete file whose `file_open` receive wall time precedes the
cutoff, and read the whole file. Within each label, group files by source
and `process_start_wall_ns` and follow each connection across hourly rotation.
Start a row at `connected`. End it at `disconnected`, keeping the recorded
reason verbatim, or at `shutdown`. Subtract the two receive monotonic
timestamps within the process. The report displays UTC wall times and
rounds lifetimes to the nearest second. [RFC 0002 D5](../rfc/0002-raw-recording-format.md#d5-connection-lifecycle-is-recorded-in-the-same-file)

For a connection with no recorded end, the last record from its process
gives a lower bound on its lifetime. A later process for the same source
distinguishes a process ending without `shutdown` from a connection still
open at the end of the input.

### Sent subscriptions

The Tokyo files contain 45 distinct non-ping sent texts. The recorder sent
each of these four messages for each of the 11 symbols, substituting the
symbol for `{symbol}`.

```text
{"method":"subscribe","subscription":{"type":"l2","c":"{symbol}"}}
{"method":"subscribe","subscription":{"type":"bbo","coin":"{symbol}"}}
{"method":"subscribe","subscription":{"type":"trades","coin":"{symbol}"}}
{"method":"subscribe","subscription":{"type":"activeAssetCtx","coin":"{symbol}"}}
```

Tokyo also sent `{"method":"subscribe","subscription":{"type":"fastAssetCtxs"}}`.
Each workstation run sent two distinct non-ping messages.

```text
{"method":"subscribe","subscription":{"type":"bbo","coin":"BTC"}}
{"method":"subscribe","subscription":{"type":"trades","coin":"BTC"}}
```

The counts use distinct outgoing texts across connections within each label.

## Results

| Recorded end or observation boundary | Tokyo recorder | Workstation paper run 1 | Workstation paper run 2 |
| --- | ---: | ---: | ---: |
| `closed by the server: 1000 Expired` | 24 | 0 | 3 |
| `WebSocket protocol error: Connection reset without closing handshake` | 4 | 0 | 0 |
| `IO error: Connection reset by peer (os error 104)` | 2 | 0 | 0 |
| `no frame for 3000 ms` | 1 | 1 | 1 |
| `shutdown` | 3 | 1 | 1 |
| Process ended without shutdown | 0 | 0 | 0 |
| Open at end of input | 1 | 0 | 0 |

In Tokyo, 20 of the 24 `Expired` closes came after 2 h 46 min 53 s to
3 h 02 min 19 s. The other four came after 7 min 57 s, 14 min 32 s,
16 min 32 s and 23 min 15 s. Those four connections and all six reset
connections fall between Nov 24 10:54 and Nov 25 11:51 UTC. Starting with
the connection opened on Nov 25 at 11:50:36 UTC, 14 consecutive connections
ended with `Expired`, all in the longer range. The following connection
was still open at the sample boundary.

Workstation paper run 1 recorded a frame timeout after 37 min 43 s and no
`Expired` close. Paper 2 recorded three `Expired` closes, after
2 h 49 min 17 s, 2 h 49 min 43 s and 2 h 53 min 06 s, and a frame timeout
after 2 h 50 min 05 s. Both runs ended with `shutdown`.

## Limits

These observations cover about three days at two sites, with one client IP
address per site. The recordings identify the server's stated reason.
The cause of the four short closes and whether the venue enforces a fixed
connection age remain unknown. [HL-149](../venues/hyperliquid.md#hl-149)

## Register entry that cites this note

- [HL-150](../venues/hyperliquid.md#hl-150)
