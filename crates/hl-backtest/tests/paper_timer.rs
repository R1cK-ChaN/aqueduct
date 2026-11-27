//! Replay the timer and queue-loss sessions produced by live loopback tests.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use hl_backtest::session::{ReplayComponents, check_session};
use hl_sim::{SimConfig, SimGateway};
use hl_strategy::{Actions, Context, Strategy, StrategyHost};
use hl_types::{
    Event, InstrumentId, MarketEventKind, OrderSpec, Side, TimeInForce, TimerId, Timestamp,
};
use serde_json::Value;

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct FixtureDirectory(PathBuf);

impl Drop for FixtureDirectory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove quiet-timer fixture");
    }
}

/// Independent reconstruction of the scheduler test's `test_probe` strategy.
/// Its first quote submits one order and registers two equal deadlines.
#[derive(Debug)]
struct TimerProbe {
    instrument: InstrumentId,
    quoted: bool,
}

impl Strategy for TimerProbe {
    fn on_event(&mut self, _ctx: &Context<'_>, event: &Event, actions: &mut Actions) {
        if matches!(event, Event::Market(market) if matches!(market.kind, MarketEventKind::Bbo(_)))
            && !self.quoted
        {
            self.quoted = true;
            actions.place(OrderSpec {
                instrument: self.instrument,
                side: Side::Buy,
                px: "100".parse().expect("fixture price"),
                qty: "1".parse().expect("fixture quantity"),
                tif: TimeInForce::Alo,
                reduce_only: false,
            });
            actions.set_timer(Timestamp::from_nanos(2_000_000), TimerId(9));
            actions.set_timer(Timestamp::from_nanos(2_000_000), TimerId(3));
        }
    }
}

fn run_and_check_fixture(test: &str, variable: &str) -> Vec<Value> {
    let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    let directory = FixtureDirectory(std::env::temp_dir().join(format!(
        "hl-backtest-live-timer-{}-{sequence}",
        std::process::id()
    )));
    fs::create_dir(&directory.0).expect("create loopback fixture directory");
    let identity = hl_backtest::build_identity();
    let output = Command::new(env!("CARGO"))
        .current_dir(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .args([
            "test",
            "--offline",
            "-p",
            "hl-live",
            "--lib",
            test,
            "--",
            "--exact",
        ])
        .env(variable, &directory.0)
        .output()
        .expect("run the live loopback fixture without a crate dependency");
    assert!(
        output.status.success(),
        "live loopback fixture {test} failed\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let session = directory.0.join("session.jsonl");
    let text =
        fs::read_to_string(&session).expect("the selected live test must produce its session");
    let rows: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).expect("session row"))
        .collect();
    check_session(&session, &identity, |start, _| {
        if start.strategy_name != "test_probe" || !start.strategy.is_empty() {
            return Err("unexpected quiet-timer strategy parameters".to_owned());
        }
        let config = SimConfig::from_parameters(&start.sim)?;
        Ok(ReplayComponents {
            host: StrategyHost::new(
                TimerProbe {
                    instrument: start.instrument,
                    quoted: false,
                },
                start.strategy_tag,
            ),
            sim: SimGateway::new(config),
            strategy_parameters: Vec::new(),
            sim_parameters: config.parameters(),
            rules: Vec::new(),
        })
    })
    .expect("actual checker reproduces the live session");
    rows
}

#[test]
fn loopback_quiet_timer_session_passes_the_actual_offline_checker() {
    let rows = run_and_check_fixture(
        "scheduler_tests::loopback_quiet_timer_waits_for_strict_sample_and_replays_exact_rows",
        "HL_TEST_TIMER_SESSION",
    );
    let timers: Vec<_> = rows
        .iter()
        .filter(|row| row["kind"] == "timer_fired")
        .collect();
    assert_eq!(timers.len(), 2);
    assert_eq!(timers[0]["timer_id"], "9");
    assert_eq!(timers[1]["timer_id"], "3");
    assert!(timers.iter().all(|row| row["ts_local_ns"] == "2000000"));
}

#[test]
fn loopback_full_queue_session_passes_the_actual_offline_checker() {
    let rows = run_and_check_fixture(
        "scheduler_tests::loopback_full_queue_preserves_lifecycle_and_publishes_losses_in_order",
        "HL_TEST_DROP_SESSION",
    );
    let causes: Vec<_> = rows
        .iter()
        .filter(|row| row["kind"] == "gap")
        .map(|row| row["cause"].as_str().expect("gap cause"))
        .collect();
    assert_eq!(causes, ["disconnected", "dropped", "dropped", "shutdown"]);
}
