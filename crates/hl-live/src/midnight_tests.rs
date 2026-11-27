//! Exercise the writer, scheduler and offline CLI with an explicitly advanced clock.

use std::ffi::OsStr;
use std::fs;
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use hl_recorder::Stamp;
use hl_run::ReplayTrace;
use hl_types::OrderEventKind;
use serde_json::{Value, json};
use tungstenite::WebSocket;

use crate::admission::PaperClock;
use crate::{PaperConfig, StopReader, run_paper_with_clock};

#[path = "../tests/support/mod.rs"]
mod support;
use support::{
    TIMEOUT, bbo, check_session, config_text, connect, count, listener, read_rows, trade,
};

const MIDNIGHT: i64 = 86_400_000_000_000; // 1970-01-02 00:00 UTC
const ANCHOR: i64 = MIDNIGHT - 2_000_000_000;

struct Fixture {
    stamps: Arc<Mutex<Stamp>>,
    partial: PathBuf,
}

impl Fixture {
    fn set_time(&self, wall_ns: i64, mono_ns: u64) {
        *self.stamps.lock().expect("clock") = Stamp { wall_ns, mono_ns };
    }

    fn wait_rows(&self, ready: impl Fn(&[Value]) -> bool) -> Vec<Value> {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let rows = read_rows(&self.partial);
            if ready(&rows) {
                return rows;
            }
            assert!(Instant::now() < deadline, "missing session rows: {rows:?}");
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn fill_at_midnight(&self, socket: &mut WebSocket<TcpStream>) -> Vec<Value> {
        trade(socket, (MIDNIGHT / 1_000_000) as u64, 1, "0.5");
        bbo(socket, (MIDNIGHT / 1_000_000 + 1) as u64, "98", "102");
        self.wait_rows(|rows| count(rows, "report", "filled") == 1)
    }

    fn check_open_copy(&self) {
        let rows = self.wait_rows(|rows| rows.iter().any(|r| r["kind"] == "checkpoint"));
        let checkpoint = assert_checkpoint(&rows);
        let directory = self.partial.parent().expect("session directory");
        assert!(
            directory
                .join(checkpoint["manifest"][0]["path"].as_str().unwrap())
                .is_file()
        );
        let copy = directory.join("open-copy.jsonl.part");
        fs::copy(&self.partial, &copy).expect("copy while paper is running");
        assert_eq!(
            check_session(&copy),
            "checkpoint 1970-01-02T00:00:00Z matches (4 rows)\n"
        );
    }
}

fn assert_checkpoint(rows: &[Value]) -> &Value {
    let mut checkpoints = rows.iter().filter(|row| row["kind"] == "checkpoint");
    let checkpoint = checkpoints.next().expect("one midnight crossed");
    assert!(checkpoints.next().is_none());
    assert_eq!(checkpoint["cutoff_ns"], MIDNIGHT.to_string());
    // The first BBO placed both quotes. Only those two actions precede M.
    assert_eq!(checkpoint["last_seq"], "1");
    // Header, connecting, connected, two subscriptions, then the first BBO.
    assert_eq!(
        checkpoint["last_raw"],
        json!({"file_index":"0","record_index":"5"})
    );
    assert_eq!(
        checkpoint["late_print_misses"],
        json!({"trades":0,"qty":"0"})
    );
    assert_eq!(checkpoint["manifest"].as_array().unwrap().len(), 1);
    assert_eq!(checkpoint["manifest"][0]["file_seq"], "0");
    checkpoint
}

fn run_midnight(
    name: &str,
    running: impl FnOnce(&Fixture, &mut WebSocket<TcpStream>),
    sealed: impl FnOnce(&[Value]),
) -> ReplayTrace {
    let retained = (name == "data")
        .then(|| std::env::var_os("HL_TEST_MIDNIGHT_SESSION").map(PathBuf::from))
        .flatten();
    let root = retained.clone().unwrap_or_else(|| {
        std::env::temp_dir().join(format!("hl-live-midnight-{name}-{}", std::process::id()))
    });
    fs::create_dir(&root).expect("new test output directory");
    let listener = listener();
    let config = PaperConfig::from_toml(&config_text(&root, listener.local_addr().unwrap().port()))
        .expect("loopback settings");
    let stamps = Arc::new(Mutex::new(Stamp {
        wall_ns: ANCHOR,
        mono_ns: 0,
    }));
    let clock = PaperClock::controlled(ANCHOR, Arc::clone(&stamps));
    let shutdown = AtomicBool::new(false);
    let mut retained_trace = ReplayTrace::default();
    let session = thread::scope(|scope| {
        let worker = scope.spawn(|| {
            run_paper_with_clock(&config, &shutdown, clock, |trace| {
                retained_trace.clone_from(trace)
            })
        });
        let _stop_on_unwind = StopReader(&shutdown);
        let mut socket = connect(&listener);
        let directory = fs::read_dir(&root).unwrap().next().unwrap().unwrap().path();
        let fixture = Fixture {
            stamps,
            partial: directory.join("session.jsonl.part"),
        };
        bbo(&mut socket, (ANCHOR / 1_000_000) as u64, "98", "102");
        fixture.wait_rows(|rows| count(rows, "action", "place") == 2);
        running(&fixture, &mut socket);
        shutdown.store(true, Ordering::Relaxed);
        worker
            .join()
            .expect("paper thread")
            .expect("clean stop")
            .join("session.jsonl")
    });
    let rows = read_rows(&session);
    assert_checkpoint(&rows);
    assert_eq!(rows.last().unwrap()["kind"], "end");
    assert!(check_session(&session).starts_with("session matches"));
    sealed(&rows);
    if retained.is_none() {
        fs::remove_dir_all(root).expect("remove synthetic test directory");
    }
    retained_trace
}

#[test]
fn loopback_midnight_data_waits_for_sealed_raw_and_checks_open_copy() {
    run_midnight(
        "data",
        |fixture, socket| {
            // Exchange time closes the old day while raw wall time stays in its old hour.
            let rows = fixture.fill_at_midnight(socket);
            assert!(
                rows.iter()
                    .any(|r| r["kind"] == "report" && r["ts_local_ns"] == "86400000000000")
            );
            assert!(rows.iter().all(|r| r["kind"] != "checkpoint"));
            let day = fixture.partial.parent().unwrap().join("loopback/19700101");
            assert!(
                fs::read_dir(day)
                    .unwrap()
                    .any(|p| p.unwrap().path().extension() == Some(OsStr::new("part")))
            );
            fixture.set_time(MIDNIGHT + 2_000_000, 2_002_000_000);
            fixture.check_open_copy();
        },
        |rows| {
            let end = rows.last().unwrap();
            assert_eq!(end["manifest"].as_array().unwrap().len(), 2);
            assert_eq!(end["pnl"]["position"], "0.5");
        },
    );
}

#[test]
fn loopback_paper_trace_retains_only_fills() {
    let trace = run_midnight(
        "trace",
        |fixture, socket| {
            fixture.fill_at_midnight(socket);
            fixture.set_time(MIDNIGHT + 2_000_000, 2_002_000_000);
        },
        |rows| {
            assert_eq!(count(rows, "action", "place"), 2);
            assert_eq!(count(rows, "report", "accepted"), 2);
            assert_eq!(count(rows, "report", "filled"), 1);
        },
    );
    let fills = trace
        .order_events
        .iter()
        .filter(|report| matches!(report.kind, OrderEventKind::Filled(_)))
        .count();
    assert_eq!(
        (trace.actions.len(), trace.order_events.len() - fills, fills),
        (0, 0, 1),
        "retained actions, non-fill reports and fills"
    );
}

#[test]
fn loopback_quiet_midnight_publishes_without_another_frame() {
    run_midnight(
        "quiet",
        |fixture, _| {
            fixture.set_time(MIDNIGHT, 2_000_000_000);
            fixture.check_open_copy();
        },
        |rows| assert_eq!(count(rows, "report", "accepted"), 0),
    );
}

#[test]
fn loopback_stop_after_midnight_seals_the_due_checkpoint_before_end() {
    run_midnight(
        "stop",
        |fixture, _| {
            // Shutdown crosses midnight on the process clock. The old wall hour's
            // raw file can only seal in the finalizer.
            fixture.set_time(ANCHOR, 2_000_000_001);
        },
        |rows| {
            let end = rows.last().unwrap();
            assert_eq!(end["stop_ns"], (MIDNIGHT + 1).to_string());
            assert_eq!(end["manifest"], assert_checkpoint(rows)["manifest"]);
        },
    );
}
