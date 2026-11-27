//! Capture one public feed and run a paper strategy against the simulator.
//!
//! RFC 0008 in `docs/rfc/0008-paper-trading.md` gives each raw input and its
//! strategy callback the same receipt history in paper and offline replay.
//! The socket reader stamps and enqueues under one admission lock. The
//! scheduler takes that lock to sample anchored monotonic time and detach
//! the prefix. It writes and converts the prefix outside the lock before
//! releasing any tied reports or timers.
//!
//! On shutdown the process finishes the shared driver at the raw shutdown
//! record, seals its recording, then publishes a checksummed session manifest.
//! The offline verifier is `hl-backtest run --check-session`.

mod admission;
mod config;
mod observe;
mod output;
mod scheduler;

#[cfg(test)]
mod midnight_tests;
#[cfg(test)]
mod scheduler_tests;

use std::cell::RefCell;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use hl_recorder::Clock;
use hl_run::IncrementalDriver;
use hl_run::recording::GapCause;
use hl_run::session::{Session, SessionStart};
use hl_sim::{MODEL_NAME, SimGateway};
use hl_strategy::{FixedSpreadQuoter, StrategyHost};

use admission::{Admission, PaperClock};
pub use config::PaperConfig;
use observe::{Observation, Observed};
use output::{Capture, create_directory};
use scheduler::{Pacing, ProcessedRecord};

const TICK: Duration = Duration::from_secs(1);

// A scoped join can otherwise block forever if the scheduler unwinds while
// the source is still reading. The source checks this flag on every read tick.
struct StopReader<'a>(&'a AtomicBool);

impl Drop for StopReader<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// Runs one new paper session until the supplied shutdown flag is set.
/// Returns its directory only after the raw files and session have been sealed.
pub fn run_paper(config: &PaperConfig, shutdown: &AtomicBool) -> io::Result<PathBuf> {
    run_paper_with_clock(
        config,
        shutdown,
        PaperClock::Real(Clock::start()),
        #[cfg(test)]
        |_| {},
    )
}

fn run_paper_with_clock(
    config: &PaperConfig,
    shutdown: &AtomicBool,
    clock: PaperClock,
    #[cfg(test)] inspect_trace: impl FnOnce(&hl_run::ReplayTrace),
) -> io::Result<PathBuf> {
    let anchor = clock.start_wall_ns();
    let directory = create_directory(&config.out_dir, anchor)?;
    let start = SessionStart {
        commit: env!("HL_BUILD_COMMIT").into(),
        dirty: env!("HL_BUILD_DIRTY") == "true",
        driver: "paper".into(),
        network: config.network,
        source: Some(config.source.name.clone()),
        coin: config.coin.clone(),
        instrument: config.strategy.instrument,
        process_start_wall_ns: Some(anchor),
        strategy_name: "fixed_spread".into(),
        strategy_tag: 1,
        strategy: config.strategy.parameters(),
        sim_model: MODEL_NAME.into(),
        sim: config.sim.parameters(),
    };
    let mut capture = Capture::new(&directory, &config.source, config.zstd_level, &start)?;
    let mut session = Session::new(&start);
    capture.append(session.take_lines())?;
    capture.tick(anchor)?;

    let admission = Arc::new(Admission::new(clock.clone(), config.queue_capacity));
    let mut sink = admission.sink();
    thread::scope(|scope| -> io::Result<()> {
        let reader = thread::Builder::new()
            .name("paper-source".into())
            .spawn_scoped(scope, move || {
                hl_recorder::run_source(0, &config.source, shutdown, &mut sink)
            })?;
        let _stop = StopReader(shutdown);
        let observation = RefCell::new(Observation::default());
        let observed = Observed {
            strategy: FixedSpreadQuoter::new(config.strategy),
            clock: clock.clone(),
            sim: config.sim,
            observation: &observation,
        };
        let mut host = StrategyHost::new(observed, start.strategy_tag);
        let mut gateway = SimGateway::new(config.sim);
        let mut driver = IncrementalDriver::for_paper(&mut host, &mut gateway, &mut session);
        drive(
            config,
            &clock,
            &admission,
            &reader,
            &observation,
            &mut capture,
            &mut driver,
        )?;
        reader
            .join()
            .map_err(|_| io::Error::other("paper source panicked"))?;
        let trace = driver.finish(capture.stop()?).map_err(io::Error::other)?;
        #[cfg(test)]
        inspect_trace(&trace);
        capture.append(session.take_lines())?;
        observation.borrow_mut().finish_minute();
        capture.append(observation.borrow_mut().drain(None))?;
        capture.seal(&mut session, trace, host.orders().len())
    })?;
    Ok(directory)
}

fn drive<S: hl_strategy::Strategy>(
    config: &PaperConfig,
    clock: &PaperClock,
    admission: &Admission,
    reader: &thread::ScopedJoinHandle<'_, ()>,
    observation: &RefCell<Observation>,
    capture: &mut Capture<'_>,
    driver: &mut IncrementalDriver<'_, S>,
) -> io::Result<()> {
    let mut pacing = Pacing::new(clock.start_wall_ns());
    let mut last_tick = Instant::now();
    loop {
        // Read completion before the handoff. If the reader has exited,
        // this prefix must contain every remaining record, including shutdown.
        let finished = reader.is_finished();
        let prefix = admission.take_prefix()?;
        let stopped = pacing.process_records(prefix.records, driver, |driver, record| {
            let mut events = Vec::new();
            let origins = capture.record(&record, &mut events)?;
            let latest_input = events.last().map(|event| event.ts_local);
            let shutdown = origins
                .iter()
                .any(|origin| origin.gap_cause == Some(GapCause::Shutdown));
            observation.borrow_mut().admitted_mono_ns = record.recv_mono_ns;
            for (event, origin) in events.iter().zip(origins) {
                config.validate_event(event).map_err(io::Error::other)?;
                capture.capture_due_checkpoints(driver, event.ts_local)?;
                driver
                    .on_market_event_with_origin(event, &origin)
                    .map_err(io::Error::other)?;
                capture.append(driver.take_session_lines())?;
                maintain(capture, clock, &mut last_tick)?;
            }
            maintain(capture, clock, &mut last_tick)?;
            Ok(ProcessedRecord {
                latest_input,
                shutdown,
            })
        })?;
        // Only a complete handoff can use its anchored clock to close a
        // quiet interval. A shutdown closes at the recorded stop instead.
        if !stopped {
            let sample = pacing.anchored(prefix.sample)?;
            capture.capture_due_checkpoints(driver, sample)?;
            pacing.close(driver, sample)?;
        }
        capture.append(driver.take_session_lines())?;
        capture.append(observation.borrow_mut().drain(pacing.closed_through()))?;
        if stopped {
            break;
        }
        if finished {
            return Err(io::Error::other("source exited without a shutdown record"));
        }
        maintain(capture, clock, &mut last_tick)?;
        let next = observation
            .borrow_mut()
            .next_wakeup(pacing.closed_through());
        // This outside-lock sample is used only to shorten a sleep after
        // processing. The next handoff always takes a fresh closure sample.
        let wait =
            pacing.wait_duration(clock.now(), next, TICK.saturating_sub(last_tick.elapsed()))?;
        admission.wait(wait)?;
    }
    Ok(())
}

fn maintain(
    capture: &mut Capture<'_>,
    clock: &PaperClock,
    last_tick: &mut Instant,
) -> io::Result<()> {
    if last_tick.elapsed() >= TICK {
        capture.tick(clock.now().wall_ns)?;
        *last_tick = Instant::now();
    }
    Ok(())
}
