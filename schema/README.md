# ClickHouse schema

Table definitions as numbered migrations, applied in order by
`hl-loader migrate`:

```text
schema/0001_init.sql
schema/0002_quality.sql
schema/0003_rawlog_state.sql
```

The loader embeds these files at build time and records each applied one,
with its SHA-256, in `hl.schema_migrations`.

To start a local server, apply the migrations, load files and check data
quality, see the [local ClickHouse guide](local/README.md).

## Rules

- A migration that has been applied anywhere is never edited. Changes go in a
  new file, and the new file is added to `MIGRATIONS` in
  `crates/hl-loader/src/db.rs` together with the row types that use it.
- Every statement ends with `;` at the end of a line, there is no `;` inside
  a statement, `--` comments stand on their own lines, and `?` does not
  appear (the client would read it as a placeholder). The loader refuses a
  file that breaks these rules.
- Use `IF NOT EXISTS`, so a migration that failed half-way can be rerun.
- `hl-loader load` and `check` exit with status 3 before writing anything
  if the database migrations differ from the ones built into the loader.
  Build the loader from the commit that contains the new migration. Run
  `hl-loader migrate` with that binary, then use it to replace every
  installed copy.
- Three databases are planned: `hl` (market data, rebuildable from raw files),
  `live` (our own orders and account) and `research` (experiment results).
  Only `hl` exists so far.
- Prices and sizes are `Decimal64(8)`, the same representation as the Rust
  fixed-point types in `hl-types`.

Migration `0003` adds `hl.rawlog_state`, with one logical row per
`(source, process_start_wall_ns, file_seq)`. It stores the file's SHA-256,
ingest id and compressed JSON containing the connection and L2 books.
Prices and sizes use their exact scaled integers. A forced reload replaces
the state under a new ingest id. The state is derived from raw files and
can be rebuilt by loading each chain in order.

Design and measurements are in
[RFC 0004](../docs/rfc/0004-clickhouse-schema-and-loading.md)
(market data and carried state, `0001` and `0003`) and
[RFC 0005](../docs/rfc/0005-data-quality-checks.md) (quality tables, `0002`).
