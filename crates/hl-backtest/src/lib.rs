//! Read raw recordings and verify offline strategy sessions with [`hl_run`].

use hl_run::pnl::{PnlSummary, summarize};
use hl_run::session::{ManifestEntry, Session, SessionEnd, SessionStart};
use hl_run::{ReplayTrace, replay_with_session};
use hl_sim::SimGateway;
use hl_strategy::{Strategy, StrategyHost};

use crate::recording::Recording;
use crate::session::BuildIdentity;

pub mod recording;
pub mod session;

/// Git identity sampled when Cargo built this copy of the library.
pub fn build_identity() -> BuildIdentity {
    BuildIdentity {
        commit: env!("HL_BUILD_COMMIT").to_owned(),
        dirty: env!("HL_BUILD_DIRTY") == "true",
    }
}

/// Run the supplied components from raw-derived inputs and seal their session in memory.
/// The ledger includes only marks and delivered fills through the inclusive stop.
pub fn replay_session<S: Strategy>(
    recording: &Recording,
    start: &SessionStart,
    manifest: Vec<ManifestEntry>,
    host: &mut StrategyHost<S>,
    sim: &mut SimGateway,
) -> Result<(Session, ReplayTrace, PnlSummary), String> {
    let stop = recording.stop_time.ok_or("no complete recording header")?;
    let mut session = Session::new(start);
    let trace = replay_with_session(
        host,
        sim,
        &recording.events,
        &recording.origins,
        stop,
        &mut session,
    )
    .map_err(|error| error.to_string())?;
    let end = session_end(recording, start, manifest, host.orders().len(), &trace)?;
    let pnl = end.pnl;
    session.finish(&end);
    Ok((session, trace, pnl))
}

fn session_end(
    recording: &Recording,
    start: &SessionStart,
    manifest: Vec<ManifestEntry>,
    open_orders: usize,
    trace: &ReplayTrace,
) -> Result<SessionEnd, String> {
    Ok(SessionEnd {
        stop: recording.stop_time.ok_or("no complete recording header")?,
        last_raw: recording.last_position,
        manifest,
        pnl: summarize(start.instrument, &recording.events, &trace.order_events)?,
        open_orders,
        pending: trace.pending_at_end,
        late_print_misses: trace.late_print_misses,
    })
}
