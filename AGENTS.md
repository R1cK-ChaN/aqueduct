# Working rules for this repository

These rules apply to people and to coding agents.

## Never do these

1. Keep private keys out of the repository, tests, logs, command lines and
   model contexts.
2. Do not commit data or secrets. This includes market data, vendor samples,
   `.env` files and credentials.
3. Have a maintainer review any code that can sign or send an order before
   running it anywhere.

## Language

Code, comments, documentation, commit messages, issues and pull requests are
written in English.

## Engineering rules

- **Venue facts.** A statement about how a venue behaves, or about what its
  feeds and archives contain, is written once in `docs/venues/` with its
  source and the date it was checked. New RFC, README and rustdoc text cites
  the entry instead of restating it. A simulator approximation names the
  entry it stands in for.
- **No floating point in accounting.** Prices, sizes, fees and balances use
  the fixed-point types in `hl-types`. Strategies may use floats for signals
  and convert at the edge.
- **Strategies do no IO.** A strategy depends only on `hl-strategy` and
  `hl-types`. `hl-strategy` must not depend on a gateway, the simulator or
  any crate that performs IO. Orders go through the `OrderGateway` trait in
  `hl-gateway`.
- **Determinism.** The same inputs must give the same outputs. No wall clock,
  no unseeded randomness and no hash-map iteration order in anything a
  backtest result depends on.
- **Raw files are the source of truth.** ClickHouse runs on the research
  machine only, never on the trading host, and must be rebuildable from raw
  files.
- **Untrusted files.** Downloaded archives and samples go in their own
  directory, away from scripts, and Python that reads them runs with `-I`.
- **Dependencies.** External crates are declared once, with a version, in
  `[workspace.dependencies]` of the root `Cargo.toml`. `hl-types`, `hl-rules`,
  `hl-strategy`, `hl-gateway`, `hl-sim` and `hl-book` have none; adding one
  there needs the owner's agreement.
- **Interfaces first.** A change to `hl-types`, the `Strategy` trait, the
  `OrderGateway` trait or the raw recording format is written up in
  `docs/rfc/` before it is implemented.

## Before you finish a change

Run all three from the repository root and report the result as it is:

```bash
cargo fmt --all --check
```

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

```bash
cargo test --workspace
```

New behaviour comes with tests. If a check was skipped or could not run, say
so.

## Review

One task, one branch, one pull request.
