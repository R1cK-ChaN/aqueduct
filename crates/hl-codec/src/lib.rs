//! Formats: how raw data is laid out on disk and, later, how venue payloads
//! turn into `hl-types` events.
//!
//! Implemented so far:
//!
//! - [`rawlog`]: the record format of the recorder's files.
//! - [`hyperliquid`]: decoding of the Hyperliquid WebSocket book and trade
//!   channels, the official L2 archive and `node_fills_by_block` into typed
//!   records and market events.
//!
//! Planned: the context channels and Binance reference data. Backfill and
//! live data go through the same parsers so both produce identical events.
//!
//! This crate reads bytes and returns values. It opens no files or sockets;
//! file-level compression is the caller's business.

pub mod hyperliquid;
pub mod rawlog;
