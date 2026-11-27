# hl-publish seal

Check one UTC day of staged public recordings and build its local publication manifest. Run against a settled directory listing. The command opens the recordings read-only and hashes the same compressed bytes it checks. See [RFC 0010 D5](../../docs/rfc/0010-public-recording-dataset.md#d5-seal-the-day-with-a-manifest).

```text
hl-publish seal --day YYYY-MM-DD --staging-dir ROOT --output-dir OUT --expires YYYY-MM-DD
                [--quality-results FILE] [--deny-list FILE] [--previous-manifest FILE]
                [--coverage-end COIN=YYYY-MM-DDTHH:MM:SS[.fraction]Z]...
```

The source directory is `ROOT/hl-public/YYYYMMDD/`. Expiry can be the recording day or a later date. Each coverage end names one v1 coin and an instant on the recording day. The optional fraction has one to nine decimal digits. Supply the immediately preceding UTC day's manifest when available.

Each direct entry receives one stdout line in basename byte order. The second column is its one-based position in that listing. Columns are separated by tabs. A withheld entry uses a fixed reason so even a filename containing private text or a newline stays out of the report.

```text
accepted<TAB>position<TAB>recorder basename
withheld<TAB>position<TAB>reason
```

An exit status of zero means `OUT/manifest.json` is ready. Use a separate output directory for each day, outside the staged source. The command installs the complete file atomically, accepts an identical retry and reports an error if a different manifest already exists. A missing quality file still produces the per-file report and exits with a day-not-ready diagnostic. A completed run can describe zero accepted files.

## Operator inputs

The quality file is a UTF-8 JSON object. Its producer sets `complete` to true after finishing the run on the specified day's public inputs. Supply a nonempty loader revision and at least one result. Each result has a check name, subject, status and unsigned integer counts. The four statuses are `pass`, `fail`, `skip` and `info`. The manifest preserves completed alarms and the supplied check order. The sealer validates this input locally. [RFC 0005](../../docs/rfc/0005-data-quality-checks.md)

```json
{
  "day": "2026-11-26",
  "complete": true,
  "loader_revision": "synthetic",
  "checks": [
    {"name": "coverage", "subject": "bbo", "status": "info", "compared": 1, "violations": 0}
  ]
}
```

Keep the optional deny-list outside the public tree. It is a UTF-8 text file with one literal, case-sensitive substring per line. LF and CRLF are accepted. Empty lines are ignored. Spaces and `#` are ordinary characters. The sealer checks both the raw lifecycle JSON and decoded string values and keys. It also screens public output for deny-list entries and rejected outgoing text encountered in readable records. A collision with a required output field prevents sealing.

## Reading the manifest

File paths are relative to the distribution bucket. Sequence numbers, process anchors and UTC Unix nanosecond bounds are decimal strings. Sequence controls replay order within a process even when its wall clock steps. A file can link to its immediately preceding sequence in the supplied previous manifest. Conflicting claims and identical copies with ambiguous names both leave a missing sequence.

`coins_covered` maps a coin to the per-coin channels observed in accepted recordings. An outgoing subscription or shared frame contributes no per-coin observation. An empty `coverage_ends` means the operator supplied no delisting for the day.

The gap kinds are `disconnect`, `dropped`, `missing_sequence`, `process_change` and `withheld`. A `null` time bound is unknown. A disconnect with `continues: true` carries into the next day. A withheld hour describes the candidate's named UTC hour. Foreign names contribute a count with unknown bounds. Lifecycle reasons use fixed categories so local error strings stay private.

The complete synthetic [clean-day manifest](tests/manifests/clean.json) and [manifest with gaps](tests/manifests/gaps.json) are byte-for-byte CLI test expectations.
