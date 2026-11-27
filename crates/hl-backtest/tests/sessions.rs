//! Independent session examples exercise the recording, driver and CLI together.

#[path = "sessions/checkpoints.rs"]
mod checkpoints;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use hl_backtest::recording::{Recording, read_recordings};
use hl_backtest::session::{BuildIdentity, ReplayComponents, check_session};
use hl_codec::hyperliquid::Network;
use hl_codec::rawlog::{Record, RecordKind, write_record};
use hl_run::replay_with_session;
use hl_run::session::{ManifestEntry, Session, SessionEnd, SessionStart};
use hl_sim::{SimConfig, SimGateway};
use hl_strategy::{Actions, Context, Strategy, StrategyHost};
use hl_types::{
    ClientOrderId, Event, InstrumentId, MarketEventKind, OrderEventKind, OrderSpec, Px, Side,
    TimeInForce, TimerId, Timestamp,
};
use serde_json::{Value, json};

const INSTRUMENT: InstrumentId = InstrumentId::new(0x619a_6e2d);
const COMMIT: &str = "1111111111111111111111111111111111111111";
static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    directory: PathBuf,
    files: Vec<PathBuf>,
    hashes: Vec<String>,
}

impl Fixture {
    fn new() -> Self {
        let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "hl-backtest-sessions-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&directory).expect("create synthetic session directory");
        let first = vec![
            header(0),
            metadata(0, "connected"),
            bbo(1, "98", "102"),
            record(
                RecordKind::RecvText,
                4,
                json!({"channel":"trades","data":[{
                    "coin":"BTC","side":"A","px":"98","sz":"1","time":4,"tid":41,
                    "hash":"0x0000000000000000000000000000000000000000000000000000000000000000",
                    "users":["0x1111111111111111111111111111111111111111","0x2222222222222222222222222222222222222222"]
                }]}),
            ),
            bbo(5, "97", "101"),
            bbo(8, "98", "102"),
            bbo(11, "98", "102"),
            metadata(13, "disconnected"),
            metadata(14, "dropped"),
            metadata(15, "shutdown"),
        ];
        let second = vec![
            header(20),
            metadata(20, "connected"),
            bbo(21, "98", "102"),
            bbo(23, "90", "92"),
        ];
        let files = vec![
            write_recording(&directory, "first.hlrec.zst", &first, false),
            write_recording(&directory, "second.hlrec.zst", &second, true),
        ];
        // Python's hashlib supplies independent manifest hashes. All session
        // decisions and account values are literal expected-session.jsonl lines.
        let hashes = files.iter().map(|path| independent_hash(path)).collect();
        Self {
            directory,
            files,
            hashes,
        }
    }

    fn expected(&self) -> String {
        include_str!("expected-session.jsonl")
            .replace("FIRST_SHA256", &self.hashes[0])
            .replace("SECOND_SHA256", &self.hashes[1])
    }

    fn session(&self, name: &str, text: &str) -> PathBuf {
        let path = self.directory.join(name);
        fs::write(&path, text).expect("write synthetic session");
        path
    }

    fn expected_path(&self) -> PathBuf {
        self.session("expected.jsonl", &self.expected())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.directory).expect("remove synthetic session directory");
    }
}

fn independent_hash(path: &Path) -> String {
    let output = Command::new("python3")
        .args(["-I", "-c", "import hashlib, pathlib, sys; print(hashlib.sha256(pathlib.Path(sys.argv[1]).read_bytes()).hexdigest())"])
        .arg(path)
        .output()
        .expect("run independent SHA-256 utility");
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .expect("SHA-256 output is UTF-8")
        .split_whitespace()
        .next()
        .expect("SHA-256 output contains hash")
        .to_owned()
}

fn write_recording(directory: &Path, name: &str, records: &[Record], truncated: bool) -> PathBuf {
    let mut bytes = Vec::new();
    for record in records {
        write_record(
            &mut bytes,
            record.kind,
            record.recv_wall_ns,
            record.recv_mono_ns,
            &record.payload,
        )
        .expect("encode synthetic raw record");
    }
    if truncated {
        bytes.extend_from_slice(&[20, 0, 0]);
    }
    let compressed =
        zstd::stream::encode_all(bytes.as_slice(), 1).expect("compress synthetic data");
    let path = directory.join(name);
    fs::write(&path, compressed).expect("write synthetic raw file");
    path
}

fn record(kind: RecordKind, milliseconds: i64, payload: Value) -> Record {
    Record {
        kind,
        recv_wall_ns: milliseconds * 1_000_000,
        recv_mono_ns: 0,
        payload: serde_json::to_vec(&payload).expect("encode synthetic payload"),
    }
}

fn header(milliseconds: i64) -> Record {
    record(
        RecordKind::Meta,
        milliseconds,
        json!({
            "event":"file_open","format":1,"source":"hl",
            "url":"wss://api.hyperliquid.xyz/ws","file_seq":0,
            "process_start_wall_ns":milliseconds * 1_000_000
        }),
    )
}

fn metadata(milliseconds: i64, event: &str) -> Record {
    record(
        RecordKind::Meta,
        milliseconds,
        json!({"event":event,"count":1}),
    )
}

fn bbo(milliseconds: i64, bid: &str, ask: &str) -> Record {
    record(
        RecordKind::RecvText,
        milliseconds,
        json!({"channel":"bbo","data":{
            "coin":"BTC","time":milliseconds,
            "bbo":[{"px":bid,"sz":"10","n":1},{"px":ask,"sz":"10","n":1}]
        }}),
    )
}

#[derive(Debug)]
struct FixtureStrategy {
    bid_px: Px,
    quotes: usize,
    gaps: usize,
    bid: Option<ClientOrderId>,
    ask: Option<ClientOrderId>,
}

impl FixtureStrategy {
    fn new(bid_px: Px) -> Self {
        Self {
            bid_px,
            quotes: 0,
            gaps: 0,
            bid: None,
            ask: None,
        }
    }
}

fn order(side: Side, px: Px, quantity: &str) -> OrderSpec {
    OrderSpec {
        instrument: INSTRUMENT,
        side,
        px,
        qty: quantity.parse().expect("literal quantity"),
        tif: TimeInForce::Alo,
        reduce_only: false,
    }
}

impl Strategy for FixtureStrategy {
    fn on_event(&mut self, _ctx: &Context<'_>, event: &Event, actions: &mut Actions) {
        match event {
            Event::Market(market) if matches!(market.kind, MarketEventKind::Bbo(_)) => {
                self.quotes += 1;
                match self.quotes {
                    1 => {
                        self.bid = Some(actions.place(order(Side::Buy, self.bid_px, "2")));
                        self.ask = Some(actions.place(order(
                            Side::Sell,
                            "101".parse().expect("price"),
                            "2",
                        )));
                        actions.set_timer(Timestamp::from_millis(6), TimerId(7));
                        actions.set_timer(Timestamp::from_millis(50), TimerId(8));
                    }
                    2 => {
                        actions.modify(
                            INSTRUMENT,
                            self.bid.expect("initial bid"),
                            "98".parse().expect("price"),
                            "1".parse().expect("quantity"),
                        );
                        actions.place(order(Side::Buy, "102".parse().expect("price"), "1"));
                    }
                    5 => {
                        actions.place(order(Side::Buy, self.bid_px, "2"));
                    }
                    _ => {}
                }
            }
            Event::Market(market) if matches!(market.kind, MarketEventKind::FeedGap) => {
                self.gaps += 1;
                match self.gaps {
                    1 => actions.cancel_all(Some(INSTRUMENT)),
                    5 => actions.cancel_all(None),
                    _ => {}
                }
            }
            Event::Timer(timer) if timer.id == TimerId(7) => {
                actions.cancel(INSTRUMENT, self.ask.expect("initial ask"));
            }
            Event::Order(report)
                if matches!(report.kind, OrderEventKind::Canceled { .. })
                    && Some(report.cloid) == self.ask =>
            {
                actions.cancel(INSTRUMENT, report.cloid);
                actions.modify(
                    INSTRUMENT,
                    report.cloid,
                    "101".parse().expect("price"),
                    "2".parse().expect("quantity"),
                );
            }
            _ => {}
        }
    }
}

fn build() -> BuildIdentity {
    BuildIdentity {
        commit: COMMIT.to_owned(),
        dirty: false,
    }
}

fn sim_config() -> SimConfig {
    SimConfig {
        order_latency: Duration::from_millis(1),
        report_latency: Duration::from_millis(2),
        maker_fee: "0.0001".parse().expect("literal fee"),
        min_order_notional: "10".parse().expect("literal minimum"),
    }
}

fn start() -> SessionStart {
    SessionStart {
        commit: COMMIT.to_owned(),
        dirty: false,
        driver: "backtest".to_owned(),
        network: Network::Mainnet,
        source: None,
        coin: "BTC".to_owned(),
        instrument: INSTRUMENT,
        process_start_wall_ns: None,
        strategy_name: "session_fixture".to_owned(),
        strategy_tag: 7,
        strategy: vec![("bid_px".to_owned(), "99".to_owned())],
        sim_model: "strict_trade_through".to_owned(),
        sim: sim_config().parameters(),
    }
}

fn components(
    start: &SessionStart,
    _recording: &Recording,
) -> Result<ReplayComponents<FixtureStrategy>, String> {
    if start.strategy_name != "session_fixture"
        || start.strategy.len() != 1
        || start.strategy[0].0 != "bid_px"
    {
        return Err("invalid fixture strategy parameters".to_owned());
    }
    let price = start.strategy[0]
        .1
        .parse()
        .map_err(|error| format!("bid_px: {error}"))?;
    let config = SimConfig::from_parameters(&start.sim)?;
    Ok(ReplayComponents {
        host: StrategyHost::new(FixtureStrategy::new(price), start.strategy_tag),
        sim: SimGateway::new(config),
        strategy_parameters: vec![("bid_px".to_owned(), price.to_string())],
        sim_parameters: config.parameters(),
    })
}

fn check(path: &Path) -> Result<(), String> {
    check_session(path, &build(), components)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[test]
fn driver_writes_the_independent_expected_session_byte_for_byte() {
    let fixture = Fixture::new();
    let recording = read_recordings(&fixture.files, Network::Mainnet, "BTC").expect("read fixture");
    let start = start();
    let ReplayComponents {
        mut host, mut sim, ..
    } = components(&start, &recording).expect("build fixture components");
    let mut session = Session::new(&start);
    let stop = recording.stop_time.expect("complete fixture record");
    let trace = replay_with_session(
        &mut host,
        &mut sim,
        &recording.events,
        &recording.origins,
        stop,
        &mut session,
    )
    .expect("replay fixture");
    let manifest = ["first.hlrec.zst", "second.hlrec.zst"]
        .into_iter()
        .enumerate()
        .map(|(index, name)| ManifestEntry {
            path: name.to_owned(),
            file_seq: Some(0),
            sha256: fixture.hashes[index].clone(),
        })
        .collect();
    session.finish(&SessionEnd {
        stop,
        last_raw: recording.last_position,
        manifest,
        pnl: hl_run::pnl::summarize(INSTRUMENT, &recording.events, &trace.order_events)
            .expect("value fixture"),
        open_orders: host.orders().len(),
        pending: trace.pending_at_end,
        late_print_misses: trace.late_print_misses,
    });
    // One buy of 1 at 99 costs 99 plus a 0.0099 fee. The final midpoint
    // (90+92)/2 is 91, so net=-8.0099 and the final mark sets the minimum.
    assert_eq!(session.lines().concat(), fixture.expected());
}

#[test]
fn checker_recomputes_the_hand_written_session() {
    let fixture = Fixture::new();
    check(&fixture.expected_path()).expect("independent expected session matches");
}

fn rows(text: &str) -> Vec<Value> {
    text.lines()
        .map(|line| serde_json::from_str(line).expect("fixture row"))
        .collect()
}

// Preserve the independent fixture's field order when changing its values.
// Re-serializing a Value would sort object keys and hide the intended regression
// behind an earlier canonical-encoding failure.
fn encoded(rows: &[Value], template: &str) -> String {
    fn object_order(value: &Value, template: &str) -> String {
        match value {
            Value::Object(object) => {
                let mut fields: Vec<_> = object.iter().collect();
                fields.sort_by_key(|(key, _)| {
                    template.find(&format!("\"{key}\":")).unwrap_or(usize::MAX)
                });
                let fields: Vec<_> = fields
                    .into_iter()
                    .map(|(key, value)| format!("{}:{}", json!(key), object_order(value, template)))
                    .collect();
                format!("{{{}}}", fields.join(","))
            }
            Value::Array(values) => format!(
                "[{}]",
                values
                    .iter()
                    .map(|value| object_order(value, template))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            _ => value.to_string(),
        }
    }
    let templates: Vec<_> = template
        .lines()
        .map(|line| {
            (
                serde_json::from_str::<Value>(line).expect("template row"),
                line,
            )
        })
        .collect();
    rows.iter()
        .map(|row| {
            let order = templates
                .iter()
                .find(|(original, _)| {
                    original["kind"] == row["kind"] && original.get("variant") == row.get("variant")
                })
                .map_or("", |(_, line)| *line);
            format!("{}\n", object_order(row, order))
        })
        .collect()
}

fn edited(fixture: &Fixture, edit: impl FnOnce(&mut Vec<Value>)) -> String {
    let mut rows = rows(&fixture.expected());
    edit(&mut rows);
    check(&fixture.session("edited.jsonl", &encoded(&rows, &fixture.expected())))
        .expect_err("changed session must fail")
}

fn assert_difference(error: &str, location: &str, field: &str) {
    assert!(error.contains(location), "expected {location}, got {error}");
    assert!(error.contains(field), "expected {field}, got {error}");
    assert!(
        error.contains("expected"),
        "missing expected value: {error}"
    );
    assert!(
        error.contains("replayed"),
        "missing replayed value: {error}"
    );
}

#[test]
fn every_required_field_edit_fails_at_its_first_difference() {
    let fixture = Fixture::new();
    for (row, field, replacement, location) in [
        (1, "px", "99.00000001", "seq 0"),
        (5, "oid", "2", "seq 4"),
        (9, "fee", "0.00990001", "seq 8"),
        (19, "cause", "dropped", "seq 18"),
        (3, "deadline_ns", "6000001", "seq 2"),
    ] {
        let error = edited(&fixture, |rows| rows[row][field] = json!(replacement));
        assert_difference(&error, location, field);
    }
    let error = edited(&fixture, |rows| {
        rows[0]["strategy"]["bid_px"] = json!("100")
    });
    assert_difference(&error, "seq 0", "px");
    let error = edited(&fixture, |rows| {
        rows[0]["sim"]["report_latency_ns"] = json!("2000001")
    });
    assert_difference(&error, "seq 4", "ts_local_ns");
    let error = edited(&fixture, |rows| {
        rows.last_mut().expect("end")["pnl"]["net"] = json!("-8.00990001")
    });
    assert_difference(&error, "end", "pnl.net");
}

#[test]
fn deleted_added_and_swapped_rows_fail_even_with_consecutive_sequences() {
    let fixture = Fixture::new();
    let error = edited(&fixture, |rows| {
        rows.remove(7);
        renumber(rows);
    });
    assert_difference(&error, "seq 6", "variant");
    let error = edited(&fixture, |rows| {
        rows.insert(7, rows[6].clone());
        renumber(rows);
    });
    assert_difference(&error, "seq 6", "kind");
    let error = edited(&fixture, |rows| {
        rows.swap(7, 8);
        renumber(rows);
    });
    assert_difference(&error, "seq 6", "variant");
}

#[test]
fn missing_and_extra_final_decisions_are_compared_before_end() {
    let fixture = Fixture::new();
    let error = edited(&fixture, |rows| {
        rows.remove(rows.len() - 2);
    });
    assert_difference(&error, "seq 26", "kind");
    let error = edited(&fixture, |rows| {
        let mut extra = rows[rows.len() - 2].clone();
        extra["seq"] = json!("27");
        rows.insert(rows.len() - 1, extra);
    });
    assert_difference(&error, "seq 27", "kind");
}

fn renumber(rows: &mut [Value]) {
    for (sequence, row) in rows
        .iter_mut()
        .filter(|row| row.get("seq").is_some())
        .enumerate()
    {
        row["seq"] = json!(sequence.to_string());
    }
}

#[test]
fn incomplete_unknown_and_nonconsecutive_sessions_are_rejected() {
    let fixture = Fixture::new();
    let error = edited(&fixture, |rows| {
        rows.pop();
    });
    assert!(error.contains("end"), "{error}");
    let error = edited(&fixture, |rows| rows[2]["seq"] = json!("9"));
    assert!(error.contains("seq") && error.contains('1'), "{error}");
    let error = edited(&fixture, |rows| rows[3]["kind"] = json!("mystery"));
    assert!(
        error.contains("kind") && error.contains("mystery"),
        "{error}"
    );
    let error = edited(&fixture, |rows| {
        rows.remove(0);
    });
    assert!(error.contains("start"), "{error}");
}

#[test]
fn duplicate_fields_are_rejected_including_escaped_parameter_names() {
    let fixture = Fixture::new();
    for (before, after) in [
        (r#""dirty":false"#, r#""dirty":false,"dirty":false"#),
        (r#""bid_px":"99""#, r#""bid_px":"99","bid_\u0070x":"99""#),
        (
            r#""maker_fee_bps":"1""#,
            r#""maker_fee_bps":"1","maker_fee_bps":"1""#,
        ),
    ] {
        let text = fixture.expected().replacen(before, after, 1);
        let error = check(&fixture.session("duplicate.jsonl", &text))
            .expect_err("duplicate fields are ambiguous");
        assert!(error.to_ascii_lowercase().contains("duplicate"), "{error}");
    }
}

#[test]
fn manifest_metadata_and_raw_extent_are_recomputed() {
    let fixture = Fixture::new();
    let error = edited(&fixture, |rows| {
        rows.last_mut().expect("end")["manifest"][0]["file_seq"] = json!("1");
    });
    assert_difference(&error, "end", "file_seq");
    let error = edited(&fixture, |rows| {
        rows.last_mut().expect("end")["last_raw"]["record_index"] = json!("2");
    });
    assert_difference(&error, "end", "record_index");
    let error = edited(&fixture, |rows| {
        rows.last_mut().expect("end")["stop_ns"] = json!("23000001");
    });
    assert_difference(&error, "end", "stop_ns");
}

#[test]
fn extra_and_missing_fields_are_compared_exactly() {
    let fixture = Fixture::new();
    let error = edited(&fixture, |rows| rows[1]["extra"] = json!(false));
    assert_difference(&error, "seq 0", "extra");
    let error = edited(&fixture, |rows| {
        rows[1]
            .as_object_mut()
            .expect("object")
            .remove("reduce_only");
    });
    assert_difference(&error, "seq 0", "reduce_only");
}

#[test]
fn diagnostic_rows_are_skipped_without_using_a_decision_sequence() {
    let fixture = Fixture::new();
    let mut rows = rows(&fixture.expected());
    rows.insert(
        4,
        json!({"kind":"diagnostic","lag_ns":"123","observation":{"elapsed_ns":"456"}}),
    );
    check(&fixture.session("diagnostic.jsonl", &encoded(&rows, &fixture.expected())))
        .expect("observations are excluded");
}

#[test]
fn changed_or_missing_raw_file_fails_before_strategy_construction() {
    for missing in [false, true] {
        let fixture = Fixture::new();
        let session = fixture.expected_path();
        if missing {
            fs::remove_file(&fixture.files[0]).expect("remove synthetic file");
        } else {
            fs::write(&fixture.files[0], b"changed synthetic file").expect("change synthetic file");
        }
        let error = check_session::<FixtureStrategy>(&session, &build(), |_, _| {
            panic!("hashes must be verified before replay")
        })
        .expect_err("changed raw input fails")
        .to_string();
        assert!(error.contains("first.hlrec.zst"), "{error}");
        if !missing {
            assert!(
                error.to_ascii_lowercase().contains("sha256")
                    || error.to_ascii_lowercase().contains("sha-256"),
                "{error}"
            );
        }
    }
}

#[test]
fn different_commit_or_dirty_flag_is_refused_before_replay() {
    let fixture = Fixture::new();
    for identity in [
        BuildIdentity {
            commit: "2222222222222222222222222222222222222222".to_owned(),
            dirty: false,
        },
        BuildIdentity {
            commit: COMMIT.to_owned(),
            dirty: true,
        },
    ] {
        let error =
            check_session::<FixtureStrategy>(&fixture.expected_path(), &identity, |_, _| {
                panic!("build gate must precede replay")
            })
            .expect_err("build mismatch fails")
            .to_string();
        assert!(error.contains(COMMIT), "{error}");
        assert!(error.contains(&identity.commit), "{error}");
        assert!(
            error.contains("dirty") && error.contains("false"),
            "{error}"
        );
    }
}

fn cli_command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hl-backtest"));
    command.args([
        "run",
        "--coin",
        "BTC",
        "--half-spread",
        "1",
        "--qty",
        "1",
        "--order-latency-ms",
        "1",
        "--report-latency-ms",
        "2",
        "--maker-fee-bps",
        "1",
    ]);
    command
}

fn cli_run(fixture: &Fixture, session: Option<&Path>) -> Output {
    let mut command = cli_command();
    if let Some(path) = session {
        command.arg("--session-out").arg(path);
    }
    command
        .args(&fixture.files)
        .output()
        .expect("run hl-backtest")
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn cli_session_out_resolves_raw_paths_from_the_command_directory() {
    let fixture = Fixture::new();
    let results = fixture.directory.join("results");
    let elsewhere = fixture.directory.join("elsewhere");
    fs::create_dir(&results).expect("create session directory");
    fs::create_dir(&elsewhere).expect("create different working directory");
    for (name, process_ms) in [
        ("m-absolute.hlrec.zst", 0),
        ("z-dot.hlrec.zst", 20),
        ("a-plain.hlrec.zst", 40),
    ] {
        let mut opening = header(process_ms);
        opening.recv_wall_ns = 0;
        write_recording(
            &fixture.directory,
            name,
            &[
                opening,
                metadata(process_ms, "connected"),
                bbo(process_ms + 1, "98", "102"),
                metadata(process_ms + 2, "shutdown"),
            ],
            false,
        );
    }
    // All headers have wall time zero. Their original path components put
    // the absolute path first, then ./z-dot, then a-plain, independently of
    // argument order and the basenames' opposite alphabetical order.
    let paths = [
        PathBuf::from("a-plain.hlrec.zst"),
        PathBuf::from("./z-dot.hlrec.zst"),
        fixture.directory.join("m-absolute.hlrec.zst"),
    ];
    let plain = cli_command()
        .current_dir(&fixture.directory)
        .args(&paths)
        .output()
        .expect("replay tied headers with original input spellings");
    assert_success(&plain);
    let output = cli_command()
        .current_dir(&fixture.directory)
        .args(["--session-out", "results/session.jsonl"])
        .args(&paths)
        .output()
        .expect("write session with relative input paths");
    assert_success(&output);
    assert_eq!(output.stdout, plain.stdout);
    let session_path = results.join("session.jsonl");
    let written = rows(&fs::read_to_string(&session_path).expect("read sealed session"));
    let manifest_names: Vec<_> = written.last().expect("end row")["manifest"]
        .as_array()
        .expect("manifest array")
        .iter()
        .map(|entry| {
            Path::new(entry["path"].as_str().expect("manifest path"))
                .file_name()
                .expect("raw filename")
                .to_str()
                .expect("UTF-8 fixture name")
        })
        .collect();
    assert_eq!(
        manifest_names,
        [
            "m-absolute.hlrec.zst",
            "z-dot.hlrec.zst",
            "a-plain.hlrec.zst"
        ]
    );
    let checked = Command::new(env!("CARGO_BIN_EXE_hl-backtest"))
        .current_dir(&elsewhere)
        .args(["run", "--check-session"])
        .arg(session_path)
        .output()
        .expect("check session from another working directory");
    assert_success(&checked);
}

#[test]
fn cli_session_out_preserves_an_existing_partial_file() {
    let fixture = Fixture::new();
    let path = fixture.directory.join("existing.jsonl");
    let partial = fixture.directory.join("existing.jsonl.part");
    let original = b"an earlier incomplete session\n";
    fs::write(&partial, original).expect("write existing partial session");
    let output = cli_run(&fixture, Some(&path));
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        fs::read(&partial).expect("read preserved partial"),
        original
    );
    assert!(!path.exists());
}

#[test]
fn cli_session_preserves_stdout_is_repeatable_and_checks() {
    let fixture = Fixture::new();
    let plain = cli_run(&fixture, None);
    let first_path = fixture.directory.join("first.jsonl");
    let second_path = fixture.directory.join("second.jsonl");
    let first = cli_run(&fixture, Some(&first_path));
    let second = cli_run(&fixture, Some(&second_path));
    for output in [&plain, &first, &second] {
        assert_success(output);
    }
    assert_eq!(plain.stdout, first.stdout);
    assert_eq!(first.stdout, second.stdout);
    assert_eq!(
        fs::read(&first_path).expect("first session"),
        fs::read(&second_path).expect("second session")
    );
    assert!(!fixture.directory.join("first.jsonl.part").exists());
    let checked = Command::new(env!("CARGO_BIN_EXE_hl-backtest"))
        .args(["run", "--check-session"])
        .arg(&first_path)
        .output()
        .expect("check CLI session");
    assert_success(&checked);
    assert_eq!(String::from_utf8_lossy(&checked.stdout).lines().count(), 1);
    let session_rows = rows(&fs::read_to_string(&first_path).expect("session text"));
    if session_rows[0]["dirty"] == true {
        let stderr = String::from_utf8_lossy(&checked.stderr);
        assert!(
            stderr.contains("dirty") && stderr.contains("warning"),
            "{stderr}"
        );
    }
    let original = fs::read(&first_path).expect("sealed session");
    let refused = cli_run(&fixture, Some(&first_path));
    assert!(!refused.status.success());
    assert_eq!(fs::read(&first_path).expect("original remains"), original);
}

#[test]
fn cli_check_takes_no_other_arguments_and_differences_exit_one() {
    let fixture = Fixture::new();
    let path = fixture.directory.join("cli.jsonl");
    assert_success(&cli_run(&fixture, Some(&path)));
    for arguments in [
        vec!["--coin", "BTC"],
        vec!["extra.hlrec.zst"],
        vec!["--session-out", "other.jsonl"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_hl-backtest"))
            .args(["run", "--check-session"])
            .arg(&path)
            .args(arguments)
            .output()
            .expect("reject mixed check arguments");
        assert!(!output.status.success());
    }
    let original = fs::read_to_string(&path).expect("read session");
    let mut rows = rows(&original);
    rows[1]["px"] = json!("123");
    fs::write(&path, encoded(&rows, &original)).expect("edit action");
    let output = Command::new(env!("CARGO_BIN_EXE_hl-backtest"))
        .args(["run", "--check-session"])
        .arg(&path)
        .output()
        .expect("check changed session");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_difference(&stderr, "seq 0", "px");
}

#[test]
fn component_factory_receives_parameters_in_their_recorded_order() {
    let fixture = Fixture::new();
    check_session(&fixture.expected_path(), &build(), |start, recording| {
        assert_eq!(
            start.sim,
            [
                ("order_latency_ns".to_owned(), "1000000".to_owned()),
                ("report_latency_ns".to_owned(), "2000000".to_owned()),
                ("maker_fee_bps".to_owned(), "1".to_owned()),
                ("min_order_notional_usd".to_owned(), "10".to_owned()),
            ]
        );
        components(start, recording)
    })
    .expect("factory retains the component's declared parameter order");
}

#[test]
fn an_action_difference_precedes_a_changed_footer_stop() {
    let fixture = Fixture::new();
    let error = edited(&fixture, |rows| {
        rows[1]["px"] = json!("99.00000001");
        rows.last_mut().expect("end")["stop_ns"] = json!("23000001");
    });
    assert_difference(&error, "seq 0", "px");
}

#[test]
fn value_equal_rows_with_changed_bytes_fail_at_the_changed_row() {
    let fixture = Fixture::new();
    for (before, after, location) in [
        (
            r#""kind":"action","seq":"0""#,
            r#""seq":"0","kind":"action""#,
            "seq 0",
        ),
        (
            r#""kind":"action","seq":"0""#,
            r#""kind": "action","seq":"0""#,
            "seq 0",
        ),
        (
            r#""kind":"start","schema_version":1"#,
            r#""schema_version":1,"kind":"start""#,
            "start",
        ),
        (
            r#""kind":"end","stop_ns""#,
            r#""kind":"end", "stop_ns""#,
            "end",
        ),
    ] {
        let text = fixture.expected().replacen(before, after, 1);
        assert_ne!(text, fixture.expected(), "edit must change the fixture");
        let error = check(&fixture.session("encoding.jsonl", &text))
            .expect_err("changed canonical bytes fail");
        assert!(
            error.contains(location) && error.contains("encoding differs"),
            "{error}"
        );
    }
    for text in [
        fixture.expected().trim_end_matches('\n').to_owned(),
        fixture.expected().replace('\n', "\r\n"),
    ] {
        let error = check(&fixture.session("line-ending.jsonl", &text))
            .expect_err("line terminators are canonical bytes");
        assert!(error.contains("encoding differs"), "{error}");
    }
}

fn paper_fixture(fixture: &Fixture) -> (String, Vec<Vec<Record>>) {
    let mut next_header = header(10);
    let mut meta: Value = serde_json::from_slice(&next_header.payload).expect("header JSON");
    meta["file_seq"] = json!(1);
    meta["process_start_wall_ns"] = json!(0);
    next_header.payload = serde_json::to_vec(&meta).expect("header JSON");
    let records = vec![
        vec![header(0), metadata(0, "connected"), bbo(1, "98", "102")],
        vec![next_header, bbo(11, "98", "102"), metadata(12, "shutdown")],
    ];
    let paths: Vec<_> = records
        .iter()
        .enumerate()
        .map(|(index, records)| {
            write_recording(
                &fixture.directory,
                &format!("paper-{index}.hlrec.zst"),
                records,
                false,
            )
        })
        .collect();
    let recording = read_recordings(&paths, Network::Mainnet, "BTC").expect("paper raw fixture");
    let mut start = start();
    start.driver = "paper".to_owned();
    start.source = Some("hl".to_owned());
    start.process_start_wall_ns = Some(0);
    let ReplayComponents {
        mut host, mut sim, ..
    } = components(&start, &recording).expect("paper components");
    let manifest = paths
        .iter()
        .enumerate()
        .map(|(index, path)| ManifestEntry {
            path: path
                .file_name()
                .expect("filename")
                .to_str()
                .expect("UTF-8 filename")
                .to_owned(),
            file_seq: Some(index as u64),
            sha256: independent_hash(path),
        })
        .collect();
    let (session, _, _) =
        hl_backtest::replay_session(&recording, &start, manifest, &mut host, &mut sim)
            .expect("paper reference");
    (session.lines().concat(), records)
}

fn rewrite_paper_raw(fixture: &Fixture, text: &str, records: &[Vec<Record>]) -> String {
    let mut text = text.to_owned();
    for (index, records) in records.iter().enumerate() {
        let name = format!("paper-{index}.hlrec.zst");
        let previous_hash = independent_hash(&fixture.directory.join(&name));
        let path = write_recording(&fixture.directory, &name, records, false);
        text = text.replace(&previous_hash, &independent_hash(&path));
    }
    text
}

fn edit_header(record: &mut Record, field: &str, value: Value) {
    let mut header: Value = serde_json::from_slice(&record.payload).expect("header JSON");
    header[field] = value;
    record.payload = serde_json::to_vec(&header).expect("header JSON");
}

#[test]
fn paper_chain_and_shutdown_are_checked_before_strategy_construction() {
    for (case, expected) in [
        ("skip", "file_seq"),
        ("start_at_one", "file_seq"),
        ("anchor", "process_start_wall_ns"),
        ("source", "source"),
        ("shutdown", "final raw record must be shutdown"),
        (
            "control_after_shutdown",
            "final raw record must be shutdown",
        ),
        ("truncated", "truncated"),
        ("stop", "stop_ns"),
        ("manifest_sequence", "file_seq"),
        ("manifest_order", "file_seq"),
    ] {
        let fixture = Fixture::new();
        let (text, mut records) = paper_fixture(&fixture);
        let mut text = match case {
            "skip" => {
                edit_header(&mut records[1][0], "file_seq", json!(2));
                text
            }
            "start_at_one" => {
                edit_header(&mut records[0][0], "file_seq", json!(1));
                text
            }
            "anchor" => {
                edit_header(&mut records[1][0], "process_start_wall_ns", json!(99));
                text
            }
            "source" => {
                edit_header(&mut records[1][0], "source", json!("another"));
                text
            }
            "shutdown" => {
                records[1][2] = metadata(12, "disconnected");
                text
            }
            "control_after_shutdown" => {
                records[1].push(record(RecordKind::SentText, 12, json!({"method":"ping"})));
                text
            }
            "stop" => text.replace(r#""stop_ns":"12000000""#, r#""stop_ns":"12000001""#),
            "manifest_sequence" => text.replace(r#""file_seq":"0""#, r#""file_seq":"1""#),
            "manifest_order" => {
                let mut values = rows(&text);
                values.last_mut().expect("end")["manifest"]
                    .as_array_mut()
                    .expect("manifest")
                    .swap(0, 1);
                encoded(&values, &text)
            }
            "truncated" => text,
            _ => unreachable!(),
        };
        text = rewrite_paper_raw(&fixture, &text, &records);
        if case == "truncated" {
            let path = fixture.directory.join("paper-1.hlrec.zst");
            let old_hash = independent_hash(&path);
            write_recording(&fixture.directory, "paper-1.hlrec.zst", &records[1], true);
            text = text.replace(&old_hash, &independent_hash(&path));
        }
        let path = fixture.session("invalid-paper.jsonl", &text);
        let error = check_session::<FixtureStrategy>(&path, &build(), |_, _| {
            panic!("paper raw extent must be checked before strategy construction")
        })
        .expect_err("invalid paper chain fails")
        .to_string();
        assert!(error.contains(expected), "{case}: {error}");
    }
}

#[test]
fn paper_check_uses_declared_network_for_loopback_headers() {
    let fixture = Fixture::new();
    let (text, records) = paper_fixture(&fixture);
    check(&fixture.session("paper.jsonl", &text)).expect("complete paper chain");
    // URLs are fixture text only. A paper run uses its declared network.
    for url in [
        "ws://127.0.0.1:1234/ws",
        "ws://[::1]:1234/ws",
        "wss://api.hyperliquid-testnet.xyz/ws",
    ] {
        let mut records = records.clone();
        for records in &mut records {
            edit_header(&mut records[0], "url", json!(url));
        }
        // Reconstruct the original files before each independent hash rewrite.
        let (original, _) = paper_fixture(&fixture);
        let local = rewrite_paper_raw(&fixture, &original, &records);
        check(&fixture.session("paper-loopback.jsonl", &local))
            .expect("loopback source uses the paper network");
    }
}

#[test]
fn checker_rejects_unknown_driver_and_noncanonical_component_encoding() {
    let fixture = Fixture::new();
    let text = fixture
        .expected()
        .replace(r#""driver":"backtest""#, r#""driver":"other""#);
    let error = check(&fixture.session("unknown-driver.jsonl", &text)).expect_err("unknown driver");
    assert!(error.contains("start field driver"), "{error}");
    let text = fixture.expected().replace(
        r#""order_latency_ns":"1000000","report_latency_ns":"2000000""#,
        r#""report_latency_ns":"2000000","order_latency_ns":"1000000""#,
    );
    let error = check(&fixture.session("parameter-order.jsonl", &text))
        .expect_err("reconstructed component returns canonical order");
    assert!(error.contains("start encoding differs"), "{error}");
}
