# Hyperliquid feed timing

The input contains 3 complete files, 564,933 market frames and 2 continuous segments, covering 1.846 observed hours from 2026-11-24T07:08:09.164809383Z to 2026-11-24T08:59:59.941518074Z.

## Method and coverage

The sample uses bbo, l2 and trades. Observed hours sum first-to-last frame spans within continuous segments. Hourly files retain continuity, while recorded connection gaps and process restarts split segments.

Each physical frame contributes once to activity and socket-read statistics. A trade frame contributes once per distinct block time to block and latency measurements. The spacing split counts individual trades in the leading block. Quantiles use nearest rank and timing tables use milliseconds.

The first nonempty trade batch following each recorded trade subscription is treated as recent-history replay, covering 22 frames and 410 frame/block memberships. The sample has 22 trade frames with multiple block times. Rows labelled live omit those replay batches.

Latency is receive wall time minus exchange block time. Activity counts market frames in the monotonic interval [t−1 second, t), excluding all timestamp ties at t. Block receive spread uses last minus first monotonic time. Record order resolves arrival ties.

| Channel | Frames | Frames behind the latest block | Negative latency observations |
| --- | --- | --- | --- |
| bbo | 400,003 | 0 | 21 |
| l2 | 129,786 | 72 | 0 |
| trades | 35,144 | 1,803 | 1 |

Negative delays are retained. Absolute one-way delay calibration is unresolved.

## Block spacing

| Sample | N | Min | p1 | p10 | p50 | p90 | p99 | p99.9 | Max |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| All intervals | 81,177 | 30 | 50 | 59 | 69 | 133 | 162 | 1,819 | 71,562 |
| Live intervals | 80,819 | 30 | 50 | 59 | 69 | 133 | 155 | 207 | 550 |

| Trades in preceding block | N | Min | p1 | p10 | p50 | p90 | p99 | p99.9 | Max |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| All / 0 | 55,300 | 35 | 50 | 60 | 69 | 133 | 156 | 207 | 490 |
| All / 1 | 12,014 | 36 | 49 | 59 | 69 | 134 | 468 | 17,074 | 71,562 |
| All / 2-4 | 10,265 | 35 | 48 | 58 | 69 | 132 | 203 | 4,450 | 44,504 |
| All / 5+ | 3,598 | 30 | 47 | 57 | 69 | 130 | 156 | 837 | 7,820 |
| Live / 0 | 55,304 | 35 | 50 | 60 | 69 | 133 | 156 | 207 | 550 |
| Live / 1 | 11,798 | 36 | 49 | 59 | 69 | 133 | 153 | 211 | 269 |
| Live / 2-4 | 10,135 | 35 | 48 | 58 | 68 | 131 | 153 | 204 | 266 |
| Live / 5+ | 3,582 | 30 | 47 | 57 | 68 | 129 | 148 | 202 | 362 |

The live modal cadence is 67 ms. The most frequent gap in the 1.5–2.5-period band is 134 ms. Rounding each live interval to the nearest positive multiple of the modal cadence gives 18,199 inferred missing block steps and a missing share of 18.38%. The denominator is the sum of inferred block steps. The shortest interval wins a modal tie. This estimates visibility within the recorded instruments from the assumed cadence.

| Trades in preceding live block | Inferred missing steps | Missing share |
| --- | --- | --- |
| 0 | 12,533 | 18.48% |
| 1 | 2,667 | 18.44% |
| 2-4 | 2,233 | 18.05% |
| 5+ | 766 | 17.62% |

## Delivery order and socket reads

Across 81,179 recorded block groups, 80,831 arrive as a contiguous run and 348 are interrupted by other market frames. 1,875 frames contain a block time older than the latest block already received in their segment.

Channel patterns collapse adjacent repeats so several instruments on one channel form one run, while later returns to a channel remain visible.

| Channel-run pattern | Block groups |
| --- | --- |
| bbo | 46,477 |
| bbo > trades | 21,198 |
| bbo > l2 | 7,069 |
| bbo > l2 > trades | 3,398 |
| l2 | 1,729 |
| trades | 1,258 |
| l2 > bbo | 26 |
| l2 > trades | 11 |
| l2 > bbo > trades | 10 |
| bbo > l2 > trades > l2 | 2 |
| l2 > bbo > l2 | 1 |

| Channel pair A / B | All A before all B | All B before all A | Mixed |
| --- | --- | --- | --- |
| bbo/l2 | 10,469 | 36 | 1 |
| bbo/trades | 24,608 | 0 | 0 |
| l2/trades | 3,419 | 0 | 2 |

The proposed channel order must satisfy pairwise majorities of these full channel ranges, with mixed groups included in each denominator, and yield one compatible permutation.

| Within-block receive spread | N | Min | p1 | p10 | p50 | p90 | p99 | p99.9 | Max |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| All block groups | 81,179 | 0 | 0 | 0.015 | 0.025 | 0.052 | 0.231 | 60.78 | 2,553.601 |

Among 564,931 consecutive market-frame pairs, 0 (0.00%) share an exact receive timestamp and 482,731 (85.45%) fall in the same millisecond. The recorder stamps each WebSocket frame individually. Socket-read boundaries cannot be recovered from those stamps.

## Latency

| Channel | N | Min | p1 | p10 | p50 | p90 | p99 | p99.9 | Max |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| bbo | 400,003 | -12.372 | 174.429 | 191.943 | 229.625 | 307.35 | 456.329 | 542.493 | 1,469.104 |
| l2 | 129,786 | 46.885 | 208.367 | 228.879 | 261.301 | 331.063 | 462.932 | 579.249 | 1,448.312 |
| trades | 35,532 | -1.837 | 180.482 | 200.298 | 249.736 | 336.164 | 7,957.615 | 298,711.891 | 577,409.062 |
| live trades | 35,122 | -1.837 | 180.439 | 200.14 | 248.989 | 330.737 | 473.335 | 582.596 | 1,469.107 |

Activity boundaries are the pooled physical-frame 20th, 40th, 60th and 80th percentiles, with inclusive upper bounds 69, 81, 92, 105. Equal counts stay in the same bin. A separate live-trades row appears only when the group contains subscription history.

| Activity bin / channel | N | Min | p1 | p10 | p50 | p90 | p99 | p99.9 | Max |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Q1 (0–69) / bbo | 75,025 | -12.372 | 171.491 | 188.416 | 235.454 | 347.501 | 494.246 | 848.553 | 1,469.104 |
| Q1 (0–69) / l2 | 36,904 | 46.885 | 205.216 | 224.335 | 251.495 | 331.368 | 479.352 | 617.343 | 1,448.312 |
| Q1 (0–69) / trades | 4,903 | -1.837 | 173.756 | 193.595 | 250.914 | 496.074 | 252,677.44 | 530,218.062 | 577,409.062 |
| Q1 (0–69) / live trades | 4,493 | -1.837 | 172.737 | 191.982 | 246.65 | 361.642 | 514.846 | 1,380.853 | 1,469.107 |
| Q2 (70–81) / bbo | 78,644 | -4.652 | 173.058 | 189.683 | 226.859 | 310.661 | 466.42 | 559.665 | 1,242.841 |
| Q2 (70–81) / l2 | 28,633 | 179.239 | 209.649 | 229.315 | 259.207 | 334.83 | 470.108 | 585.068 | 753.694 |
| Q2 (70–81) / trades | 5,951 | 145.27 | 178.273 | 196.46 | 246.744 | 333.149 | 498.43 | 611.432 | 1,298.975 |
| Q3 (82–92) / bbo | 81,298 | 150.559 | 174.658 | 191.785 | 227.609 | 304.457 | 451.721 | 535.374 | 1,196.648 |
| Q3 (82–92) / l2 | 24,764 | 180.95 | 211.838 | 231.629 | 264.396 | 331.289 | 456.768 | 585.072 | 657.753 |
| Q3 (82–92) / trades | 6,966 | 152.444 | 181.275 | 199.643 | 246.176 | 326.195 | 472.915 | 582.626 | 629.99 |
| Q4 (93–105) / bbo | 83,652 | 146.361 | 176.289 | 193.607 | 227.399 | 296.447 | 407.997 | 498.91 | 1,156.691 |
| Q4 (93–105) / l2 | 21,412 | 181.293 | 212.502 | 232.698 | 268.434 | 332.726 | 440.995 | 520.132 | 1,139.882 |
| Q4 (93–105) / trades | 7,940 | 154.976 | 183.654 | 202.166 | 250.019 | 325.72 | 448.98 | 530.964 | 639.176 |
| Q5 (106+) / bbo | 81,384 | 151.138 | 178.066 | 196.926 | 231.567 | 295.127 | 385.477 | 483.898 | 1,066.237 |
| Q5 (106+) / l2 | 18,073 | 182.892 | 209.053 | 234.215 | 269.036 | 328.414 | 400.676 | 484.976 | 1,139.885 |
| Q5 (106+) / trades | 9,772 | 154.633 | 185.38 | 206.629 | 254.047 | 329.607 | 416.36 | 503.217 | 1,133.237 |

| UTC receive hour / channel | N | Min | p1 | p10 | p50 | p90 | p99 | p99.9 | Max |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 2026-11-24 07:00 / bbo | 163,394 | -12.372 | 170.683 | 188.168 | 221.507 | 293.658 | 442.226 | 514.606 | 619.067 |
| 2026-11-24 07:00 / l2 | 58,352 | 46.885 | 205.006 | 225.551 | 252.376 | 315.461 | 447.414 | 519.191 | 743.205 |
| 2026-11-24 07:00 / trades | 13,275 | -1.837 | 176.868 | 195.127 | 244.908 | 328.346 | 61,774.179 | 459,793.052 | 577,409.062 |
| 2026-11-24 07:00 / live trades | 12,865 | -1.837 | 176.497 | 194.763 | 243.126 | 313.7 | 448.181 | 519.608 | 619.068 |
| 2026-11-24 08:00 / bbo | 236,609 | 150.842 | 178.107 | 195.759 | 235.146 | 315.565 | 466.36 | 583.748 | 1,469.104 |
| 2026-11-24 08:00 / l2 | 71,434 | 182.892 | 213.273 | 233.431 | 268.682 | 342.255 | 475.721 | 631.984 | 1,448.312 |
| 2026-11-24 08:00 / trades | 22,257 | 154.976 | 183.776 | 203.974 | 253.16 | 339.62 | 490.238 | 611.432 | 1,469.107 |

Within-hour live p99 comparisons use the same pooled activity boundaries. Latency cells show Q1 → Q5 in milliseconds. The smallest extreme count covers both bins across the three channels.

| UTC receive hour | Smallest extreme N | bbo | l2 | live trades |
| --- | --- | --- | --- | --- |
| 2026-11-24 07:00 | 1,914 | 460.715 → 376.081 | 455.506 → 426.512 | 477.181 → 421.379 |
| 2026-11-24 08:00 | 1,201 | 538.631 → 386.723 | 510.4 → 398.956 | 580.814 → 416.227 |

A channel supports conditioning here when p99 moves consistently across the five bins and its Q1-to-Q5 direction holds in each sampled hour (at least two). Comparisons use bins with at least 100 observations for p99. These recordings measure the association with received traffic over this period.

## Input files

| Path | Market frames | SHA-256 |
| --- | --- | --- |
| `20261124/hl-20261124T07-1795504088995.hlrec.zst` | 15,719 | `837b7e29615215509964171e8eb0ef2fbe703f3f95b7af1534dcdd4be9602352` |
| `20261124/hl-20261124T07-1795504367800.hlrec.zst` | 218,914 | `32c4883221a912be3c0f4e731d1fede32248f4db707753272241fec0e31c0f3f` |
| `20261124/hl-20261124T08-1795507200041.hlrec.zst` | 330,300 | `cae63b8fa9ebc82f2ba2e9f15f881203eee47caa72bc8fe68c6bb638115d3081` |

## Inputs for the time model

- Use 67 ms as the provisional nominal spacing for unseen blocks, with a visible live median of 69 ms and an inferred missing share of 18.38%.
- Preserve file order for recorded feeds and use bbo → l2 → trades as the empirical default for archive replay, allowing the measured exceptions (348 interleaved blocks and 1,875 late frames).
- Sample bbo, l2, trades latency separately in preceding-second frame-count bins 0–69 / 70–81 / 82–92 / 93–105 / 106+. Each channel's p99 trend across the five bins also appears within individual UTC hours.
