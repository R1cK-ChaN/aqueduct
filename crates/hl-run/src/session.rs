//! Deterministic session rows produced as the shared driver processes work.
//!
//! The format is specified in `docs/session-file.md`. This module encodes
//! lines in memory. The caller owns their storage and the input-file hashes.

use hl_codec::hyperliquid::Network;
use hl_sim::LatePrintMisses;
use hl_types::{
    CancelReason, InstrumentId, Liquidity, OrderAction, OrderEvent, OrderEventKind, RejectReason,
    Side, TimeInForce, TimerId, Timestamp,
};
use serde_json::{Value, json};

use crate::PendingAtEnd;
use crate::pnl::PnlSummary;
use crate::recording::{EventOrigin, RawPosition};

/// Run identity and fully materialized component parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionStart {
    /// Full commit hash of the executable's build, or `unknown`.
    pub commit: String,
    /// Whether tracked files differed from that commit at build time.
    pub dirty: bool,
    /// Driver name, currently `backtest` for offline replay.
    pub driver: String,
    /// Network used for instrument identity.
    pub network: Network,
    /// Recorder source for paper, absent for an ordinary backtest.
    pub source: Option<String>,
    /// Venue coin name.
    pub coin: String,
    /// Stable internal instrument identity.
    pub instrument: InstrumentId,
    /// Paper process anchor in epoch nanoseconds, absent for backtest.
    pub process_start_wall_ns: Option<i64>,
    /// Name used by the caller to construct the strategy.
    pub strategy_name: String,
    /// Byte stored in generated client order identifiers.
    pub strategy_tag: u8,
    /// Strategy-owned parameters in their declared order.
    pub strategy: Vec<(String, String)>,
    /// Name used by the caller to construct the simulator.
    pub sim_model: String,
    /// Simulator-owned parameters in their declared order.
    pub sim: Vec<(String, String)>,
}

/// One sealed raw file in input order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestEntry {
    /// Absolute path or path relative to the session's directory.
    pub path: String,
    /// Recorder sequence, absent for a legacy file.
    pub file_seq: Option<u64>,
    /// SHA-256 of the file's stored bytes as lowercase hexadecimal.
    pub sha256: String,
}

/// Driver state after all work strictly before a checkpoint cutoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckpointBoundary {
    /// Exclusive cutoff in epoch nanoseconds.
    pub cutoff: Timestamp,
    /// Sequence of the last work row before the cutoff, if any.
    pub last_seq: Option<u64>,
    /// Superseded quantity claimed by prints received before the cutoff.
    pub late_print_misses: LatePrintMisses,
}

/// The first representable UTC midnight strictly after the given timestamp.
pub fn next_utc_midnight(after: Timestamp) -> Option<Timestamp> {
    const DAY_NS: i64 = 86_400_000_000_000;
    let nanos = after.as_nanos();
    nanos
        .checked_add(DAY_NS - nanos.rem_euclid(DAY_NS))
        .map(Timestamp::from_nanos)
}

/// Encodes a captured boundary once the raw files through its extent have sealed.
pub fn checkpoint_line(
    boundary: CheckpointBoundary,
    last_raw: Option<RawPosition>,
    manifest: &[ManifestEntry],
) -> String {
    let mut row = Object::new();
    row.field("kind", json!("checkpoint"));
    row.field("cutoff_ns", json!(boundary.cutoff.as_nanos().to_string()));
    row.field(
        "last_seq",
        json!(boundary.last_seq.map(|seq| seq.to_string())),
    );
    raw_extent(&mut row, last_raw, manifest);
    row.object("late_print_misses", misses(boundary.late_print_misses));
    row.line()
}

/// Final account, raw extent and outstanding work at an inclusive stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEnd {
    /// Inclusive logical stop in epoch nanoseconds.
    pub stop: Timestamp,
    /// Final complete raw record, absent when there was no input file.
    pub last_raw: Option<RawPosition>,
    /// Input files in replay order.
    pub manifest: Vec<ManifestEntry>,
    /// Account computed by the shared accounting adapter.
    pub pnl: PnlSummary,
    /// Number of orders retained in the strategy host's local order table.
    pub open_orders: usize,
    /// Work left after processing the stop.
    pub pending: PendingAtEnd,
    /// Late prints allocated only to superseded versions.
    pub late_print_misses: LatePrintMisses,
}

/// Encoded session lines, with one sequence shared by all driver work.
///
/// Each line includes its terminating newline. An incremental caller can
/// drain lines after callbacks without resetting the sequence.
#[derive(Debug)]
pub struct Session {
    lines: Vec<String>,
    next_seq: u64,
}

impl Session {
    /// Starts a version-one session and encodes its `start` row.
    pub fn new(start: &SessionStart) -> Self {
        let mut row = Object::new();
        row.field("kind", json!("start"));
        row.field("schema_version", json!(1));
        row.field("commit", json!(start.commit));
        row.field("dirty", json!(start.dirty));
        row.field("driver", json!(start.driver));
        row.field("network", json!(start.network.short_name()));
        row.field("source", json!(start.source));
        row.field("coin", json!(start.coin));
        row.field("instrument", json!(start.instrument.to_string()));
        row.field(
            "process_start_wall_ns",
            json!(start.process_start_wall_ns.map(|time| time.to_string())),
        );
        row.field("strategy_name", json!(start.strategy_name));
        row.field("strategy_tag", json!(start.strategy_tag.to_string()));
        let mut strategy = Object::new();
        for (name, value) in &start.strategy {
            strategy.field(name, json!(value));
        }
        row.object("strategy", strategy);
        let mut sim = Object::new();
        sim.field("model", json!(start.sim_model));
        for (name, value) in &start.sim {
            sim.field(name, json!(value));
        }
        row.object("sim", sim);
        Self {
            lines: vec![row.line()],
            next_seq: 0,
        }
    }

    /// Encoded lines that have not been drained.
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// Takes encoded lines while preserving the next decision sequence.
    pub fn take_lines(&mut self) -> Vec<String> {
        std::mem::take(&mut self.lines)
    }

    pub(crate) fn last_seq(&self) -> Option<u64> {
        self.next_seq.checked_sub(1)
    }

    /// Appends the footer after the driver and accounting adapter finish.
    pub fn finish(&mut self, end: &SessionEnd) {
        let mut row = Object::new();
        row.field("kind", json!("end"));
        row.field("stop_ns", json!(end.stop.as_nanos().to_string()));
        raw_extent(&mut row, end.last_raw, &end.manifest);
        row.object("pnl", account(&end.pnl));
        row.field("open_orders", json!(end.open_orders));
        row.field("pending_actions", json!(end.pending.actions));
        row.field("undelivered_reports", json!(end.pending.reports));
        row.field("outstanding_timers", json!(end.pending.timers));
        row.object("late_print_misses", misses(end.late_print_misses));
        self.lines.push(row.line());
    }

    fn decision(&mut self, kind: &str) -> Object {
        let mut row = Object::new();
        row.field("kind", json!(kind));
        row.field("seq", json!(self.next_seq.to_string()));
        self.next_seq += 1;
        row
    }

    pub(crate) fn action(&mut self, now: Timestamp, action: &OrderAction) {
        let mut row = self.decision("action");
        row.field("ts_local_ns", json!(now.as_nanos().to_string()));
        match action {
            OrderAction::Place { cloid, order } => {
                row.field("variant", json!("place"));
                row.field("instrument", json!(order.instrument.to_string()));
                row.field("cloid", json!(cloid.to_string()));
                row.field("side", json!(side(order.side)));
                row.field("px", json!(order.px.to_string()));
                row.field("qty", json!(order.qty.to_string()));
                row.field(
                    "tif",
                    json!(match order.tif {
                        TimeInForce::Alo => "alo",
                        TimeInForce::Gtc => "gtc",
                        TimeInForce::Ioc => "ioc",
                    }),
                );
                row.field("reduce_only", json!(order.reduce_only));
            }
            OrderAction::Modify {
                instrument,
                cloid,
                px,
                qty,
            } => {
                row.field("variant", json!("modify"));
                row.field("instrument", json!(instrument.to_string()));
                row.field("cloid", json!(cloid.to_string()));
                row.field("px", json!(px.to_string()));
                row.field("qty", json!(qty.to_string()));
            }
            OrderAction::Cancel { instrument, cloid } => {
                row.field("variant", json!("cancel"));
                row.field("instrument", json!(instrument.to_string()));
                row.field("cloid", json!(cloid.to_string()));
            }
            OrderAction::CancelAll { instrument } => {
                row.field("variant", json!("cancel_all"));
                row.field("instrument", json!(instrument.map(|id| id.to_string())));
            }
        }
        self.lines.push(row.line());
    }

    pub(crate) fn report(&mut self, report: &OrderEvent) {
        let mut row = self.decision("report");
        row.field("ts_local_ns", json!(report.ts_local.as_nanos().to_string()));
        row.field("ts_exch_ns", json!(report.ts_exch.as_nanos().to_string()));
        row.field("instrument", json!(report.instrument.to_string()));
        row.field("cloid", json!(report.cloid.to_string()));
        match report.kind {
            OrderEventKind::Accepted { oid } => {
                row.field("variant", json!("accepted"));
                row.field("oid", json!(oid.0.to_string()));
            }
            OrderEventKind::Rejected { reason } => {
                row.field("variant", json!("rejected"));
                row.field("reason", json!(reject_reason(reason)));
            }
            OrderEventKind::Filled(fill) => {
                row.field("variant", json!("filled"));
                row.field("tid", json!(fill.tid.0.to_string()));
                row.field("oid", json!(fill.oid.0.to_string()));
                row.field("side", json!(side(fill.side)));
                row.field("px", json!(fill.px.to_string()));
                row.field("qty", json!(fill.qty.to_string()));
                row.field(
                    "liquidity",
                    json!(match fill.liquidity {
                        Liquidity::Maker => "maker",
                        Liquidity::Taker => "taker",
                    }),
                );
                row.field("fee", json!(fill.fee.to_string()));
            }
            OrderEventKind::Modified { px, qty } => {
                row.field("variant", json!("modified"));
                row.field("px", json!(px.to_string()));
                row.field("qty", json!(qty.to_string()));
            }
            OrderEventKind::ModifyRejected { reason } => {
                row.field("variant", json!("modify_rejected"));
                row.field("reason", json!(reject_reason(reason)));
            }
            OrderEventKind::Canceled { reason } => {
                row.field("variant", json!("canceled"));
                row.field(
                    "reason",
                    json!(match reason {
                        CancelReason::Requested => "requested",
                        CancelReason::Venue => "venue",
                    }),
                );
            }
            OrderEventKind::CancelRejected { reason } => {
                row.field("variant", json!("cancel_rejected"));
                row.field("reason", json!(reject_reason(reason)));
            }
        }
        self.lines.push(row.line());
    }

    pub(crate) fn gap(&mut self, instrument: InstrumentId, now: Timestamp, origin: EventOrigin) {
        let mut row = self.decision("gap");
        row.field(
            "cause",
            json!(
                origin
                    .gap_cause
                    .expect("driver validated gap origin")
                    .as_str()
            ),
        );
        row.field("instrument", json!(instrument.to_string()));
        row.field("ts_local_ns", json!(now.as_nanos().to_string()));
        row.object("raw", raw_position(origin.position));
        self.lines.push(row.line());
    }

    pub(crate) fn timer_set(
        &mut self,
        id: TimerId,
        now: Timestamp,
        deadline: Timestamp,
        registration_order: u64,
    ) {
        let mut row = self.decision("timer_set");
        row.field("timer_id", json!(id.0.to_string()));
        row.field("ts_local_ns", json!(now.as_nanos().to_string()));
        row.field("deadline_ns", json!(deadline.as_nanos().to_string()));
        row.field("registration_order", json!(registration_order.to_string()));
        self.lines.push(row.line());
    }

    pub(crate) fn timer_fired(&mut self, id: TimerId, now: Timestamp) {
        let mut row = self.decision("timer_fired");
        row.field("timer_id", json!(id.0.to_string()));
        row.field("ts_local_ns", json!(now.as_nanos().to_string()));
        self.lines.push(row.line());
    }
}

fn side(side: Side) -> &'static str {
    match side {
        Side::Buy => "buy",
        Side::Sell => "sell",
    }
}

fn reject_reason(reason: RejectReason) -> &'static str {
    match reason {
        RejectReason::WouldCross => "would_cross",
        RejectReason::BelowMinNotional => "below_min_notional",
        RejectReason::InvalidOrder => "invalid_order",
        RejectReason::DuplicateClientOrderId => "duplicate_client_order_id",
        RejectReason::UnknownOrder => "unknown_order",
        RejectReason::Unsupported => "unsupported",
    }
}

fn raw_position(position: RawPosition) -> Object {
    let mut object = Object::new();
    object.field("file_index", json!(position.file_index.to_string()));
    object.field("record_index", json!(position.record_index.to_string()));
    object
}

fn raw_extent(row: &mut Object, last_raw: Option<RawPosition>, manifest: &[ManifestEntry]) {
    match last_raw {
        Some(position) => row.object("last_raw", raw_position(position)),
        None => row.field("last_raw", Value::Null),
    }
    let entries = manifest
        .iter()
        .map(|file| {
            let mut object = Object::new();
            object.field("path", json!(file.path));
            object.field("file_seq", json!(file.file_seq.map(|seq| seq.to_string())));
            object.field("sha256", json!(file.sha256));
            object.finish()
        })
        .collect::<Vec<_>>()
        .join(",");
    row.encoded("manifest", &format!("[{entries}]"));
}

fn misses(misses: LatePrintMisses) -> Object {
    let mut object = Object::new();
    object.field("trades", json!(misses.trades));
    object.field("qty", json!(misses.qty.to_string()));
    object
}

fn account(pnl: &PnlSummary) -> Object {
    let mut object = Object::new();
    object.field("excluded", json!(["funding"]));
    object.field("cash", json!(pnl.cash.to_string()));
    object.field("position", json!(pnl.position.to_string()));
    object.field("mark_px", json!(pnl.mark_px.map(|px| px.to_string())));
    object.field(
        "mark_ts_exch_ns",
        json!(pnl.mark_ts_exch.map(|time| time.as_nanos().to_string())),
    );
    object.field("notional", json!(pnl.notional.to_string()));
    object.field("fees", json!(pnl.fees.to_string()));
    object.field("gross", json!(pnl.gross.to_string()));
    object.field("net", json!(pnl.net.to_string()));
    object.field("max_abs_position", json!(pnl.max_abs_position.to_string()));
    object.field("min_net", json!(pnl.min_net.to_string()));
    object.field(
        "min_net_ts_exch_ns",
        json!(pnl.min_net_ts_exch.map(|time| time.as_nanos().to_string())),
    );
    object
}

/// Encodes object members in insertion order using JSON's string escaping.
struct Object {
    text: String,
}

impl Object {
    fn new() -> Self {
        Self {
            text: "{".to_owned(),
        }
    }

    fn field(&mut self, name: &str, value: Value) {
        self.encoded(name, &value.to_string());
    }

    fn object(&mut self, name: &str, value: Object) {
        self.encoded(name, &value.finish());
    }

    fn encoded(&mut self, name: &str, value: &str) {
        if self.text.len() > 1 {
            self.text.push(',');
        }
        self.text.push_str(&json!(name).to_string());
        self.text.push(':');
        self.text.push_str(value);
    }

    fn finish(mut self) -> String {
        self.text.push('}');
        self.text
    }

    fn line(self) -> String {
        let mut line = self.finish();
        line.push('\n');
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_midnight_is_strict_and_handles_epoch_and_range_edges() {
        let midnight = |ns| next_utc_midnight(Timestamp::from_nanos(ns)).map(Timestamp::as_nanos);
        assert_eq!(midnight(-86_400_000_000_001), Some(-86_400_000_000_000));
        assert_eq!(midnight(-1), Some(0));
        assert_eq!(midnight(0), Some(86_400_000_000_000));
        assert_eq!(midnight(86_400_000_000_000), Some(172_800_000_000_000));
        assert_eq!(midnight(i64::MAX), None);
    }
}
