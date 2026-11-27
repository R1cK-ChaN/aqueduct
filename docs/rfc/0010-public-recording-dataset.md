# RFC 0010: A public dataset of Hyperliquid recordings

- Status: accepted. The owner decided the site, licence and disclosure on 2026-11-26. The owner handles the cloud and deployment steps in D8.
- Date: 2026-11-26
- Applies to: `hl-recorder`, `hl-loader`, `hl-backtest` and the public dataset

A reader should be able to download one UTC day, check its bytes and replay
one coin without running a recorder. Our recorder timestamps each frame at
receipt. That clock matters to the latency model. The official archives feed
the loader and lack this observation. [RFC 0002][raw] · [RFC 0006][time] ·
[HL-120](../venues/hyperliquid.md#hl-120)

## D1. What we publish

**Owner decisions, 2026-11-26.** We supply recordings without charging for
them so others can research and provide liquidity. The owner pays for storage.
Readers pay Google for their requests and transfer through Requester Pays.
The owner chose a fixed set of 14 perpetuals for dataset version `v1`.
Continuous per-coin coverage matters more to dataset users than tracking
current liquidity. A public, mechanical rule applied once selected the
coins, so the list reveals no private selection. [HL-148](../venues/hyperliquid.md#hl-148)

Give distribution its own bucket and publishing job. Opening the private raw
bucket would expose everything added later under the same access settings.
Keep its create-only recorder identity and enforced public access prevention.
Run a dedicated public source from capture onwards. Filtering a private
recording would leave its original subscriptions in `SentText`. Publish each
sealed file byte for byte under its recorder name and SHA-256. Re-encoding
or merging hours would break the link between the recorder's output and the
reader's input. [RFC 0002][raw]

**Owner's terms review, 2026-11-26.** The Hyperliquid Corp. terms updated
2026-06-15 describe the web Interface and the permissionless chain in §1.1,
and restrict excessive automated use and rate-limit bypass in §3.1.8. The
review found no express market-data redistribution licence. The owner will
publish on the basis of this review. The dataset licence grants only the
owner's own rights and makes no claim over venue data. [Hyperliquid Terms][hl-terms]

Limit the dataset to our own unauthenticated Hyperliquid recordings. The
owner's review of Binance's terms effective 2026-10-27 found retained
database rights (§26), a personal or internal-use grant (§27) and separate
API terms (§14.1.2(b)) that the review did not locate. Binance Vision's
CC BY-NC-SA 4.0 terms cover its own downloads. Keep `binance-public`,
`binance-market`, vendor data and future authenticated feeds private.
[Binance Terms][bn-terms] · [Binance Vision Dataset Terms][vision-terms]

## D2. Fixed universe for v1

Record the 14 mainnet main-dex perpetuals that appeared in the top 30 by
approximate notional in all 28 weekly rankings from 2026-05-18 through
2026-11-23. They are BTC, ETH, SOL, HYPE, XRP, ZEC, DOGE, SUI, AAVE,
NEAR, TAO, PUMP, XPL and kPEPE. Spot, outcomes and HIP-3 markets are
outside this universe. The private source is configured independently.
[HL-148](../venues/hyperliquid.md#hl-148) · [HL-28](../venues/hyperliquid.md#hl-28) ·
[HL-55](../venues/hyperliquid.md#hl-55)

Keep this list for the life of `v1`. If a coin is delisted, its coverage
ends and the day's manifest records the coin and the end time. Changing
the list requires a new dataset version approved by the owner and recorded
in the manifests (D5).

Subscribe to `l2`, `bbo`, `trades` and `activeAssetCtx` for each listed
coin and to `fastAssetCtxs` once. With all 14 coins, this uses
**4 × 14 + 1 = 57** of the 1,000 subscriptions per IP. Keep the shared
context frame intact even when it names other markets. [HL-143](../venues/hyperliquid.md#hl-143) ·
[HL-45](../venues/hyperliquid.md#hl-45) · [HL-47](../venues/hyperliquid.md#hl-47) ·
[HL-91](../venues/hyperliquid.md#hl-91) · [HL-92](../venues/hyperliquid.md#hl-92)

One public connection uses one of ten connection slots. Count all clients
sharing its outgoing IP before deployment. Give this source at most two
connection attempts and 300 outbound messages per rolling minute, including
resubscriptions and pings. The recorder tracks attempts in memory, and
deployment must set a service restart delay of at least 60 seconds to reserve
the allowance across restarts. Pace sends and back off if the remaining IP
allowance is smaller.
[HL-144](../venues/hyperliquid.md#hl-144) · [HL-145](../venues/hyperliquid.md#hl-145) ·
[HL-146](../venues/hyperliquid.md#hl-146)

## D3. Recording site

On 2026-11-26 the owner chose a separate source on the Tokyo trading host,
beside the private recorder. Its receive timestamps come closest to a Tokyo
strategy's view, and it can record around the clock with the host.

The public source shares the outgoing IP's limits with the private recorder
and any later trading client. Count all clients when assigning the public
source its D2 allowance for subscriptions, open connections, new connections
per minute and messages sent per minute.
[HL-143](../venues/hyperliquid.md#hl-143) · [HL-144](../venues/hyperliquid.md#hl-144) ·
[HL-145](../venues/hyperliquid.md#hl-145) · [HL-146](../venues/hyperliquid.md#hl-146)

Both recorders use the host's disk, so the pilot measures volume and backlog.
Give the public source its own OS account, config, spool and upload path.
Write its finished files to a staging prefix in the operator's private
storage, outside the prefix the research pull reads.
Keep the publisher and quality checks on the research machine with
ClickHouse. The operator pulls staged files hourly. A later pull can collect
files missed during downtime.
[System boundary](../ARCHITECTURE.md#system-boundary)

| Consideration | Research machine | Separate source on the Tokyo trading host | Separate small VM |
| --- | --- | --- | --- |
| Uptime and gaps | On a machine that is not always on, downtime leaves a capture gap even after the next pull. | Can run around the clock with the trading host. Maintenance and recorder restarts interrupt capture. | Can record through downtime on either existing machine. Its own restarts and outages leave gaps. |
| Receive timestamps | Measure the research site's network path and local load. | A separate connection measures arrival on the same host and clock as trading, the closest of the three to a Tokyo strategy's view. | Measure the VM's path, clock and load. Even in Tokyo it has a separate connection and host. |
| Clients sharing the outgoing IP | Count all Hyperliquid clients sharing the research machine's outgoing IP. | Include the private recorder, trading clients and anything else using that IP. | A distinct outgoing IP has its own allowance. With shared egress, include all clients behind it. |
| Cost beyond public storage | Uses the existing machine, with power, disk and one public upload per file. | Uses the existing host, with extra capture load, disk and transfer to research. | Adds VM runtime, disk and transfer to research. Only the owner creates the VM. |
| Publisher and quality check | Read the sealed files locally, check them in research ClickHouse and publish. | Stage the sealed public files privately for the research checker and publisher. Keep heavy work off the trading host. | Buffer sealed files until the research machine can receive them for the same checks and publication. |

Use a single `[[source]]` named `hl-public` in the public recorder's
config. Use zstd level 3 and RFC 0002 hourly rotation. [RFC 0002][raw]

Validate the expanded config before connecting and the outgoing message
before each send. Require the exact public mainnet WebSocket URL, without
userinfo, query, custom headers or credentials. Accept only the exact
subscription shapes in D2 with a `v1` coin where required, and the public
ping. Reject unknown or duplicate keys, user channels, signing fields,
`post` and arbitrary text. The capture account needs no trading or cloud
credentials. Before an authenticated feed may run on the same host, give
it a separate OS identity, config, spool and upload path, and test that its
authentication exchange cannot enter raw files, metadata or logs. The public
accounts must be unable to read its files. [HL-40](../venues/hyperliquid.md#hl-40) ·
[HL-41](../venues/hyperliquid.md#hl-41) · [HL-42](../venues/hyperliquid.md#hl-42) ·
[HL-43](../venues/hyperliquid.md#hl-43)

**Measurement and estimate.** The one-minute samples in HL-147 came from a
workstation outside the recording region and gave a daily equivalent of
1.1–1.7 GB for 11 coins. Scaling by `14 / 11` gives an estimated
**1.40–2.16 GB/day** of compressed files. Each full transfer would average
about 0.13–0.20 Mbit/s over the day. Seven days
of files would take about 9.8–15.1 GB, plus working space for ClickHouse
on the research machine. Bytes do not scale linearly with coin count. The
shared channel has a fixed component and active coins produce different
traffic. Measure peak inbound bandwidth, backlog and the actual 14-coin
volume in the pilot. Retain unacknowledged files and alert on backlog or
low disk space. [HL-147](../venues/hyperliquid.md#hl-147) ·
[Sample and method][sample]

## D4. Storage, access and cost

**Our choice.** Use a regional Standard bucket in Tokyo (`asia-northeast1`).
Announce the bucket name with the download guide.
Enable uniform bucket-level access and Requester Pays. On this bucket alone,
grant `roles/storage.objectViewer` to `allAuthenticatedUsers` for object
get and list. The owner checks that inherited public access prevention and
domain restrictions permit this grant before advertising downloads. Leave
the private bucket outside the grant. [Object Viewer][roles] ·
[IAM principals][principals] · [Public access prevention][pap]

A reader signs in with a Google identity and supplies a billing-enabled
project in good standing on which that identity has `serviceusage.services.use`
(for example, Service Usage Consumer). The bucket grant supplies read access.
The reader's project pays applicable request and transfer charges. Anonymous
requests cannot use this grant. Ordinary readers omitting a billing project get
`400 UserProjectMissing`. Google's exception for a principal able to assign
billing on the bucket's own project does not apply to the public reader
grant. The publisher bills its own operations to the owner's project.
[Requester Pays, checked 2026-11-26][rp] · [Billing a request][rp-use]

Keep raw objects for 360 days after the end of their UTC recording day.
Set that timestamp as object custom time and use a lifecycle delete rule
on raw files after 360 days. Use no object versioning or soft-delete copies
in this distribution bucket. Keep the small daily manifests as a permanent
index of what was available, including the raw expiry date.
[Lifecycle conditions][lifecycle] · [Soft delete][soft-delete]

**Owner's storage estimate.** Tokyo regional Standard lists **USD 0.023 per
GiB-month**. Use decimal GB for the sample, `2^30` bytes per billed GiB,
30 days per model month and the unrounded `14 / 11` scale. Each row is the
monthly storage rate at that inventory. The first month's actual charge
accrues as files arrive. [Google Cloud Storage pricing, Tokyo, checked 2026-11-26][price]

| Months accumulated | Days | Public raw GiB | USD/month |
| ---: | ---: | ---: | ---: |
| 1 | 30 | 39–60 | 0.90–1.39 |
| 6 | 180 | 235–363 | 5.40–8.34 |
| 12 | 360 | 469–725 | 10.80–16.68 |

All estimates, including the high 12-month case, exclude compute, disks,
private storage, transfer to research, publisher requests, manifests and
taxes. Compare the pilot's bytes with the bill before launch. An alert
does not cap spending.
[Google pricing, checked 2026-11-26][price]

## D5. Seal the day with a manifest

**Our choice.** Run the publisher on the research machine with read-only
access to the sealed `hl-public` files staged there from Tokyo (D3).
Wait for the site's final list of files and gaps for the UTC day and for
all listed files to arrive.
Use a separate, short-lived Google identity. Give it
`roles/storage.objectCreator` on the distribution bucket, plus the viewer
access any authenticated reader has. It has no write access to the private
bucket. The existing private uploader also saves truncated files, so its
wildcard is unsuitable for publishing. [Object Creator][roles] · [RFC 0002][raw]

1. Take only regular files directly inside `hl-public/YYYYMMDD/` matching
   `hl-public-YYYYMMDDTHH-<open_ms>.hlrec.zst`, with the same date in both
   names. Refuse symlinks, `.part`, `.truncated.hlrec.zst` and foreign
   names. Read the sealed zstd frame through its checksum and clean EOF.
   Decode every RFC 0002 record, check `file_open` source, URL and sequence,
   and recheck every `SentText` against the send rules for that dataset version.
   Screen lifecycle metadata, including error text, for private identifiers.
   Withhold a suspect file whole. A redacted copy cannot match its raw hash.
2. On a research ClickHouse instance containing only these public inputs,
   run `hl-loader load rawlog` in chain order and `hl-loader check YYYY-MM-DD`
   for the UTC day. Carry book state across rotations. Preserve `pass`,
   `fail`, `skip` and informational coverage with the check name, subject,
   compared and violation counts. Checks lacking archive inputs report
   `skip`. Publish completed checks, including alarms, so a reader can
   judge the day. An interrupted check defers release. [RFC 0005][quality]
3. Compute SHA-256 and CRC32C over the sealed compressed bytes. Upload those
   bytes to `v1/hl-public/YYYYMMDD/<original filename>` with the CRC32C for
   server-side validation and `ifGenerationMatch=0`. Check the stored size
   and CRC32C on success. If the name exists, hash the existing object and
   accept it only if its size and SHA-256 match. A mismatch stops publication
   of that day. Write the manifest last under the same create-only and retry
   rules. A sealed manifest closes the day to further writes, including after
   raw expiry. [Data validation][validation] · [Request preconditions][preconditions]

`v1/hl-public/YYYYMMDD/manifest.json` is a UTF-8 JSON object. Each raw object
becomes readable when created, so disclosure and quality checks precede the
first upload. The manifest tells readers the day is ready.

| Field | Contents |
| --- | --- |
| `schema_version`, `day`, `expires`, `source`, `region` | Version `1`, UTC dates (`YYYY-MM-DD`), `hl-public` and the capture region `asia-northeast1` (D3, 2026-11-26). |
| `dataset_version` | The owner-approved dataset version, here `v1`. |
| `version_change` | Only on the first day of a version. Record the effective UTC day and the coins added or removed. For `v1`, the additions are the 14 coins in D2. |
| `coins_covered` | Coins actually observed on individual channels and which channels had data. |
| `coverage_ends` | Coin and UTC end time for any delisting that ended coverage that day. |
| `files` | In replay order, each entry's bucket-relative `path`, compressed `size_bytes`, lowercase hex `sha256`, `process_start_wall_ns` and `file_seq`. Store the last two as decimal strings. Link the preceding sequence's path across midnight. Zero starts a new process chain. |
| `gaps` | UTC bounds when known, chain and sequence, lifecycle reason and dropped count when present. Carry an open disconnect across midnight. Mark process changes, missing sequences and withheld/truncated hours. Leave unknown bounds open. |
| `quality` | UTC day, loader revision and RFC 0005 results for just this dataset. A day with no accepted files can still report zero coverage. |

Key a chain by `(source, process_start_wall_ns)`. Detect duplicates and
holes from `file_seq` even when the wall clock steps. Withhold conflicting
copies of a sequence and record the hole as a gap. Readers can then choose
a contiguous span without mistaking a restart or a withheld file for a
quiet market. [RFC 0002][raw] ·
[RFC 0008][paper]

## D6. Download and replay

**Reader procedure.** Use a signed-in Google Cloud CLI, Bash, `jq`, GNU
`sha256sum` and `hl-backtest` built from the clean public revision given
in the download guide. Set `BILLING_PROJECT` to your own project. Set
`BUCKET` and `DAY` (`YYYYMMDD`) from the guide. Use an empty directory
outside the checkout. This example
uses a published day with BTC BBO coverage and consecutive sequences.

```bash
set -euo pipefail
prefix="gs://${BUCKET}/v1/hl-public/${DAY}"
gcloud storage cp --billing-project="$BILLING_PROJECT" "$prefix/manifest.json" manifest.json
jq '{day, coins_covered, gaps, quality}' manifest.json
jq -er '.files[] | [.sha256, (.path | split("/") | last)] | join("  ")' manifest.json > SHA256SUMS
jq -er '.files[].path' manifest.json > paths
files=()
while IFS= read -r path; do
  name=${path##*/}
  gcloud storage cp --billing-project="$BILLING_PROJECT" "gs://$BUCKET/$path" "./$name"
  files+=("./$name")
done < paths
sha256sum --check --strict SHA256SUMS
hl-backtest run --network mainnet --coin BTC --half-spread 10 --qty 0.001 \
  --order-latency-ms 400 --report-latency-ms 250 --maker-fee-bps 1.5 \
  --session-out session.jsonl "${files[@]}"
hl-backtest run --check-session session.jsonl
```

The last command uses the same build and files to check raw hashes and
recompute the decisions. The public download guide supplies the bucket,
release dates, licence and citation. Ship the documents it uses with the
public build so a reader can follow the guide from that build alone.
They include the build and CLI README, RFC 0002 (raw records), RFC 0005
(quality) and RFC 0008 (replay). Include the session file reference as
part of the replay interface.
[Build and CLI][cli] · [RFC 0002][raw] · [RFC 0005][quality] ·
[RFC 0008][paper] · [Session file][session]

## D7. Licence and disclosure

**Licence, 2026-11-26.** The owner delegated the licence choice. The
dataset licence is CC BY 4.0, effective with the first release (D8).
Live market making is an intended commercial use. CC BY 4.0 permits it,
and the credit it requires tells readers where the data comes from.
CC BY-NC 4.0 requires credit but would rule out that use. CC0 waives
the owner's rights as far as possible and leaves credit voluntary. Free
distribution by the owner fits all three. Record the licence with each release.

Suggested citation line: “hlmm contributors, *Public Hyperliquid recordings*,
UTC `<day>`, manifest `<object path>`, SHA-256 `<manifest digest>`,
CC BY 4.0.” Hyperliquid neither provides nor endorses this dataset.

**Disclosure, 2026-11-26.** The owner delegated the disclosure choice.
Retain the site, timing and public feed details in the first two rows and
exclude the material in the third row by construction. The public site
label is “Tokyo”. The exact bytes make the decision apply to the whole file.

| Treatment | What a reader can learn |
| --- | --- |
| Retain | The coarse recording site and its latency profile from exchange and receive clocks. `file_open` reveals the public source, venue URL, recorder version, process start and sequence. Lifecycle records reveal restarts and outages. |
| Retain | The entire public subscription set and changes to it, including sent frames. Public trades also carry venue-supplied participant fields. [HL-90](../venues/hyperliquid.md#hl-90) |
| Exclude by construction | Private candidate lists and subscription changes, private feed URLs, credentials and deployment identifiers. Keep them out of the public service and manifest. Withhold any file whose metadata discloses them. |

Keep recordings outside Git. Publish only sealed `hl-public` files and
their manifests after review. All other recordings and staging files are
private. The public guide depends solely on the public objects and
infrastructure.

## D8. Order of later issues

| Order | Work | Owner action and redeploy |
| ---: | --- | --- |
| 1 | Record the site, licence and disclosure. | Decided on 2026-11-26. Use a separate source on the Tokyo trading host and CC BY 4.0 for the dataset. Retain the first two disclosure rows in D7 and exclude the third by construction. The public site label is “Tokyo”. |
| 2 | The recorder uses the fixed `v1` config and enforces the send and reconnect checks, with tests for synthetic credential fields, wrong sources and reconnects. The recorder bundle includes the isolated source's deployment files. | The owner reviews the config, installs the bundle, starts the Tokyo pilot and measures a week of volume, throughput, gaps, upload backlog and disk use to check the cost estimate before launch. |
| 3 | `hl-publish` checks disclosure locally and builds the manifest with supplied quality results. The quality run, upload and research timer remain. Test mismatched upload retries and a crash before the manifest in the upload step. | Owner reviews the local trial with synthetic input. Install the research job with public writes disabled when it is ready. |
| 4 | Set up the distribution bucket, billing, IAM, expiry and monitoring. | Owner performs every cloud step after review. Check billed access with an outside identity and refusal of anonymous and unbilled reads. |
| 5 | Publish a reviewed day and the public downloader guide. | Owner enables the job after inspecting the first manifest and raw disclosure. Verify download, checksum and offline session replay from the public build before announcing the dataset. |

[raw]: 0002-raw-recording-format.md
[quality]: 0005-data-quality-checks.md
[time]: 0006-event-and-time-model.md
[paper]: 0008-paper-trading.md
[session]: ../session-file.md
[cli]: ../../README.md#run-a-recording-backtest
[sample]: ../measurements/2026-11-24-recorder-feeds.md#results
[hl-terms]: https://app.hyperliquid.xyz/terms
[bn-terms]: https://www.binance.com/en/terms
[vision-terms]: https://data.binance.vision/Binance_Vision-Terms_of_Use.pdf
[price]: https://cloud.google.com/storage/pricing
[rp]: https://cloud.google.com/storage/docs/requester-pays
[rp-use]: https://cloud.google.com/storage/docs/using-requester-pays
[principals]: https://cloud.google.com/iam/docs/principals-overview
[roles]: https://cloud.google.com/storage/docs/access-control/iam-roles
[pap]: https://cloud.google.com/storage/docs/public-access-prevention
[lifecycle]: https://cloud.google.com/storage/docs/lifecycle
[soft-delete]: https://cloud.google.com/storage/docs/soft-delete
[validation]: https://cloud.google.com/storage/docs/data-validation
[preconditions]: https://cloud.google.com/storage/docs/request-preconditions
