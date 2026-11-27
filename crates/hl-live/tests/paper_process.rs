//! Exercise the production CLI with synthetic WebSocket frames on loopback.
//!
//! Each stage waits for the previous stage's flushed session rows. The waits
//! bound failures without using elapsed sleeps as evidence that work finished.

#![cfg(unix)]

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hl_codec::rawlog::{Record, RecordKind};
use hl_recorder::open_recording;
use serde_json::Value;

mod support;
use support::{
    TIMEOUT, bbo, check_session, config_text, connect, count, listener, read_rows, trade,
};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    directory: PathBuf,
    keep: bool,
}

impl Fixture {
    fn new(keep: bool) -> Self {
        let directory = if keep {
            std::env::var_os("HL_TEST_KEEP_SESSION").map(PathBuf::from)
        } else {
            None
        };
        let keep = directory.is_some();
        let directory = directory.unwrap_or_else(|| {
            std::env::temp_dir().join(format!(
                "hl-live-process-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ))
        });
        fs::create_dir(&directory).expect("create unique synthetic fixture directory");
        Self { directory, keep }
    }

    fn out_dir(&self) -> PathBuf {
        self.directory.join("runs")
    }

    fn start(&self, port: u16, run: usize) -> PaperProcess {
        let config = self.directory.join(format!("paper-{run}.toml"));
        let log = self.directory.join(format!("paper-{run}.log"));
        fs::write(&config, config_text(&self.out_dir(), port)).expect("write loopback config");
        let child = Command::new(env!("CARGO_BIN_EXE_hl-live"))
            .args(["paper", "--config"])
            .arg(config)
            .stdout(Stdio::null())
            .stderr(File::create(&log).expect("create process log"))
            .spawn()
            .expect("start paper process");
        PaperProcess {
            child: Some(child),
            out_dir: self.out_dir(),
            log,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }
}

struct PaperProcess {
    child: Option<Child>,
    out_dir: PathBuf,
    log: PathBuf,
}

impl PaperProcess {
    fn wait_rows(&mut self, predicate: impl Fn(&[Value]) -> bool) -> Vec<Value> {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let rows = self
                .partial()
                .map(|path| read_rows(&path))
                .unwrap_or_default();
            if predicate(&rows) {
                return rows;
            }
            assert!(
                self.child
                    .as_mut()
                    .expect("running child")
                    .try_wait()
                    .expect("poll child")
                    .is_none(),
                "paper exited before expected rows: {}",
                self.log_text()
            );
            assert!(
                Instant::now() < deadline,
                "expected session rows did not arrive; rows={rows:?}; log={}",
                self.log_text()
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn partial(&self) -> Option<PathBuf> {
        fs::read_dir(&self.out_dir).ok()?.find_map(|entry| {
            let path = entry.ok()?.path().join("session.jsonl.part");
            path.is_file().then_some(path)
        })
    }

    fn log_text(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }

    fn wait_connect_failure(&mut self) {
        let deadline = Instant::now() + TIMEOUT;
        while !self.log_text().contains("connect_failed") {
            assert!(
                self.child
                    .as_mut()
                    .expect("running child")
                    .try_wait()
                    .expect("poll connecting child")
                    .is_none(),
                "paper exited during connection attempt: {}",
                self.log_text()
            );
            assert!(
                Instant::now() < deadline,
                "connection attempt did not finish"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn stop(mut self, signal: &str) -> PathBuf {
        self.wait_rows(|rows| !rows.is_empty());
        let partial = self.partial().expect("active session file");
        let child = self.child.as_mut().expect("running child");
        assert!(
            Command::new("kill")
                .args(["-s", signal, &child.id().to_string()])
                .status()
                .expect("send requested stop signal")
                .success()
        );
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some(status) = self
                .child
                .as_mut()
                .expect("running child")
                .try_wait()
                .expect("poll stopping child")
            {
                assert!(status.success(), "paper failed: {}", self.log_text());
                self.child.take();
                break;
            }
            assert!(Instant::now() < deadline, "paper did not stop");
            thread::sleep(Duration::from_millis(5));
        }
        assert!(!partial.exists(), "clean stop seals the partial session");
        let session = partial.with_file_name("session.jsonl");
        assert!(session.is_file(), "clean stop writes session.jsonl");
        session
    }
}

impl Drop for PaperProcess {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_millis()
        .try_into()
        .expect("milliseconds fit u64")
}

// Exchange timestamps have millisecond resolution. Always put a proving
// frame strictly after the nanosecond submission it is meant to prove.
fn after_last_callback(rows: &[Value]) -> u64 {
    let ns: u64 = rows
        .iter()
        .rev()
        .find_map(|row| row["ts_local_ns"].as_str())
        .expect("session callback time")
        .parse()
        .expect("nanoseconds");
    now_ms().max(ns / 1_000_000 + 1)
}

fn assert_diagnostics(rows: &[Value], expected_callbacks: u64) {
    let mut next_input = 0;
    let mut previous_minute = None;
    for row in rows.iter().filter(|row| row["kind"] == "diagnostic") {
        assert!(row.get("seq").is_none());
        let number = |field: &str| {
            row[field]
                .as_str()
                .expect("diagnostic integer string")
                .parse::<u64>()
                .expect("diagnostic integer")
        };
        let minute = number("minute_start_ns");
        assert_eq!(minute % 60_000_000_000, 0);
        if let Some(previous) = previous_minute {
            assert!(minute > previous, "one diagnostic per UTC minute");
        }
        previous_minute = Some(minute);
        let first = number("first_input");
        let last = number("last_input");
        assert_eq!(first, next_input);
        assert_eq!(row["callbacks"].as_u64(), Some(last - first + 1));
        for field in ["max_lag_input", "max_duration_input"] {
            assert!((first..=last).contains(&number(field)));
        }
        number("max_lag_ns");
        number("max_duration_ns");
        let rss = row
            .get("resident_memory_bytes")
            .expect("diagnostic RSS field");
        if cfg!(target_os = "linux") && Path::new("/proc/self/status").is_file() {
            assert!(
                rss.as_u64()
                    .is_some_and(|bytes| bytes > 0 && bytes % 1024 == 0)
            );
        } else {
            assert!(rss.is_null() || rss.is_u64());
        }
        next_input = last + 1;
    }
    assert_eq!(next_input, expected_callbacks);
}

fn complete_session(path: &Path) -> (Vec<Value>, Vec<Record>) {
    let rows = read_rows(path);
    let start = &rows[0];
    assert_eq!(start["kind"], "start");
    assert_eq!(start["driver"], "paper");
    assert_eq!(start["source"], "loopback");
    let end = rows.last().expect("session footer");
    assert_eq!(end["kind"], "end");
    assert!(end["last_raw"].is_object());
    let entries = end["manifest"].as_array().expect("manifest array");
    assert!(!entries.is_empty());
    let directory = path.parent().expect("session directory");
    let mut records = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        assert_eq!(entry["file_seq"], index.to_string());
        let relative = Path::new(entry["path"].as_str().expect("manifest path"));
        assert!(relative.is_relative());
        assert!(relative.to_string_lossy().ends_with(".hlrec.zst"));
        let file_records: Vec<_> = open_recording(&directory.join(relative))
            .expect("open sealed raw file")
            .map(|record| record.expect("complete raw record"))
            .collect();
        let header: Value = serde_json::from_slice(&file_records[0].payload).expect("raw header");
        assert_eq!(header["event"], "file_open");
        assert_eq!(header["file_seq"], index as u64);
        assert_eq!(header["source"], "loopback");
        assert_eq!(
            header["process_start_wall_ns"]
                .as_i64()
                .expect("raw anchor")
                .to_string(),
            start["process_start_wall_ns"]
                .as_str()
                .expect("session anchor")
        );
        records.extend(file_records);
    }
    let last = records.last().expect("raw shutdown record");
    assert_eq!(last.kind, RecordKind::Meta);
    let shutdown: Value = serde_json::from_slice(&last.payload).expect("raw metadata");
    assert_eq!(shutdown["event"], "shutdown");
    assert!(check_session(path).starts_with("session matches"));
    (rows, records)
}

#[test]
fn end_to_end_places_accepts_fills_modifies_and_checks_after_sigint() {
    let fixture = Fixture::new(true);
    let listener = listener();
    let mut paper = fixture.start(listener.local_addr().expect("port").port(), 0);
    let mut socket = connect(&listener);

    bbo(&mut socket, now_ms(), "98", "102");
    let placed = paper.wait_rows(|rows| count(rows, "action", "place") == 2);
    trade(&mut socket, after_last_callback(&placed), 1, "0.5");
    let filled = paper.wait_rows(|rows| {
        count(rows, "report", "accepted") == 2 && count(rows, "report", "filled") == 1
    });
    bbo(&mut socket, after_last_callback(&filled), "99", "103");
    let modified = paper.wait_rows(|rows| count(rows, "action", "modify") == 2);
    bbo(&mut socket, after_last_callback(&modified), "99", "103");
    paper.wait_rows(|rows| count(rows, "report", "modified") == 2);

    let path = paper.stop("INT");
    let (rows, _) = complete_session(&path);
    let end = rows.last().expect("end");
    assert_eq!(end["pnl"]["position"], "0.5");
    assert_eq!(end["pnl"]["fees"], "0.007425");
    assert_eq!(end["pending_actions"], 1, "shutdown cancel awaits evidence");
    assert_eq!(end["undelivered_reports"], 0);
    assert_eq!(end["outstanding_timers"], 0);
    assert_diagnostics(&rows, 5);
    if fixture.keep {
        eprintln!("manual check session: {}", path.display());
    }
}

#[test]
fn reconnect_filters_history_cancels_and_resumes_before_sigterm() {
    let fixture = Fixture::new(false);
    let listener = listener();
    let mut paper = fixture.start(listener.local_addr().expect("port").port(), 0);
    let mut first = connect(&listener);
    bbo(&mut first, now_ms(), "98", "102");
    let placed = paper.wait_rows(|rows| count(rows, "action", "place") == 2);
    let history_time = after_last_callback(&placed);
    // This live print proves the placements and partially fills the bid.
    trade(&mut first, history_time, 1, "0.5");
    paper.wait_rows(|rows| count(rows, "report", "filled") == 1);
    first.close(None).expect("close first connection");
    drop(first);
    let disconnected = paper.wait_rows(|rows| {
        rows.iter()
            .any(|row| row["kind"] == "gap" && row["cause"] == "disconnected")
            && count(rows, "action", "cancel_all") == 1
    });

    let mut second = connect(&listener);
    trade(&mut second, history_time, 2, "10");
    bbo(&mut second, after_last_callback(&disconnected), "98", "102");
    let canceled = paper.wait_rows(|rows| count(rows, "report", "canceled") == 2);
    bbo(&mut second, after_last_callback(&canceled), "98", "102");
    paper.wait_rows(|rows| count(rows, "action", "place") == 4);

    let path = paper.stop("TERM");
    let (rows, records) = complete_session(&path);
    assert_eq!(count(&rows, "report", "filled"), 1, "history cannot fill");
    assert_eq!(rows.last().expect("end")["pnl"]["position"], "0.5");
    assert_diagnostics(&rows, 6);
    let connected: Vec<Value> = records
        .iter()
        .filter(|record| record.kind == RecordKind::Meta)
        .map(|record| serde_json::from_slice(&record.payload).expect("meta JSON"))
        .filter(|row: &Value| row["event"] == "connected")
        .collect();
    assert_eq!(connected.len(), 2);
}

#[test]
fn mid_hour_restart_creates_two_independent_sealed_sessions_without_a_bbo() {
    let fixture = Fixture::new(false);
    let listener = listener();
    let port = listener.local_addr().expect("port").port();
    let mut paths = Vec::new();
    let mut anchors = Vec::new();
    for run in 0..2 {
        let paper = fixture.start(port, run);
        let _socket = connect(&listener);
        let path = paper.stop(if run == 0 { "INT" } else { "TERM" });
        let (rows, _) = complete_session(&path);
        assert_eq!(count(&rows, "action", "place"), 0);
        assert_eq!(rows.last().expect("end")["pnl"]["position"], "0");
        assert_diagnostics(&rows, 1);
        anchors.push(rows[0]["process_start_wall_ns"].clone());
        paths.push(path);
    }
    assert_ne!(paths[0].parent(), paths[1].parent());
    assert_ne!(anchors[0], anchors[1]);
    assert!(paths[0].parent() < paths[1].parent(), "names sort by start");
}

#[test]
fn never_connected_stops_cleanly_and_checks() {
    let fixture = Fixture::new(false);
    let closed = listener();
    let port = closed.local_addr().expect("port").port();
    drop(closed);
    let mut paper = fixture.start(port, 0);
    paper.wait_connect_failure();
    let path = paper.stop("TERM");
    let (rows, records) = complete_session(&path);
    assert_eq!(count(&rows, "action", "place"), 0);
    assert_eq!(rows.last().expect("end")["pnl"]["position"], "0");
    assert_diagnostics(&rows, 1);
    assert!(records.iter().all(|record| {
        record.kind != RecordKind::Meta
            || serde_json::from_slice::<Value>(&record.payload).expect("metadata")["event"]
                != "connected"
    }));
}
