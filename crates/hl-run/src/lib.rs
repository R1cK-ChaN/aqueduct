//! Drive a strategy and simulator with market data in receipt order.
//!
//! [`IncrementalDriver`] accepts one market or gap event at a time. The caller
//! closes a local timestamp once it knows no further input at or before that
//! time will arrive. [`replay`] supplies this driver from a slice and finishes
//! at an explicit inclusive stop. The [`recording`] module converts raw
//! records and supplies their stop time. The [`pnl`] module values delivered
//! fills after the run.
//!
//! # Event order
//!
//! At each local timestamp, process work in this order, as specified by
//! RFC 0008 D2, D3 and D7 in `docs/rfc/0008-paper-trading.md`.
//!
//! 1. Every market and gap input, in receipt order. Each goes to the simulator
//!    before its strategy callback, whose actions are submitted immediately.
//! 2. Already determined simulator reports, in delivery order.
//! 3. Timers, in registration order, rechecking reports after each callback.
//!
//! A later input closes earlier timestamps. Explicit closure also releases
//! reports and timers during silence. Unproved actions wait for qualifying
//! market evidence, and work after the inclusive stop stays pending.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::fmt;
use std::path::Path;

use hl_codec::hyperliquid::{Network, SelectedRules, parse_rules_file};
use hl_gateway::OrderGateway;
use hl_sim::{LatePrintMisses, SimError, SimGateway};
use hl_strategy::{Strategy, StrategyHost};
use hl_types::{
    Event, MarketEvent, MarketEventKind, OrderAction, OrderEvent, OrderEventKind, TimerEvent,
    TimerId, Timestamp,
};

use recording::EventOrigin;
use session::{CheckpointBoundary, Session};

pub mod pnl;
pub mod recording;
pub mod session;

/// Selects rules from the text read by the driver and names the file on error.
pub fn select_rules(
    path: &Path,
    text: std::io::Result<String>,
    network: Network,
    coin: &str,
) -> Result<SelectedRules, String> {
    let with_path = |cause: &dyn fmt::Display| format!("rules file {}: {cause}", path.display());
    let text = text.map_err(|error| with_path(&error))?;
    parse_rules_file(&text)
        .and_then(|file| file.select(network, coin))
        .map_err(|error| with_path(&error))
}

/// Why the driver could not process an input or close a timestamp. `index`
/// is the zero-based position of the offending market or gap event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayError {
    /// An event's `ts_local` is earlier than its `ts_exch`.
    LocalBeforeExchange {
        /// Position of the event.
        index: usize,
    },
    /// An event's `ts_local` is earlier than the previous event's.
    LocalTimeDecreases {
        /// Position of the event.
        index: usize,
    },
    /// Input arrived at or before a timestamp the caller already closed.
    TimeAlreadyClosed {
        /// Position of the event.
        index: usize,
    },
    /// A closure precedes the latest input or an earlier explicit closure.
    CloseTimeDecreases,
    /// Capturing the last work sequence requires a session.
    CheckpointWithoutSession,
    /// The number of provenance entries differs from the market input count.
    OriginCount {
        /// Number of market events.
        events: usize,
        /// Number of provenance entries.
        origins: usize,
    },
    /// A gap lacks its cause and position, or a non-gap carries a gap cause.
    GapOriginMismatch {
        /// Position of the event.
        index: usize,
    },
    /// The simulator could not select rules for an action at its effect time.
    Sim(SimError),
}

impl ReplayError {
    /// Includes the driver's network and coin when reporting a simulator coverage error.
    pub fn for_pair(&self, network: Network, coin: &str) -> String {
        match self {
            Self::Sim(error) => format!("{} {coin}: {error}", network.short_name()),
            _ => self.to_string(),
        }
    }
}

impl fmt::Display for ReplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LocalBeforeExchange { index } => {
                write!(f, "market event {index}: ts_local is earlier than ts_exch")
            }
            Self::LocalTimeDecreases { index } => {
                write!(
                    f,
                    "market event {index}: ts_local is earlier than the previous event's"
                )
            }
            Self::TimeAlreadyClosed { index } => {
                write!(f, "market event {index}: ts_local is already closed")
            }
            Self::CloseTimeDecreases => {
                f.write_str("closure precedes the latest input or previous closure")
            }
            Self::CheckpointWithoutSession => f.write_str("checkpoint requires a session"),
            Self::OriginCount { events, origins } => {
                write!(f, "{events} market events have {origins} origin entries")
            }
            Self::GapOriginMismatch { index } => {
                write!(
                    f,
                    "market event {index}: feed gap and raw gap cause disagree"
                )
            }
            Self::Sim(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ReplayError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sim(error) => Some(error),
            _ => None,
        }
    }
}

/// Work left when the inclusive stop has been processed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PendingAtEnd {
    /// Submitted actions without qualifying market evidence.
    pub actions: usize,
    /// Determined reports whose delivery time exceeds the stop.
    pub reports: usize,
    /// Registered timers whose firing time exceeds the stop.
    pub timers: usize,
}

/// What happened during a replay.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReplayTrace {
    /// Requested actions in order, with their local times. Empty in paper mode.
    pub actions: Vec<(Timestamp, OrderAction)>,
    /// Delivered reports in order. Paper retains only fills.
    pub order_events: Vec<OrderEvent>,
    /// Work remaining after the inclusive stop.
    pub pending_at_end: PendingAtEnd,
    /// Late trades allocated to superseded versions, without producing fills.
    pub late_print_misses: LatePrintMisses,
}

// Keep the scheduler testable with an immediate-report gateway. Public
// driving remains specific to SimGateway and its market-evidence contract.
trait ScheduledGateway: OrderGateway {
    fn next_wakeup(&self) -> Option<Timestamp>;
}

impl ScheduledGateway for SimGateway {
    fn next_wakeup(&self) -> Option<Timestamp> {
        SimGateway::next_wakeup(self)
    }
}

/// Declaration order gives reports priority over tied timers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Step {
    Reports,
    Timer,
}

#[derive(Debug)]
struct Scheduler<'a, S, G> {
    host: &'a mut StrategyHost<S>,
    gateway: &'a mut G,
    /// Pending timers: fire time, then the order in which they were set.
    timers: BinaryHeap<Reverse<(Timestamp, u64, TimerId)>>,
    timers_set: u64,
    trace: ReplayTrace,
    retain_full_trace: bool,
    session: Option<&'a mut Session>,
}

impl<'a, S: Strategy, G: ScheduledGateway> Scheduler<'a, S, G> {
    fn new(host: &'a mut StrategyHost<S>, gateway: &'a mut G) -> Self {
        Self {
            host,
            gateway,
            timers: BinaryHeap::new(),
            timers_set: 0,
            trace: ReplayTrace::default(),
            retain_full_trace: true,
            session: None,
        }
    }

    /// Hands one event to the strategy and routes what it asks for.
    fn deliver(&mut self, event: &Event) {
        let now = event.ts_local();
        let actions = self.host.on_event(event);
        if self.retain_full_trace {
            self.trace
                .actions
                .extend(actions.order_actions().iter().map(|&action| (now, action)));
        }
        if let Some(session) = self.session.as_deref_mut() {
            for action in actions.order_actions() {
                session.action(now, action);
            }
        }
        self.gateway.submit(now, actions.order_actions());
        for request in actions.timer_requests() {
            if let Some(session) = self.session.as_deref_mut() {
                session.timer_set(request.id, now, request.at, self.timers_set);
            }
            self.timers
                .push(Reverse((request.at.max(now), self.timers_set, request.id)));
            self.timers_set += 1;
        }
    }

    fn run_due(&mut self, within_cutoff: impl Fn(Timestamp) -> bool) {
        let mut reports = Vec::new();
        loop {
            let next_report = self.gateway.next_wakeup().map(|at| (at, Step::Reports));
            let next_timer = self
                .timers
                .peek()
                .map(|Reverse((at, _, _))| (*at, Step::Timer));
            let Some((now, step)) = next_report.into_iter().chain(next_timer).min() else {
                break;
            };
            if !within_cutoff(now) {
                break;
            }
            match step {
                Step::Reports => {
                    reports.clear();
                    self.gateway.poll_events(now, &mut reports);
                    for &report in &reports {
                        if let Some(session) = self.session.as_deref_mut() {
                            session.report(&report);
                        }
                        if self.retain_full_trace
                            || matches!(report.kind, OrderEventKind::Filled(_))
                        {
                            self.trace.order_events.push(report);
                        }
                        self.deliver(&Event::Order(report));
                    }
                }
                Step::Timer => {
                    let Reverse((at, _, id)) = self.timers.pop().expect("timer was selected");
                    if let Some(session) = self.session.as_deref_mut() {
                        session.timer_fired(id, at);
                    }
                    self.deliver(&Event::Timer(TimerEvent { id, ts_local: at }));
                }
            }
        }
    }
}

fn validate_event(
    event: &MarketEvent,
    previous_local: Option<Timestamp>,
    index: usize,
) -> Result<(), ReplayError> {
    if event.ts_local < event.ts_exch {
        return Err(ReplayError::LocalBeforeExchange { index });
    }
    if previous_local.is_some_and(|previous| event.ts_local < previous) {
        return Err(ReplayError::LocalTimeDecreases { index });
    }
    Ok(())
}

fn validate_origin(
    event: &MarketEvent,
    origin: Option<&EventOrigin>,
    index: usize,
) -> Result<(), ReplayError> {
    let is_gap = matches!(event.kind, MarketEventKind::FeedGap);
    let has_cause = origin.is_some_and(|origin| origin.gap_cause.is_some());
    if is_gap != has_cause {
        return Err(ReplayError::GapOriginMismatch { index });
    }
    Ok(())
}

/// A receipt-ordered strategy run with caller-controlled timestamp closure.
///
/// Supply all inputs tied at a local timestamp before closing that timestamp.
/// Each input closes every earlier time automatically. This permits a paper
/// source to close quiet intervals explicitly and replay to close them by
/// looking ahead, with identical scheduling in both cases.
#[derive(Debug)]
pub struct IncrementalDriver<'a, S> {
    scheduler: Scheduler<'a, S, SimGateway>,
    last_input: Option<Timestamp>,
    closed_through: Option<Timestamp>,
    input_count: usize,
}

impl<'a, S: Strategy> IncrementalDriver<'a, S> {
    /// Starts a run with an empty timer queue and trace.
    pub fn new(host: &'a mut StrategyHost<S>, sim: &'a mut SimGateway) -> Self {
        Self {
            scheduler: Scheduler::new(host, sim),
            last_input: None,
            closed_through: None,
            input_count: 0,
        }
    }

    /// Starts a run that emits rows into the supplied session as work occurs.
    pub fn with_session(
        host: &'a mut StrategyHost<S>,
        sim: &'a mut SimGateway,
        session: &'a mut Session,
    ) -> Self {
        let mut driver = Self::new(host, sim);
        driver.scheduler.session = Some(session);
        driver
    }

    /// Emits every work row while retaining only delivered fills for paper accounting.
    pub fn for_paper(
        host: &'a mut StrategyHost<S>,
        sim: &'a mut SimGateway,
        session: &'a mut Session,
    ) -> Self {
        let mut driver = Self::with_session(host, sim, session);
        driver.scheduler.retain_full_trace = false;
        driver
    }

    /// Drains encoded lines while an incremental session run is active.
    pub fn take_session_lines(&mut self) -> Vec<String> {
        self.scheduler
            .session
            .as_deref_mut()
            .map_or_else(Vec::new, Session::take_lines)
    }

    /// Processes one input after releasing all work strictly before its local
    /// time. Tied reports and timers wait for closure or a later input.
    ///
    /// Returns an error without processing the input if its local time is
    /// before its exchange time, decreases, or has already been closed.
    /// A failed simulator rule lookup stops replay before the strategy receives the input.
    pub fn on_market_event(&mut self, event: &MarketEvent) -> Result<(), ReplayError> {
        self.process_market_event(event, None)
    }

    /// Processes one input with its raw provenance, emitting a gap row before
    /// the gap's strategy callback when a session is active.
    ///
    /// A gap must carry a cause and a non-gap must have no cause. Violations
    /// return an error before the simulator or strategy processes the input.
    pub fn on_market_event_with_origin(
        &mut self,
        event: &MarketEvent,
        origin: &EventOrigin,
    ) -> Result<(), ReplayError> {
        self.process_market_event(event, Some(origin))
    }

    fn process_market_event(
        &mut self,
        event: &MarketEvent,
        origin: Option<&EventOrigin>,
    ) -> Result<(), ReplayError> {
        validate_event(event, self.last_input, self.input_count)?;
        if origin.is_some() || self.scheduler.session.is_some() {
            validate_origin(event, origin, self.input_count)?;
        }
        if self
            .closed_through
            .is_some_and(|closed| event.ts_local <= closed)
        {
            return Err(ReplayError::TimeAlreadyClosed {
                index: self.input_count,
            });
        }
        self.scheduler.run_due(|at| at < event.ts_local);
        self.scheduler
            .gateway
            .on_market_event(event)
            .map_err(ReplayError::Sim)?;
        if matches!(event.kind, MarketEventKind::FeedGap)
            && let Some(session) = self.scheduler.session.as_deref_mut()
        {
            session.gap(
                event.instrument,
                event.ts_local,
                *origin.expect("session gap origin was validated"),
            );
        }
        self.scheduler.deliver(&Event::Market(event.clone()));
        self.last_input = Some(event.ts_local);
        self.input_count += 1;
        Ok(())
    }

    /// Declares that no more input at or before `time` will arrive and runs
    /// reports and timers through that inclusive time.
    ///
    /// A closure may equal the previous closure. A closure before the latest
    /// input or previous closure returns an error without processing work.
    pub fn close_through(&mut self, time: Timestamp) -> Result<(), ReplayError> {
        if self.last_input.is_some_and(|latest| time < latest)
            || self.closed_through.is_some_and(|closed| time < closed)
        {
            return Err(ReplayError::CloseTimeDecreases);
        }
        self.scheduler.run_due(|at| at <= time);
        self.closed_through = Some(time);
        Ok(())
    }

    /// Closes times strictly before `cutoff` and captures the resulting work
    /// sequence and cumulative misses together. Supply all earlier inputs
    /// before calling this method, including those tied at the preceding nanosecond.
    /// A prior input or inclusive closure at or after the cutoff is an error.
    pub fn checkpoint_before(
        &mut self,
        cutoff: Timestamp,
    ) -> Result<CheckpointBoundary, ReplayError> {
        if self.scheduler.session.is_none() {
            return Err(ReplayError::CheckpointWithoutSession);
        }
        if let Some(before) = cutoff.as_nanos().checked_sub(1) {
            self.close_through(Timestamp::from_nanos(before))?;
        } else if self.last_input.is_some() || self.closed_through.is_some() {
            return Err(ReplayError::CloseTimeDecreases);
        }
        let session = self
            .scheduler
            .session
            .as_deref()
            .expect("validated session");
        Ok(CheckpointBoundary {
            cutoff,
            last_seq: session.last_seq(),
            late_print_misses: self.scheduler.gateway.late_print_misses(),
        })
    }

    /// Closes the inclusive stop and returns the trace and remaining work.
    ///
    /// The stop must be at least the latest input and explicit closure.
    pub fn finish(mut self, stop: Timestamp) -> Result<ReplayTrace, ReplayError> {
        self.close_through(stop)?;
        self.scheduler.trace.pending_at_end = PendingAtEnd {
            actions: self.scheduler.gateway.pending_actions(),
            reports: self.scheduler.gateway.undelivered_reports(),
            timers: self.scheduler.timers.len(),
        };
        self.scheduler.trace.late_print_misses = self.scheduler.gateway.late_print_misses();
        Ok(self.scheduler.trace)
    }
}

/// Replays the input through an [`IncrementalDriver`] to an inclusive stop.
///
/// `market` must have nondecreasing `ts_local`, with `ts_local >= ts_exch`
/// for every event. The complete slice is validated before changing the host
/// or simulator. Inputs after `stop` are validated but are not processed.
/// Reports and timers due at `stop` are processed. Later work remains in the
/// trace's end counts, including unproved actions awaiting market evidence.
pub fn replay<S: Strategy>(
    host: &mut StrategyHost<S>,
    sim: &mut SimGateway,
    market: &[MarketEvent],
    stop: Timestamp,
) -> Result<ReplayTrace, ReplayError> {
    let mut previous_local = None;
    for (index, event) in market.iter().enumerate() {
        validate_event(event, previous_local, index)?;
        previous_local = Some(event.ts_local);
    }
    let mut driver = IncrementalDriver::new(host, sim);
    for event in market.iter().take_while(|event| event.ts_local <= stop) {
        driver.on_market_event(event)?;
    }
    driver.finish(stop)
}

/// Replays inputs through the shared driver while encoding session work.
///
/// `origins` must have one entry per market event. Every gap must have its
/// cause and raw position. The complete input and provenance slices are
/// validated before changing the host or simulator. Inputs after the
/// inclusive stop are validated but are not processed.
pub fn replay_with_session<S: Strategy>(
    host: &mut StrategyHost<S>,
    sim: &mut SimGateway,
    market: &[MarketEvent],
    origins: &[EventOrigin],
    stop: Timestamp,
    session: &mut Session,
) -> Result<ReplayTrace, ReplayError> {
    if market.len() != origins.len() {
        return Err(ReplayError::OriginCount {
            events: market.len(),
            origins: origins.len(),
        });
    }
    let mut previous_local = None;
    for (index, (event, origin)) in market.iter().zip(origins).enumerate() {
        validate_event(event, previous_local, index)?;
        validate_origin(event, Some(origin), index)?;
        previous_local = Some(event.ts_local);
    }
    let mut driver = IncrementalDriver::with_session(host, sim, session);
    for (event, origin) in market
        .iter()
        .zip(origins)
        .take_while(|(event, _)| event.ts_local <= stop)
    {
        driver.on_market_event_with_origin(event, origin)?;
    }
    driver.finish(stop)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use hl_strategy::{Actions, Context};
    use hl_types::{
        InstrumentId, MarketEventKind, OrderEventKind, OrderId, OrderSpec, Side, TimeInForce,
    };

    use super::*;

    #[derive(Default)]
    struct ImmediateGateway {
        reports: VecDeque<OrderEvent>,
    }

    impl OrderGateway for ImmediateGateway {
        fn submit(&mut self, now: Timestamp, actions: &[OrderAction]) {
            for action in actions {
                if let OrderAction::Place { cloid, order } = action {
                    self.reports.push_back(OrderEvent {
                        instrument: order.instrument,
                        cloid: *cloid,
                        ts_exch: now,
                        ts_local: now,
                        kind: OrderEventKind::Accepted { oid: OrderId(1) },
                    });
                }
            }
        }

        fn poll_events(&mut self, now: Timestamp, out: &mut Vec<OrderEvent>) {
            while self
                .reports
                .front()
                .is_some_and(|event| event.ts_local <= now)
            {
                out.push(self.reports.pop_front().unwrap());
            }
        }
    }

    impl ScheduledGateway for ImmediateGateway {
        fn next_wakeup(&self) -> Option<Timestamp> {
            self.reports.front().map(|event| event.ts_local)
        }
    }

    #[derive(Debug, Default)]
    struct TimerActions {
        seen: Vec<(Timestamp, &'static str)>,
    }

    impl Strategy for TimerActions {
        fn on_event(&mut self, ctx: &Context<'_>, event: &Event, actions: &mut Actions) {
            match event {
                Event::Market(_) => {
                    self.seen.push((ctx.now(), "market"));
                    actions.set_timer(ctx.now(), TimerId(2));
                    actions.set_timer(ctx.now(), TimerId(1));
                }
                Event::Timer(timer) if timer.id == TimerId(2) => {
                    self.seen.push((ctx.now(), "first timer"));
                    actions.place(OrderSpec {
                        instrument: InstrumentId::new(1),
                        side: Side::Buy,
                        px: "10".parse().unwrap(),
                        qty: "1".parse().unwrap(),
                        tif: TimeInForce::Alo,
                        reduce_only: false,
                    });
                }
                Event::Timer(_) => self.seen.push((ctx.now(), "second timer")),
                Event::Order(_) => self.seen.push((ctx.now(), "report")),
            }
        }
    }

    #[test]
    fn timer_action_reports_are_delivered_before_the_next_tied_timer() {
        let now = Timestamp::from_nanos(10);
        let mut host = StrategyHost::new(TimerActions::default(), 1);
        let mut gateway = ImmediateGateway::default();
        let mut scheduler = Scheduler::new(&mut host, &mut gateway);
        scheduler.deliver(&Event::Market(MarketEvent {
            instrument: InstrumentId::new(1),
            ts_exch: now,
            ts_local: now,
            kind: MarketEventKind::FeedGap,
        }));
        scheduler.run_due(|at| at <= now);
        // Both timers are registered at 10. The first submits at 10 and
        // this test gateway produces its report at 10+0=10. Recheck reports
        // before taking the second timer, preserving registration order 2,1.
        assert_eq!(
            host.strategy().seen,
            [
                (now, "market"),
                (now, "first timer"),
                (now, "report"),
                (now, "second timer"),
            ]
        );
    }

    #[test]
    fn fills_only_scheduler_discards_actions_and_acknowledgements_during_the_run() {
        let mut host = StrategyHost::new(TimerActions::default(), 1);
        let mut gateway = ImmediateGateway::default();
        let mut scheduler = Scheduler::new(&mut host, &mut gateway);
        scheduler.retain_full_trace = false;
        for ms in 1..=256 {
            let now = Timestamp::from_millis(ms);
            scheduler.deliver(&Event::Market(MarketEvent {
                instrument: InstrumentId::new(1),
                ts_exch: now,
                ts_local: now,
                kind: MarketEventKind::FeedGap,
            }));
            scheduler.run_due(|at| at <= now);
        }
        assert_eq!(scheduler.host.strategy().seen.len(), 4 * 256);
        assert_eq!(scheduler.trace.actions.capacity(), 0);
        assert_eq!(scheduler.trace.order_events.capacity(), 0);
    }
}
