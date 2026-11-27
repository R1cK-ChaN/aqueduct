# RFC 0002: Raw recording format and recorder behaviour

- Status: implemented in `hl-recorder` v0; owner review pending
- Date: 2026-11-24
- Crates: `hl-codec` (the record format), `hl-recorder` (the process)

The recorder's files are the source of truth for everything we record
ourselves. The documented official archive omits incremental book and BBO
streams, so our replay depends on recording them.
[HL-102](../venues/hyperliquid.md#hl-102)

The format must therefore be simple to write correctly, lose nothing, and
stay readable for years.

## Decisions

### D1. The recorder stores frames, not meaning

The recorder writes every WebSocket frame exactly as received, with two
receive timestamps, and never parses a payload. A parser bug then costs a
re-run of the loader, not data. The encoded `l2` and `fastAssetCtxs` payloads
are stored as they arrive. Decoding is `hl-codec`'s job at load time.
[HL-68](../venues/hyperliquid.md#hl-68), [HL-92](../venues/hyperliquid.md#hl-92)

Venue details live in the configuration file, not in code: URL, the text of
each subscription message, the ping text and its interval. A change such as a
renamed subscription field is a config edit, not a new build.

### D2. Record layout

A recording, once decompressed, is a sequence of records. All integers are
little-endian.

| Offset | Size | Field | Meaning |
| --- | --- | --- | --- |
| 0 | 4 | `payload_len` | `u32`, length of the payload in bytes |
| 4 | 1 | `kind` | `u8`, see below |
| 5 | 8 | `recv_wall_ns` | `i64`, wall clock at receipt, nanoseconds since the Unix epoch |
| 13 | 8 | `recv_mono_ns` | `u64`, monotonic clock, nanoseconds since the recorder process started |
| 21 | `payload_len` | `payload` | the bytes |

| Kind | Name | Payload |
| --- | --- | --- |
| 1 | `Meta` | JSON object written by the recorder; always has an `event` field |
| 2 | `RecvText` | a text frame received from the venue, UTF-8 |
| 3 | `RecvBinary` | a binary frame received from the venue |
| 4 | `SentText` | a text frame the recorder sent: subscriptions and pings |

A reader must reject an unknown kind. New kinds get new numbers; existing
numbers never change meaning. The first record of every file is a `Meta`
record with `"event": "file_open"`. Its payload includes `format`,
`source`, `url`, `recorder_version`, `process_start_wall_ns` and
`file_seq`. Together, `source` and `process_start_wall_ns` identify the
files written for one source by one recorder process. Within that chain,
`file_seq` is an unsigned 64-bit integer starting at 0 and increasing by 1
each time the writer opens another file. Each source has its own counter.
This lets a reader detect a missing file even if the clock steps.

Earlier recordings omit `file_seq`. The field is additive, readers ignore
unknown header fields, and the binary layout and `"format": 1` still apply.
Rotation only opens a file. Connections, subscriptions and their lifecycle
events continue across the boundary.

The wall clock records raw receipt time and can step when NTP corrects it.
The monotonic clock advances within one process run. Replay uses the
`file_open` process anchor to relate the two stamps and derives logical
`ts_local` as specified in RFC 0008 D4.
[RFC 0008 D4](0008-paper-trading.md#d4-close-a-timestamp-before-firing-its-deadlines)

Rejected alternative: JSON Lines with the frame embedded as a string. It is
greppable, but every quote in every frame is escaped on write and unescaped
on read, and a frame that is not valid UTF-8 needs a second encoding. Length
prefixes keep the bytes exact. `hl-recorder inspect` prints a file as text
when a person needs to look.

### D3. One compressed file per source per hour

```text
<out_dir>/<source>/<YYYYMMDD>/<source>-<YYYYMMDD>T<HH>-<open_ms>.hlrec.zst
```

- The hour is the UTC hour of `recv_wall_ns`. `open_ms` is the wall-clock
  millisecond at which the file was opened; it keeps names unique when the
  process restarts within an hour or the clock steps back.
- Each file is one zstd frame with a content checksum.
- While it is being written a file has the extra suffix `.part`. When its
  hour ends, or on clean shutdown, the frame is finished, the file is synced
  and renamed. Uploaders and loaders take files without `.part`.
- The compressor is flushed about once a second, so a killed process loses at
  most about a second of data. The partial file left behind is a truncated
  zstd frame: readable up to the last flush, then an error. On the next start
  the recorder renames such files to `...truncated.hlrec.zst`, so that
  readers know to expect the truncation.

### D4. Never block the socket on the disk

One thread per source reads the socket and does nothing else: it stamps each
frame on receipt and pushes it into a bounded queue. One writer thread drains
the queue, compresses and writes. If the queue is full the frame is dropped
and counted; the count is written as a `Meta` record (`"event": "dropped"`)
so the gap is visible in the data itself. A slow disk must not delay
timestamps or stall the connection.

### D5. Connection lifecycle is recorded in the same file

`Meta` events `connecting`, `connected`, `connect_failed`, `disconnected`
(with a reason) and `shutdown` sit in the stream at the moment they happen.
The loader derives gap intervals from them without a separate log.

A connection is considered stale when no frame arrives for `stale_after_ms`
(default 3000). The recorder then drops it and reconnects with exponential
backoff from 0.5 s to 30 s; the backoff resets after a connection has stayed
up for 30 s. Subscriptions are sent again after every reconnect, paced by
`subscribe_pace_ms`, while frames already arriving are read and stamped.

## Not in v0

- **Upload to object storage.** Upload runs outside the recorder. An external
  sync job can copy sealed files by their stable names.
- **A second, redundant market-data connection** and de-duplication.
- **Metrics endpoint and alerts.** v0 logs JSON lines to stderr.
- **REST polling** of `meta`, `perpDexs` and book checks.

## Verification

- Unit tests: record round trip, truncated input, unknown kind, hour rotation,
  file naming, recovery of leftover `.part` files, drop accounting, config
  validation.
- An integration test starts a WebSocket server on localhost, lets the
  recorder subscribe, receive frames and survive a server-side disconnect,
  then decodes the files and checks every frame is present in order.
- The results of the short manual runs against the public Hyperliquid and
  Binance feeds are in the [recorder feed measurements](../measurements/2026-11-24-recorder-feeds.md).
