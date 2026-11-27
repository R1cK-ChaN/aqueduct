# Data vendor facts

Entry conventions and citation rules are in the [register guide](README.md).

## Rules

### VN-1

0xArchive's data license requires prior written permission to redistribute its data as standalone records, bulk copies or a substitute market-data product. This restriction covers the sample files used by the repository.

- **Kind:** rule
- **Source:** [0xArchive Data License](https://0xarchive.io/policies/data-license) and [sample handling](../../scripts/probes/README.md#not-kept-here)
- **Checked:** 2026-11-25
- **Status:** confirmed

### VN-2

Tardis provides data for the first day of each month without a Data API key.

- **Kind:** rule
- **Source:** [Tardis free data](https://docs.tardis.dev/faq/billing-and-subscriptions.md)
- **Checked:** 2026-11-25
- **Status:** confirmed

## Feeds and archives

### VN-3

The probes used a 0xArchive evaluation sample for 2026-11-10, obtained from the vendor and kept outside the repository.

- **Kind:** interface
- **Source:** [Sample-day probe instructions](../../scripts/probes/README.md#0xarchive-sample-day-checks) and [bulk-export guide](https://docs.0xarchive.io/exports/quickstart) and [export manifest schema documentation](https://docs.0xarchive.io/exports/manifest-schemas)
- **Checked:** 2026-11-25 (documentation review)
- **Status:** unverified. The public documentation does not describe how evaluation samples are offered, and the claim about the sample has not been rechecked.

### VN-4

0xArchive Hyperliquid trade exports carry a trade id, user address, order id and side, the identity fields used by the sample reconciliation against official fills.

- **Kind:** interface
- **Source:** [Trade export columns](https://docs.0xarchive.io/exports/file-columns) and [trade reconciliation](../../scripts/probes/README.md#0xarchive-sample-day-checks)
- **Checked:** 2026-11-25
- **Status:** confirmed

### VN-5

0xArchive L4 Parquet exports include checkpoints containing the full order-level book, with orders at each price listed in queue order.

- **Kind:** interface
- **Source:** [L4 checkpoint columns](https://docs.0xarchive.io/exports/file-columns)
- **Checked:** 2026-11-25
- **Status:** confirmed

### VN-6

0xArchive L4 Parquet change files identify individual orders and carry `new`, `update` and `remove` events in block-number and sequence order. An `update` sets the remaining order size.

- **Kind:** interface
- **Source:** [L4 change columns](https://docs.0xarchive.io/exports/file-columns)
- **Checked:** 2026-11-25
- **Status:** confirmed

### VN-7

0xArchive supplies an L2 price-level grid alongside its order-level data, which the sample probes compare with the reconstructed L4 book and official L2 snapshots.

- **Kind:** interface
- **Source:** [L2 export columns](https://docs.0xarchive.io/exports/file-columns) and [L4 reconciliation](../../scripts/probes/README.md#0xarchive-sample-day-checks)
- **Checked:** 2026-11-25
- **Status:** confirmed

### VN-8

0xArchive's HIP-3 order-book interfaces preserve the dex prefix in a symbol such as `xyz:TSLA`.

- **Kind:** interface
- **Source:** [L4 symbol conventions](https://docs.0xarchive.io/rest-api/order-books-l4)
- **Checked:** 2026-11-25
- **Status:** confirmed

### VN-9

0xArchive REST L4 diffs document `insert_before` for data from 2026-07-21 onward. For a new ALO order with queue priority, its value identifies the resting order immediately after the insertion point, and null or absence means append at the price-level tail.

- **Kind:** interface
- **Source:** [REST L4 diff schema](https://docs.0xarchive.io/schemas/operations/get-hyperliquid-l4-diffs) and [L4 reconstruction](https://docs.0xarchive.io/rest-api/order-books-l4)
- **Checked:** 2026-11-25
- **Status:** confirmed

### VN-10

The 0xArchive L4 Parquet change file omits the new order's insertion position, so FIFO replay can produce a different within-price order from the checkpoint queue.

- **Kind:** interface
- **Source:** [L4 export reconstruction](https://docs.0xarchive.io/exports/file-columns)
- **Checked:** 2026-11-25
- **Status:** confirmed

### VN-11

The L4 sample cited in RFC 0007 showed an in-place size reduction with an unchanged `oid`.

- **Kind:** interface
- **Source:** [RFC 0007 sample observation](../rfc/0007-fill-models.md#d7-reset-queue-priority-on-a-price-change-or-size-increase) and [L4 diff schema](https://docs.0xarchive.io/schemas/operations/get-hyperliquid-l4-diffs)
- **Checked:** 2026-11-25 (documentation review)
- **Status:** unverified. The cited observation has no retained row or sample identifier, and the public schema describes size updates without identifying this observation or the initiating action.

### VN-12

Tardis records Hyperliquid L2 order-book data and publishes a Hyperliquid `incremental_book_L2` sample, the data family named for the RFC 0005 comparison.

- **Kind:** interface
- **Source:** [Tardis Hyperliquid data documentation](https://docs.tardis.dev/historical-data-details/hyperliquid.md)
- **Checked:** 2026-11-25
- **Status:** confirmed

## Measured by us
