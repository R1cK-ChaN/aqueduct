//! Synthetic recordings exercise the reader, replay driver and command together.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use hl_backtest::recording::{Recording, read_recordings};
use hl_codec::hyperliquid::{Network, instrument_id};
use hl_codec::rawlog::{Record, RecordKind, write_record};
use hl_run::recording::RecordConverter;
use hl_run::{ReplayTrace, replay};
use hl_sim::{SimConfig, SimGateway};
use hl_strategy::{Actions, Context, FixedSpreadConfig, FixedSpreadQuoter, Strategy, StrategyHost};
use hl_types::{Event, MarketEvent, MarketEventKind, OrderEventKind, Timestamp};
use serde_json::{Value, json};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct TestFiles {
    directory: PathBuf,
}

impl TestFiles {
    fn new() -> Self {
        let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "hl-backtest-recordings-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&directory).expect("create synthetic recording directory");
        Self { directory }
    }

    fn recording(&self, name: &str, records: &[Record], ending: Ending) -> PathBuf {
        let path = self.directory.join(name);
        let mut bytes = encode(records);
        match ending {
            Ending::Complete | Ending::ZstdCut => {}
            Ending::RecordHeaderCut => bytes.extend_from_slice(&[20, 0, 0]),
            Ending::RecordPayloadCut => {
                let mut partial = encode(&[meta(9_000, "disconnected")]);
                partial.pop();
                bytes.extend(partial);
            }
        }
        let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), 1)
            .expect("create synthetic zstd encoder");
        encoder.write_all(&bytes).expect("compress records");
        encoder.flush().expect("flush complete records");
        let flushed_len = encoder.get_ref().len();
        let mut compressed = encoder.finish().expect("finish zstd frame");
        if matches!(ending, Ending::ZstdCut) {
            compressed.truncate(flushed_len);
        }
        fs::write(&path, compressed).expect("write synthetic recording");
        path
    }
}

impl Drop for TestFiles {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.directory).expect("remove synthetic recording directory");
    }
}

#[derive(Clone, Copy, Debug)]
enum Ending {
    Complete,
    RecordHeaderCut,
    RecordPayloadCut,
    ZstdCut,
}

fn encode(records: &[Record]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for record in records {
        write_record(
            &mut bytes,
            record.kind,
            record.recv_wall_ns,
            record.recv_mono_ns,
            &record.payload,
        )
        .expect("encode synthetic record");
    }
    bytes
}

fn record(kind: RecordKind, received_ms: i64, payload: Value) -> Record {
    Record {
        kind,
        recv_wall_ns: received_ms * 1_000_000,
        recv_mono_ns: 0,
        payload: serde_json::to_vec(&payload).expect("encode JSON payload"),
    }
}

fn with_mono(mut record: Record, recv_mono_ns: u64) -> Record {
    record.recv_mono_ns = recv_mono_ns;
    record
}

fn header(open_ms: i64, process: i64, file_seq: Option<u64>) -> Record {
    let mut payload = json!({
        "event": "file_open",
        "format": 1,
        "source": "hl",
        "url": "wss://api.hyperliquid.xyz/ws",
        "process_start_wall_ns": process,
    });
    if let Some(sequence) = file_seq {
        payload["file_seq"] = sequence.into();
    }
    record(RecordKind::Meta, open_ms, payload)
}

fn meta(received_ms: i64, event: &str) -> Record {
    record(RecordKind::Meta, received_ms, json!({"event": event}))
}

fn bbo(received_ms: i64, exchange_ms: i64, coin: &str) -> Record {
    bbo_prices(received_ms, exchange_ms, coin, Some("98"), Some("102"))
}

fn bbo_prices(
    received_ms: i64,
    exchange_ms: i64,
    coin: &str,
    bid: Option<&str>,
    ask: Option<&str>,
) -> Record {
    record(
        RecordKind::RecvText,
        received_ms,
        json!({"channel": "bbo", "data": {
            "coin": coin,
            "time": exchange_ms,
            "bbo": [
                bid.map(|px| json!({"px": px, "sz": "10", "n": 1})),
                ask.map(|px| json!({"px": px, "sz": "10", "n": 1}))
            ]
        }}),
    )
}

fn trade(received_ms: i64, exchange_ms: i64, tid: u64, size: &str) -> Record {
    priced_trade(received_ms, exchange_ms, tid, "A", "98", size)
}

fn priced_trade(
    received_ms: i64,
    exchange_ms: i64,
    tid: u64,
    side: &str,
    price: &str,
    size: &str,
) -> Record {
    record(
        RecordKind::RecvText,
        received_ms,
        json!({"channel": "trades", "data": [{
            "coin": "BTC", "time": exchange_ms, "tid": tid,
            "side": side, "px": price, "sz": size,
            "hash": format!("0x{}", "00".repeat(32)),
            "users": [format!("0x{}", "01".repeat(20)), format!("0x{}", "02".repeat(20))]
        }]}),
    )
}

fn read(paths: &[PathBuf]) -> Recording {
    read_recordings(paths, Network::Mainnet, "BTC").expect("read synthetic recordings")
}

#[test]
fn incremental_conversion_preserves_every_prefix_and_matches_file_conversion() {
    let files = TestFiles::new();
    let inputs = [
        (
            vec![
                header(1_000, 1, Some(0)),
                trade(1_000, 999, 1, "0.1"),
                meta(1_001, "connected"),
                bbo(1_002, 1_010, "BTC"),
                trade(1_003, 1_000, 2, "0.1"),
                trade(1_004, 1_001, 3, "0.1"),
                meta(1_005, "disconnected"),
                trade(1_006, 1_006, 4, "0.1"),
                meta(1_007, "connected"),
            ],
            false,
            Ending::Complete,
        ),
        (
            vec![
                header(1_100, 1, Some(1)),
                trade(1_101, 1_100, 5, "0.1"),
                meta(1_102, "dropped"),
                trade(1_103, 1_102, 6, "0.1"),
            ],
            false,
            Ending::Complete,
        ),
        (
            vec![
                header(2_000, 2, Some(0)),
                trade(2_001, 2_001, 7, "0.1"),
                meta(2_002, "connected"),
                bbo(2_003, 2_003, "BTC"),
                trade(2_004, 2_002, 8, "0.1"),
            ],
            true,
            Ending::RecordHeaderCut,
        ),
        (
            vec![header(3_000, 2, Some(1))],
            false,
            Ending::RecordHeaderCut,
        ),
    ];
    let paths: Vec<_> = inputs
        .iter()
        .enumerate()
        .map(|(index, (records, _, ending))| {
            files.recording(&format!("prefix-{index}.hlrec.zst"), records, *ending)
        })
        .collect();
    let expected = read(&paths);
    let mut converter = RecordConverter::new(Network::Mainnet, "BTC");
    let mut events = Vec::new();
    let mut batch = Vec::new();
    let mut check_prefix = |batch: &mut Vec<MarketEvent>| {
        events.append(batch);
        assert!(expected.events.starts_with(&events));
    };
    for (records, process_change, ending) in &inputs {
        let header = &records[0];
        let metadata: Value = serde_json::from_slice(&header.payload).expect("fixture header");
        converter
            .file_start(
                header.recv_wall_ns,
                header.recv_mono_ns,
                metadata
                    .get("process_start_wall_ns")
                    .and_then(Value::as_i64),
                *process_change,
                &mut batch,
            )
            .expect("start one synthetic file");
        check_prefix(&mut batch);
        for record in &records[1..] {
            converter
                .record(record, &mut batch)
                .expect("convert one complete record");
            check_prefix(&mut batch);
        }
        if matches!(ending, Ending::RecordHeaderCut) {
            converter
                .truncated(&mut batch)
                .expect("convert synthetic truncation");
            check_prefix(&mut batch);
        }
    }
    assert_eq!(events, expected.events);
    assert_eq!(converter.stats(), &expected.stats);
    assert_eq!(converter.stats().gaps.disconnected, 1);
    assert_eq!(converter.stats().gaps.process_change, 1);
    assert_eq!(converter.stats().truncated_files, 2);
    assert_eq!(
        events.last().expect("header-only truncation gap").ts_exch,
        Timestamp::from_millis(3_000)
    );
}

#[derive(Debug)]
struct ObservedQuoter {
    quoter: FixedSpreadQuoter,
    market: Vec<MarketEvent>,
    delivery_times: Vec<Timestamp>,
}

impl Strategy for ObservedQuoter {
    fn on_event(&mut self, ctx: &Context<'_>, event: &Event, actions: &mut Actions) {
        if let Event::Market(market) = event {
            self.market.push(market.clone());
            self.delivery_times.push(ctx.now());
        }
        self.quoter.on_event(ctx, event, actions);
    }
}

fn run_recording(recording: &Recording) -> (ReplayTrace, StrategyHost<ObservedQuoter>) {
    let mut host = StrategyHost::new(
        ObservedQuoter {
            quoter: FixedSpreadQuoter::new(FixedSpreadConfig {
                instrument: instrument_id(Network::Mainnet, "BTC"),
                half_spread: "1".parse().expect("fixed half spread"),
                qty: "1".parse().expect("fixed size"),
            }),
            market: Vec::new(),
            delivery_times: Vec::new(),
        },
        1,
    );
    let mut sim = SimGateway::new(SimConfig {
        order_latency: Duration::ZERO,
        report_latency: Duration::ZERO,
        maker_fee: "0".parse().expect("fixed fee"),
        min_order_notional: "10".parse().expect("fixed minimum notional"),
    });
    let stop = recording.stop_time.expect("complete synthetic header");
    let trace =
        replay(&mut host, &mut sim, &recording.events, stop).expect("replay synthetic recording");
    (trace, host)
}

fn command(paths: &[PathBuf]) -> Output {
    command_with_fee(paths, "0")
}

fn command_with_fee(paths: &[PathBuf], maker_fee_bps: &str) -> Output {
    command_with_settings(paths, maker_fee_bps, "0")
}

fn command_with_settings(paths: &[PathBuf], maker_fee_bps: &str, report_ms: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hl-backtest"))
        .args([
            "run",
            "--coin",
            "BTC",
            "--half-spread",
            "1",
            "--qty",
            "1",
            "--order-latency-ms",
            "0",
            "--report-latency-ms",
            report_ms,
            "--maker-fee-bps",
            maker_fee_bps,
        ])
        .args(paths)
        .output()
        .expect("run hl-backtest command")
}

fn summary(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout.last(), Some(&b'\n'));
    assert_eq!(
        output.stdout.iter().filter(|&&byte| byte == b'\n').count(),
        1
    );
    let timing = output
        .stderr
        .strip_prefix(b"hl-backtest: no grid check applied\n")
        .expect("the CLI reports when it runs without rules");
    assert!(timing.starts_with(b"read_decode_ms="));
    assert_eq!(timing.iter().filter(|&&byte| byte == b'\n').count(), 1);
    serde_json::from_slice(&output.stdout).expect("one JSON summary")
}

#[test]
fn negative_latency_is_clamped_before_delivery_and_later_events_keep_local_order() {
    let files = TestFiles::new();
    let path = files.recording(
        "clock.hlrec.zst",
        &[
            header(900, 1, Some(0)),
            meta(900, "connected"),
            bbo(1_000, 1_010, "BTC"),
            bbo(1_004, 1_002, "BTC"),
            bbo(1_005, 1_003, "BTC"),
        ],
        Ending::Complete,
    );
    let recording = read(std::slice::from_ref(&path));
    assert_eq!(recording.stats.negative_latency, 1);
    assert_eq!(recording.stats.local_time_raised, 2);
    assert_eq!(recording.stats.exchange_time_out_of_order, 2);

    let (_, host) = run_recording(&recording);
    assert_eq!(
        host.strategy().delivery_times,
        vec![Timestamp::from_millis(1_010); 3]
    );
    assert_eq!(host.strategy().market, recording.events);
    assert_eq!(recording.events[0].ts_local, recording.events[0].ts_exch);

    let result = summary(&command(&[path]));
    assert_eq!(result["first_ts_local_ms"], 1_010);
    assert_eq!(result["last_ts_local_ms"], 1_010);
    assert_eq!(result["time"]["negative_latency"], 1);
    assert_eq!(result["time"]["local_time_raised"], 2);
    assert_eq!(result["time"]["exchange_time_out_of_order"], 2);
}

#[test]
fn process_clock_floor_precedes_previous_local_time_and_keeps_raw_latency() {
    let files = TestFiles::new();
    let path = files.recording(
        "process-floor.hlrec.zst",
        &[
            header(1_000, 1_000_000_000, Some(0)),
            with_mono(bbo(1_001, 1_000, "BTC"), 10_000_007),
            with_mono(bbo(1_002, 1_005, "BTC"), 20_000_000),
            with_mono(bbo(1_030, 1_028, "BTC"), 25_000_000),
            with_mono(bbo(1_003, 1_002, "BTC"), 27_000_000),
            with_mono(bbo(1_035, 1_035, "BTC"), 30_000_000),
        ],
        Ending::Complete,
    );
    // In ns, anchor=1_000_000_000. The first P=1_010_000_007 raises
    // max(wall,E)=1_001_000_000 by 9_000_007, retaining its final 7 ns.
    // In ms, the remaining P values are 1020, 1025, 1027 and 1030.
    // Their max(wall,E) values are 1005, 1030, 1003 and 1035, so only
    // 1020-1005=15 and 1027-1003=24 add raises. The latter is then
    // raised to previous R=1030. Exactly one wall<E and one E<max_E.
    let recording = read(std::slice::from_ref(&path));
    assert_eq!(
        recording
            .events
            .iter()
            .map(|event| event.ts_local)
            .collect::<Vec<_>>(),
        [
            1_010_000_007,
            1_020_000_000,
            1_030_000_000,
            1_030_000_000,
            1_035_000_000,
        ]
        .map(Timestamp::from_nanos)
    );
    assert_eq!(recording.stop_time, Some(Timestamp::from_millis(1_035)));
    let result = summary(&command(&[path]));
    assert_eq!(
        result["time"],
        json!({
            "negative_latency": 1,
            "local_time_raised": 1,
            "exchange_time_out_of_order": 1,
            "process_clock_raised": 3,
            "process_clock_max_raise_ns": 24_000_000
        })
    );
}

#[test]
fn backward_wall_step_at_rotation_keeps_file_sequence_and_monotonic_receipt_time() {
    let files = TestFiles::new();
    let first = files.recording(
        "z-file-zero.hlrec.zst",
        &[
            with_mono(header(1_100, 1_000_000_000, Some(0)), 100_000_000),
            with_mono(bbo(1_110, 1_105, "BTC"), 110_000_000),
        ],
        Ending::Complete,
    );
    let second = files.recording(
        "a-file-one.hlrec.zst",
        &[
            with_mono(header(900, 1_000_000_000, Some(1)), 200_000_000),
            with_mono(bbo(910, 900, "BTC"), 210_000_000),
            with_mono(trade(920, 915, 1, "0.25"), 220_000_000),
        ],
        Ending::Complete,
    );
    // Sequence 0 precedes 1 despite both its later wall time and path.
    // P in ms is 1000+(110,210,220)=(1110,1210,1220).
    // max(wall,E)=(1110,910,920), so the two raises are 300 ms each.
    // E=900 and 915 both trail the earlier E=1105. The initial unknown
    // connection continues across rotation, admitting the trade.
    for paths in [
        [first.clone(), second.clone()],
        [second.clone(), first.clone()],
    ] {
        let recording = read(&paths);
        assert_eq!(
            recording
                .events
                .iter()
                .map(|event| (event.ts_exch, event.ts_local))
                .collect::<Vec<_>>(),
            [(1_105, 1_110), (900, 1_210), (915, 1_220)].map(|(exchange, local)| (
                Timestamp::from_millis(exchange),
                Timestamp::from_millis(local)
            ))
        );
        assert_eq!(recording.stats.gaps.process_change, 0);
        assert_eq!(recording.stats.trades_live, 1);
        assert_eq!(recording.stats.negative_latency, 0);
        assert_eq!(recording.stats.local_time_raised, 0);
        assert_eq!(recording.stats.exchange_time_out_of_order, 2);
        assert_eq!(recording.stats.process_clock_raised, 2);
        assert_eq!(recording.stats.process_clock_max_raise_ns, 300_000_000);
        assert_eq!(recording.stop_time, Some(Timestamp::from_millis(1_220)));
    }
}

#[test]
fn legacy_anchor_uses_both_header_stamps() {
    let files = TestFiles::new();
    let file_open = record(
        RecordKind::Meta,
        1_000,
        json!({
            "event": "file_open", "format": 1, "source": "hl",
            "url": "wss://api.hyperliquid.xyz/ws"
        }),
    );
    let path = files.recording(
        "legacy-anchor.hlrec.zst",
        &[
            with_mono(file_open, 250_000_000),
            with_mono(bbo(990, 995, "BTC"), 300_000_000),
        ],
        Ending::Complete,
    );
    // Anchor=1000-250=750 ms. P=750+300=1050 ms raises
    // max(990,995)=995 by 55 ms. Raw wall 990 still precedes E=995.
    let recording = read(&[path]);
    assert_eq!(recording.events[0].ts_local, Timestamp::from_millis(1_050));
    assert_eq!(recording.stats.process_clock_raised, 1);
    assert_eq!(recording.stats.process_clock_max_raise_ns, 55_000_000);
    assert_eq!(recording.stats.negative_latency, 1);
    assert_eq!(recording.stop_time, Some(Timestamp::from_millis(1_050)));
}

#[test]
fn process_clock_does_not_move_the_raw_wall_connection_cutoff() {
    let files = TestFiles::new();
    let path = files.recording(
        "raw-cutoff.hlrec.zst",
        &[
            header(1_000, 1_000_000_000, Some(0)),
            with_mono(meta(900, "connected"), 200_000_000),
            with_mono(trade(910, 899, 1, "0.1"), 210_000_000),
            with_mono(trade(911, 950, 2, "0.1"), 211_000_000),
        ],
        Ending::Complete,
    );
    // The cutoff is raw 900 ms, although connected P=1000+200=1200.
    // E=899 is excluded, E=950 is admitted. For that trade,
    // P=1000+211=1211 raises max(911,950)=950 by 261 ms.
    let recording = read(&[path]);
    assert_eq!(recording.events.len(), 1);
    assert!(matches!(
        &recording.events[0].kind,
        MarketEventKind::Trade(trade) if trade.tid.0 == 2
    ));
    assert_eq!(recording.events[0].ts_local, Timestamp::from_millis(1_211));
    assert_eq!(recording.stats.trades_replayed, 1);
    assert_eq!(recording.stats.trades_live, 1);
    assert_eq!(recording.stats.negative_latency, 1);
    assert_eq!(recording.stats.process_clock_raised, 1);
    assert_eq!(recording.stats.process_clock_max_raise_ns, 261_000_000);
}

#[test]
fn every_gap_uses_its_own_record_stamps_and_process_anchor() {
    let files = TestFiles::new();
    let first = files.recording(
        "gap-records.hlrec.zst",
        &[
            header(1_000, 1_000_000_000, Some(0)),
            with_mono(bbo(1_001, 1_001, "BTC"), 1_000_000),
            with_mono(meta(900, "disconnected"), 10_000_000),
            with_mono(meta(901, "shutdown"), 20_000_000),
            with_mono(meta(902, "dropped"), 30_000_000),
            with_mono(
                record(RecordKind::SentText, 903, json!({"method": "ping"})),
                40_000_000,
            ),
        ],
        Ending::RecordHeaderCut,
    );
    let second = files.recording(
        "header-only-gap.hlrec.zst",
        &[with_mono(header(1_100, 2_000_000_000, Some(0)), 50_000_000)],
        Ending::RecordHeaderCut,
    );
    // First-process gaps use P=1000+(10,20,30,40), including the last
    // complete ping for truncation. Their raises over raw wall are
    // 110,119,128,137 ms. The new header uses the NEW anchor, giving
    // P=2000+50=2050 for both its process-change and truncation gaps.
    // Each raises raw 1100 by 950 ms. Only the first four gap exchange
    // stamps are below the earlier BBO's E=1001. No previous-R raise.
    let recording = read(&[second, first]);
    let gaps: Vec<_> = recording
        .events
        .iter()
        .filter(|event| matches!(event.kind, MarketEventKind::FeedGap))
        .map(|event| (event.ts_exch, event.ts_local))
        .collect();
    assert_eq!(
        gaps,
        [
            (900, 1_010),
            (901, 1_020),
            (902, 1_030),
            (903, 1_040),
            (1_100, 2_050),
            (1_100, 2_050),
        ]
        .map(|(exchange, local)| (
            Timestamp::from_millis(exchange),
            Timestamp::from_millis(local)
        ))
    );
    assert_eq!(recording.stats.gaps.disconnected, 1);
    assert_eq!(recording.stats.gaps.shutdown, 1);
    assert_eq!(recording.stats.gaps.dropped, 1);
    assert_eq!(recording.stats.gaps.process_change, 1);
    assert_eq!(recording.stats.gaps.truncated, 2);
    assert_eq!(recording.stats.truncated_files, 2);
    assert_eq!(recording.stats.process_clock_raised, 6);
    assert_eq!(recording.stats.process_clock_max_raise_ns, 950_000_000);
    assert_eq!(recording.stats.local_time_raised, 0);
    assert_eq!(recording.stats.exchange_time_out_of_order, 4);
    assert_eq!(recording.stop_time, Some(Timestamp::from_millis(2_050)));
}

#[test]
fn final_control_process_clock_closes_report_deadlines_inclusively() {
    let files = TestFiles::new();
    // A=1001, proved by E=1002. Acks are due at max(1001+5,1002)=1006.
    // The final ping has raw wall=900, but its P is 1000+mono ms.
    // Mono=5 stops at 1005 with 2 pending. Mono=6 includes both acks.
    // The ping creates no event, so it adds no process-clock counter.
    for (mono_ns, stop_ms, accepted, pending) in
        [(5_000_000, 1_005, 0, 2), (6_000_000, 1_006, 2, 0)]
    {
        let path = files.recording(
            "process-stop.hlrec.zst",
            &[
                header(1_000, 1_000_000_000, Some(0)),
                with_mono(bbo(1_001, 1_001, "BTC"), 1_000_000),
                with_mono(bbo(1_002, 1_002, "BTC"), 2_000_000),
                with_mono(
                    record(RecordKind::SentText, 900, json!({"method": "ping"})),
                    mono_ns,
                ),
            ],
            Ending::Complete,
        );
        let recording = read(std::slice::from_ref(&path));
        assert_eq!(recording.stop_time, Some(Timestamp::from_millis(stop_ms)));
        let result = summary(&command_with_settings(&[path], "0", "5"));
        assert_eq!(result["last_ts_local_ms"], 1_002);
        assert_eq!(result["reports"]["accepted"], accepted);
        assert_eq!(result["pending_at_end"]["reports"], pending);
        assert_eq!(result["time"]["process_clock_raised"], 0);
        assert_eq!(result["time"]["process_clock_max_raise_ns"], 0);
    }

    // A complete header alone stops at max(wall=900,P=1000+10)=1010 ms.
    let path = files.recording(
        "header-only-stop.hlrec.zst",
        &[with_mono(header(900, 1_000_000_000, Some(0)), 10_000_000)],
        Ending::Complete,
    );
    let recording = read(&[path]);
    assert!(recording.events.is_empty());
    assert_eq!(recording.stop_time, Some(Timestamp::from_millis(1_010)));
    assert_eq!(recording.stats.process_clock_raised, 0);
}

#[test]
fn numbered_chains_merge_their_next_headers_and_keep_interleaved_process_gaps() {
    let files = TestFiles::new();
    let specifications = [
        ("d-a0.hlrec.zst", 1_000, 1, 7, 1_001),
        ("c-b0.hlrec.zst", 1_500, 2, 0, 1_501),
        ("b-a1.hlrec.zst", 2_000, 1, 8, 2_001),
        ("a-b1.hlrec.zst", 2_500, 2, 1, 2_501),
    ];
    let paths: Vec<_> = specifications
        .map(|(name, open, process, sequence, event)| {
            files.recording(
                name,
                &[
                    header(open, process, Some(sequence)),
                    bbo(event, event, "BTC"),
                ],
                Ending::Complete,
            )
        })
        .into_iter()
        .rev()
        .collect();
    // Heads are A7=1000, B0=1500, A8=2000, B1=2500 ms. Taking one
    // head at a time gives A7,B0,A8,B1 and three process changes.
    // P is 1 or 2 ns, always below wall, so all raises remain zero.
    let recording = read(&paths);
    assert_eq!(
        recording
            .events
            .iter()
            .map(|event| event.ts_local)
            .collect::<Vec<_>>(),
        [1_001, 1_500, 1_501, 2_000, 2_001, 2_500, 2_501].map(Timestamp::from_millis)
    );
    assert_eq!(recording.stats.bbo, 4);
    assert_eq!(recording.stats.gaps.process_change, 3);
    assert_eq!(recording.stats.process_clock_raised, 0);
    assert_eq!(recording.stats.process_clock_max_raise_ns, 0);
    assert_eq!(recording.stats.local_time_raised, 0);
}

#[test]
fn equal_chain_head_wall_times_use_path_order() {
    let files = TestFiles::new();
    let first = files.recording(
        "a-process-two.hlrec.zst",
        &[header(1_000, 2, Some(0)), bbo(1_001, 1_001, "BTC")],
        Ending::Complete,
    );
    let second = files.recording(
        "z-process-one.hlrec.zst",
        &[header(1_000, 1, Some(0)), bbo(1_002, 1_002, "BTC")],
        Ending::Complete,
    );
    // Equal 1000-ms heads choose path a before z, regardless of process
    // identity. The gap's raw 1000 is raised to the preceding R=1001.
    let recording = read(&[second, first]);
    assert_eq!(
        recording
            .events
            .iter()
            .map(|event| (event.ts_exch, event.ts_local))
            .collect::<Vec<_>>(),
        [(1_001, 1_001), (1_000, 1_001), (1_002, 1_002)].map(|(exchange, local)| (
            Timestamp::from_millis(exchange),
            Timestamp::from_millis(local)
        ))
    );
    assert_eq!(recording.stats.gaps.process_change, 1);
    assert_eq!(recording.stats.local_time_raised, 1);
    assert_eq!(recording.stats.exchange_time_out_of_order, 1);
}

#[test]
fn clock_arithmetic_checks_headers_events_and_ignored_records_for_overflow() {
    let files = TestFiles::new();
    let overflowing_inputs = [
        vec![with_mono(header(0, i64::MAX, Some(0)), 1)],
        vec![header(0, i64::MAX, Some(0)), with_mono(bbo(0, 0, "BTC"), 1)],
        vec![
            header(0, i64::MAX, Some(0)),
            with_mono(record(RecordKind::SentText, 0, json!({})), 1),
        ],
    ];
    // In each case, P=(2^63-1)+1=2^63 exceeds the i64 timestamp range.
    for (index, records) in overflowing_inputs.iter().enumerate() {
        let path = files.recording(
            &format!("overflow-{index}.hlrec.zst"),
            records,
            Ending::Complete,
        );
        let error = read_recordings(&[path], Network::Mainnet, "BTC")
            .expect_err("overflowing clock must fail");
        assert!(error.to_string().contains("overflow"), "{error}");
    }

    // Full u64 monotonic values can still fit after adding a negative
    // anchor: -2^63+(2^64-1)=2^63-1, exactly i64::MAX. Raising raw
    // wall=-2^63 to P=2^63-1 adds 2^64-1 ns, exactly u64::MAX.
    let mut dropped = with_mono(meta(0, "dropped"), u64::MAX);
    dropped.recv_wall_ns = i64::MIN;
    let path = files.recording(
        "largest-valid-clock.hlrec.zst",
        &[header(0, i64::MIN, Some(0)), dropped],
        Ending::Complete,
    );
    let recording = read(&[path]);
    assert_eq!(
        recording.events[0].ts_local,
        Timestamp::from_nanos(i64::MAX)
    );
    assert_eq!(recording.stats.process_clock_raised, 1);
    assert_eq!(recording.stats.process_clock_max_raise_ns, u64::MAX);
    assert_eq!(recording.stop_time, Some(Timestamp::from_nanos(i64::MAX)));

    // An absent anchor would require -2^63-1 here, below i64::MIN.
    let mut legacy = record(
        RecordKind::Meta,
        0,
        json!({"event": "file_open", "url": "wss://api.hyperliquid.xyz/ws"}),
    );
    legacy.recv_wall_ns = i64::MIN;
    legacy.recv_mono_ns = 1;
    let path = files.recording("legacy-overflow.hlrec.zst", &[legacy], Ending::Complete);
    let error = read_recordings(&[path], Network::Mainnet, "BTC")
        .expect_err("overflowing legacy anchor must fail");
    assert!(error.to_string().contains("overflow"), "{error}");
}

#[test]
fn subscription_replays_reach_neither_strategy_nor_simulator_but_live_trades_do() {
    let files = TestFiles::new();
    let path = files.recording(
        "trades.hlrec.zst",
        &[
            header(1_000, 1, Some(0)),
            meta(1_000, "connected"),
            bbo(1_001, 1_001, "BTC"),
            meta(2_000, "connected"),
            // This would fill the existing bid if replayed into the simulator.
            trade(2_001, 1_500, 41, "0.75"),
            trade(2_002, 2_000, 42, "0.25"),
        ],
        Ending::Complete,
    );
    let recording = read(std::slice::from_ref(&path));
    assert_eq!(recording.stats.trades_replayed, 1);
    assert_eq!(recording.stats.trades_live, 1);

    let (trace, host) = run_recording(&recording);
    assert_eq!(host.strategy().market.len(), 2);
    assert!(matches!(
        &host.strategy().market[1].kind,
        MarketEventKind::Trade(trade) if trade.tid.0 == 42
    ));
    let fills: Vec<_> = trace
        .order_events
        .iter()
        .filter_map(|event| match event.kind {
            OrderEventKind::Filled(fill) => Some(fill),
            _ => None,
        })
        .collect();
    assert_eq!(fills.len(), 1);
    assert_eq!(fills[0].qty, "0.25".parse().expect("fixed fill size"));

    let result = summary(&command(&[path]));
    assert_eq!(result["skipped"]["trades_replayed"], 1);
    assert_eq!(result["events"]["trades_live"], 1);
    assert_eq!(result["reports"]["filled"], 1);
    assert_eq!(result["fills"]["count"], 1);
    assert_eq!(result["fills"]["bought_qty"], "0.25");
    assert_eq!(result["fills"]["sold_qty"], "0");
}

#[test]
fn mid_connection_start_keeps_trades_and_reconnect_restores_the_cutoff() {
    let files = TestFiles::new();
    let first = files.recording(
        "first.hlrec.zst",
        &[
            header(1_000, 1, Some(17)),
            // A trade can predate the file header and still belong to the connection.
            trade(1_000, 999, 40, "0.1"),
            bbo(1_001, 1_001, "BTC"),
            trade(1_002, 1_002, 41, "0.1"),
            trade(1_003, 1_003, 42, "0.2"),
        ],
        Ending::Complete,
    );
    let second = files.recording(
        "second.hlrec.zst",
        &[
            header(1_100, 1, Some(18)),
            trade(1_101, 1_101, 43, "0.25"),
            meta(1_200, "disconnected"),
            // E=1201 proves the cancel at 1200. Its reports follow this BBO,
            // so the second BBO can place a new bid for the trade-filter test.
            bbo(1_201, 1_201, "BTC"),
            bbo(1_202, 1_202, "BTC"),
            trade(1_203, 1_203, 44, "0.5"),
            meta(1_300, "connected"),
            trade(1_301, 1_299, 45, "0.75"),
            trade(1_302, 1_300, 46, "0.4"),
            trade(1_303, 1_303, 47, "0.15"),
        ],
        Ending::Complete,
    );
    let paths = [first, second];
    let recording = read(&paths);
    let (trace, host) = run_recording(&recording);
    let trade_ids: Vec<_> = host
        .strategy()
        .market
        .iter()
        .filter_map(|event| match &event.kind {
            MarketEventKind::Trade(trade) => Some(trade.tid.0),
            _ => None,
        })
        .collect();
    assert_eq!(trade_ids, [40, 41, 42, 43, 46, 47]);
    let fill_quantities: Vec<_> = trace
        .order_events
        .iter()
        .filter_map(|event| match event.kind {
            OrderEventKind::Filled(fill) => Some(fill.qty),
            _ => None,
        })
        .collect();
    assert_eq!(
        fill_quantities,
        ["0.1", "0.2", "0.25", "0.4", "0.15"].map(|qty| qty.parse().expect("fixed fill size"))
    );

    let result = summary(&command(&paths));
    assert_eq!(result["events"]["trades_live"], 6);
    assert_eq!(result["skipped"]["trades_replayed"], 2);
}

#[test]
fn every_gap_cause_emits_one_event_and_one_summary_count() {
    let files = TestFiles::new();
    let first = files.recording(
        "first.hlrec.zst",
        &[
            header(1_000, 1, Some(0)),
            meta(1_000, "connected"),
            bbo(1_001, 1_001, "BTC"),
            meta(1_100, "disconnected"),
            meta(1_200, "connected"),
            meta(1_300, "shutdown"),
            trade(1_301, 1_301, 1, "0.25"),
            meta(1_400, "connected"),
            meta(1_500, "dropped"),
            trade(1_501, 1_399, 2, "0.25"),
            trade(1_502, 1_400, 3, "0.25"),
        ],
        Ending::Complete,
    );
    let second = files.recording(
        "second.hlrec.zst",
        &[
            header(2_000, 2, Some(0)),
            meta(2_000, "connected"),
            bbo(2_001, 2_001, "BTC"),
        ],
        Ending::RecordHeaderCut,
    );
    let paths = [first, second];
    let recording = read(&paths);
    let gaps: Vec<_> = recording
        .events
        .iter()
        .filter(|event| matches!(event.kind, MarketEventKind::FeedGap))
        .map(|event| {
            assert_eq!(event.ts_exch, event.ts_local);
            event.ts_local
        })
        .collect();
    assert_eq!(
        gaps,
        [1_100, 1_300, 1_500, 2_000, 2_001].map(Timestamp::from_millis)
    );
    let (_, host) = run_recording(&recording);
    assert_eq!(host.strategy().market, recording.events);

    let result = summary(&command(&paths));
    assert_eq!(result["events"]["feed_gaps"], 5);
    assert_eq!(result["events"]["trades_live"], 1);
    assert_eq!(result["skipped"]["trades_replayed"], 2);
    assert_eq!(result["skipped"]["truncated_files"], 1);
    assert_eq!(result["actions"]["cancel_all"], 5);
    // The trade at E=1400 proves both 1001 placements and the 1100
    // cancel-all, which cancels two orders. The BBO and truncation at
    // 2001 submit 2 places + 1 cancel-all with no later proof.
    assert_eq!(result["reports"]["canceled"], 2);
    assert_eq!(result["open_orders_at_end"], 2);
    assert_eq!(
        result["pending_at_end"],
        json!({"actions": 3, "reports": 0, "timers": 0})
    );
    assert_eq!(
        result["gaps"],
        json!({
            "disconnected": 1, "shutdown": 1, "dropped": 1,
            "process_change": 1, "truncated": 1
        })
    );
}

#[test]
fn cuts_inside_records_or_zstd_frames_preserve_complete_records_and_add_one_gap() {
    let files = TestFiles::new();
    for ending in [
        Ending::RecordHeaderCut,
        Ending::RecordPayloadCut,
        Ending::ZstdCut,
    ] {
        let path = files.recording(
            &format!("{ending:?}.hlrec.zst"),
            &[
                header(1_000, 1, Some(0)),
                meta(1_000, "connected"),
                bbo(1_001, 1_001, "BTC"),
            ],
            ending,
        );
        let recording = read(std::slice::from_ref(&path));
        assert_eq!(recording.events.len(), 2, "{ending:?}");
        assert!(matches!(recording.events[0].kind, MarketEventKind::Bbo(_)));
        assert!(matches!(recording.events[1].kind, MarketEventKind::FeedGap));
        assert_eq!(recording.stats.truncated_files, 1, "{ending:?}");
        assert_eq!(recording.stats.gaps.truncated, 1, "{ending:?}");
        assert_eq!(recording.stats.feed_gaps, 1, "{ending:?}");
        assert_eq!(recording.stats.gaps.disconnected, 0, "{ending:?}");
        let result = summary(&command(&[path]));
        assert_eq!(result["events"]["bbo"], 1, "{ending:?}");
        assert_eq!(result["gaps"]["truncated"], 1, "{ending:?}");
    }
}

#[test]
fn recording_header_errors_keep_the_path_and_validation_order() {
    let files = TestFiles::new();
    for (payload, message) in [
        (
            json!({"event":"file_open", "format":2, "file_seq":0}),
            "invalid file_open format",
        ),
        (
            json!({"event":"file_open", "file_seq":0}),
            "numbered file_open needs source and process_start_wall_ns",
        ),
        (json!({"event":"file_open"}), "invalid file_open url"),
        (
            json!({"event":"file_open", "url":"ws://example.invalid/ws"}),
            "unsupported recording URL",
        ),
    ] {
        let path = files.recording(
            "bad-header.hlrec.zst",
            &[record(RecordKind::Meta, 1, payload)],
            Ending::Complete,
        );
        let error = read_recordings(std::slice::from_ref(&path), Network::Mainnet, "BTC")
            .expect_err("invalid header");
        assert_eq!(error.to_string(), format!("{}: {message}", path.display()));
    }
}

#[test]
fn a_missing_sequence_exits_one_and_identifies_the_missing_file() {
    let files = TestFiles::new();
    let first = files.recording(
        "first.hlrec.zst",
        &[header(1_000, 1, Some(0)), bbo(1_001, 1_001, "BTC")],
        Ending::Complete,
    );
    let third = files.recording(
        "third.hlrec.zst",
        &[header(3_000, 1, Some(2)), bbo(3_001, 3_001, "BTC")],
        Ending::Complete,
    );
    let output = command(&[third, first]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("missing file_seq 1"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn header_order_makes_shuffled_paths_identical_and_preserves_connection_state() {
    let files = TestFiles::new();
    let earlier = files.recording(
        "z-earlier.hlrec.zst",
        &[
            header(1_000, 1, Some(0)),
            meta(1_000, "connected"),
            bbo(1_001, 1_001, "BTC"),
        ],
        Ending::Complete,
    );
    let later = files.recording(
        "a-later.hlrec.zst",
        &[
            header(2_000, 1, Some(1)),
            bbo(2_001, 2_001, "BTC"),
            trade(2_100, 2_100, 1, "0.25"),
        ],
        Ending::Complete,
    );
    let forward = command(&[earlier.clone(), later.clone()]);
    let backward = command(&[later, earlier]);
    let result = summary(&forward);
    summary(&backward);
    assert_eq!(forward.stdout, backward.stdout);
    assert_eq!(result["files"], 2);
    assert_eq!(result["events"]["bbo"], 2);
    assert_eq!(result["events"]["trades_live"], 1);
    assert_eq!(result["skipped"]["trades_replayed"], 0);
    assert_eq!(result["gaps"]["process_change"], 0);
    assert_eq!(result["fills"]["bought_qty"], "0.25");
}

#[test]
fn equal_header_times_use_path_order() {
    let files = TestFiles::new();
    let first = files.recording(
        "a-first.hlrec.zst",
        &[header(1_000, 1, Some(0)), bbo(1_001, 1_001, "BTC")],
        Ending::Complete,
    );
    let second = files.recording(
        "b-second.hlrec.zst",
        &[header(1_000, 1, Some(1)), bbo(1_002, 1_002, "BTC")],
        Ending::Complete,
    );
    let output = command(&[second, first]);
    let result = summary(&output);
    assert_eq!(result["first_ts_local_ms"], 1_001);
    assert_eq!(result["last_ts_local_ms"], 1_002);
}

#[test]
fn legacy_files_are_independent_processes_even_when_process_identifiers_match() {
    let files = TestFiles::new();
    let first = files.recording(
        "first.hlrec.zst",
        &[
            header(1_000, 1, None),
            meta(1_000, "connected"),
            bbo(1_001, 1_001, "BTC"),
        ],
        Ending::Complete,
    );
    let second = files.recording(
        "second.hlrec.zst",
        &[
            header(2_000, 1, None),
            trade(2_001, 2_001, 1, "0.25"),
            bbo(2_002, 2_002, "BTC"),
        ],
        Ending::Complete,
    );
    let result = summary(&command(&[first, second]));
    assert_eq!(result["gaps"]["process_change"], 1);
    assert_eq!(result["events"]["trades_live"], 0);
    assert_eq!(result["skipped"]["trades_replayed"], 1);
}

#[test]
fn other_coins_depth_and_non_market_records_are_skipped_and_decode_errors_counted() {
    let files = TestFiles::new();
    let mut malformed = bbo(1_002, 1_002, "BTC");
    malformed.payload = b"{".to_vec();
    let path = files.recording(
        "filtered.hlrec.zst",
        &[
            header(1_000, 1, Some(0)),
            bbo(1_001, 1_001, "ETH"),
            malformed,
            record(
                RecordKind::RecvText,
                1_003,
                json!({"channel": "l2", "data": {"s": {
                    "coin": "BTC", "time": 1_003, "levels": [[], []]
                }}}),
            ),
            record(RecordKind::RecvText, 1_004, json!({"channel": "pong"})),
            record(RecordKind::RecvBinary, 1_005, json!("binary")),
            record(RecordKind::SentText, 1_006, json!({"method": "ping"})),
            record(
                RecordKind::RecvText,
                1_007,
                json!({"channel": "bbo", "data": {}}),
            ),
            record(
                RecordKind::RecvText,
                1_008,
                json!({"channel": "l2", "data": {}}),
            ),
            record(
                RecordKind::RecvText,
                1_009,
                json!({"channel": "error", "data": {}}),
            ),
            bbo(1_010, 1_010, "BTC"),
        ],
        Ending::Complete,
    );
    let recording = read(std::slice::from_ref(&path));
    assert_eq!(recording.events.len(), 1);
    assert_eq!(recording.stats.decode_errors, 4);
    let result = summary(&command(&[path]));
    assert_eq!(result["events"]["bbo"], 1);
    assert_eq!(result["skipped"]["decode_errors"], 4);
}

#[test]
fn unreadable_files_and_a_coin_without_bbo_exit_one() {
    let files = TestFiles::new();
    let absent = command(&[files.directory.join("absent.hlrec.zst")]);
    assert_eq!(absent.status.code(), Some(1));
    assert!(absent.stdout.is_empty());
    assert!(!absent.stderr.is_empty());

    let other_coin = files.recording(
        "other-coin.hlrec.zst",
        &[header(1_000, 1, Some(0)), bbo(1_001, 1_001, "ETH")],
        Ending::Complete,
    );
    let output = command(&[other_coin]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    assert!(diagnostic.contains("BTC"));
    assert!(diagnostic.to_ascii_lowercase().contains("bbo"));
}

#[test]
fn rejected_orders_are_counted_by_event_and_reason() {
    let files = TestFiles::new();
    let small_notional = record(
        RecordKind::RecvText,
        1_001,
        json!({"channel": "bbo", "data": {
            "coin": "BTC", "time": 1_001,
            "bbo": [
                {"px": "1", "sz": "10", "n": 1},
                {"px": "3", "sz": "10", "n": 1}
            ]
        }}),
    );
    let path = files.recording(
        "rejections.hlrec.zst",
        &[
            header(1_000, 1, Some(0)),
            small_notional,
            // E=1002 > 1001 proves both places. 1*1 and 3*1 are below 10.
            trade(1_002, 1_002, 1, "1"),
        ],
        Ending::Complete,
    );
    let result = summary(&command(&[path]));
    assert_eq!(result["actions"]["place"], 2);
    assert_eq!(result["reports"]["rejected"], 2);
    assert_eq!(
        result["reports"]["rejections_by_reason"]["below_min_notional"],
        2
    );
    assert_eq!(result["reports"]["accepted"], 0);
    assert_eq!(result["fills"]["count"], 0);
    assert_eq!(result["open_orders_at_end"], 0);
}

#[test]
fn pnl_uses_exchange_order_and_marks_before_fills_at_equal_times() {
    let files = TestFiles::new();
    let path = files.recording(
        "pnl.hlrec.zst",
        &[
            header(1_000, 1, Some(0)),
            meta(1_000, "connected"),
            bbo(1_001, 1_001, "BTC"),
            trade(1_100, 1_100, 1, "1"),
            priced_trade(1_201, 1_200, 2, "B", "102", "1"),
            // Received after the closing trade, with the same exchange time.
            bbo_prices(1_250, 1_200, "BTC", Some("88"), Some("92")),
            priced_trade(1_300, 1_300, 3, "A", "88", "1"),
            bbo_prices(1_400, 1_400, "BTC", Some("88"), Some("92")),
            priced_trade(1_500, 1_500, 4, "A", "88", "0.5"),
            bbo_prices(1_600, 1_600, "BTC", Some("90"), Some("94")),
            // This late BBO belongs before the final mark in exchange order.
            bbo_prices(1_700, 1_550, "BTC", Some("89"), Some("93")),
            // Locked, crossed and one-sided BBOs cannot replace the final mark.
            bbo_prices(1_800, 1_800, "BTC", Some("80"), Some("80")),
            bbo_prices(1_900, 1_900, "BTC", Some("82"), Some("80")),
            bbo_prices(2_000, 2_000, "BTC", Some("1"), None),
        ],
        Ending::Complete,
    );

    // The fills are buy 1@99, sell 1@101, buy 1@89 and buy 0.5@89.
    // P = 1 - 1 + 1 + 0.5 = 1.5, also the largest absolute position.
    // C = -99 + 101 - 89 - 44.5 = -131.5.
    // N = 99 + 101 + 89 + 44.5 = 333.5.
    // Each fee is the fill's notional * 1.5/10000, giving
    // F = 0.01485 + 0.01515 + 0.01335 + 0.006675 = 0.050025.
    // At the final mid 92, G = -131.5 + 1.5*92 = 6.5,
    // and net = 6.5 - 0.050025 = 6.449975.
    // The first buy has net = -99 + 1*100 - 0.01485 = 0.98515.
    // At exchange 1200, the mid 90 mark precedes the sell. With P=1 and
    // C=-99, net = -99 + 1*90 - 0.01485 = -9.01485, the minimum.
    // Selling first would close flat at net = 2 - 0.03 = 1.97 and miss
    // this minimum entirely. Later net values are 2.95665, 3.449975,
    // 4.949975 and 6.449975, all above the minimum.
    let first = command_with_fee(std::slice::from_ref(&path), "1.5");
    let second = command_with_fee(&[path], "1.5");
    let result = summary(&first);
    summary(&second);
    assert_eq!(first.stdout, second.stdout);
    assert_eq!(
        result["fills"],
        json!({"count": 4, "bought_qty": "2.5", "sold_qty": "1"})
    );
    assert_eq!(result["events"]["bbo"], 8);
    assert_eq!(result["time"]["exchange_time_out_of_order"], 1);
    assert_eq!(
        result["pnl"],
        json!({
            "excluded": ["funding"],
            "position": "1.5",
            "mark_px": "92",
            "mark_ts_exch_ms": 1_600,
            "notional": "333.5",
            "fees": "0.050025",
            "gross": "6.5",
            "net": "6.449975",
            "max_abs_position": "1.5",
            "min_net": "-9.01485",
            "min_net_ts_exch_ms": 1_200
        })
    );
}

#[test]
fn pnl_without_a_usable_mark_keeps_a_flat_zero_account() {
    let files = TestFiles::new();
    let path = files.recording(
        "unusable-marks.hlrec.zst",
        &[
            header(1_000, 1, Some(0)),
            bbo_prices(1_001, 1_001, "BTC", Some("98"), None),
            bbo_prices(1_002, 1_002, "BTC", None, Some("102")),
            bbo_prices(1_003, 1_003, "BTC", Some("100"), Some("100")),
            bbo_prices(1_004, 1_004, "BTC", Some("102"), Some("98")),
        ],
        Ending::Complete,
    );
    let result = summary(&command(&[path]));
    assert_eq!(result["events"]["bbo"], 4);
    assert_eq!(result["fills"]["count"], 0);
    // No fills leaves P=C=F=N=0, so G=net=0 even without a mark.
    // The initial minimum is zero and no later step makes a lower low.
    assert_eq!(
        result["pnl"],
        json!({
            "excluded": ["funding"],
            "position": "0",
            "mark_px": null,
            "mark_ts_exch_ms": null,
            "notional": "0",
            "fees": "0",
            "gross": "0",
            "net": "0",
            "max_abs_position": "0",
            "min_net": "0",
            "min_net_ts_exch_ms": null
        })
    );
}

#[test]
fn an_inexact_fill_exits_one_without_a_summary_and_identifies_the_fill() {
    let files = TestFiles::new();
    let path = files.recording(
        "inexact-fill.hlrec.zst",
        &[
            header(1_000, 1, Some(0)),
            bbo_prices(
                1_001,
                1_001,
                "BTC",
                Some("98.00000001"),
                Some("102.00000001"),
            ),
            trade(1_100, 1_100, 1, "0.1"),
        ],
        Ending::Complete,
    );
    // Mid = (98.00000001 + 102.00000001)/2 = 100.00000001.
    // With half-spread 1, the buy rests at 99.00000001. The sell trade
    // fills 0.1 at that price, so notional = 99.00000001*0.1 = 9.900000001.
    // Nine decimal places cannot be represented exactly in the e8 types.
    let output = command(&[path]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    for expected in [
        "inexact product",
        "tid=1",
        "px=99.00000001",
        "qty=0.1",
        "ts_exch_ms=1100",
    ] {
        assert!(diagnostic.contains(expected), "{diagnostic}");
    }
}

#[test]
fn final_control_record_closes_report_deadlines_inclusively() {
    let files = TestFiles::new();
    // A=1001, proved by E=1002. Both acks are due at max(1001+5,1002)=1006.
    // The last BBO stays at 1002. A later control record sets the actual stop.
    for (stop, delivered, pending) in [(1_005, 0, 2), (1_006, 2, 0)] {
        let path = files.recording(
            "cutoff.hlrec.zst",
            &[
                header(1_000, 1, Some(0)),
                bbo(1_001, 1_001, "BTC"),
                bbo(1_002, 1_002, "BTC"),
                record(RecordKind::RecvText, stop, json!({"channel": "pong"})),
            ],
            Ending::Complete,
        );
        let result = summary(&command_with_settings(&[path], "0", "5"));
        assert_eq!(result["last_ts_local_ms"], 1_002);
        assert_eq!(result["reports"]["accepted"], delivered);
        assert_eq!(result["report_wait_ns"]["action"]["count"], delivered);
        assert_eq!(result["pending_at_end"]["reports"], pending);
    }
}

#[test]
fn late_print_quantity_at_the_old_bid_never_reaches_the_ledger() {
    let files = TestFiles::new();
    let path = files.recording(
        "late-trade.hlrec.zst",
        &[
            header(1_000, 1, Some(0)),
            bbo(1_000, 1_000, "BTC"),
            trade(1_010, 1_010, 1, "0.6"),
            bbo_prices(1_020, 1_020, "BTC", Some("93"), Some("97")),
            bbo_prices(1_030, 1_030, "BTC", Some("93"), Some("97")),
            trade(1_040, 1_020, 2, "0.6"),
        ],
        Ending::Complete,
    );
    // Bid 99 fills 0.6 at 1010. At S=1020 the quoter moves it to 94.
    // The late sell at 98 with E=S could take min(1-0.6,0.6)=0.4 at 99.
    // Only the actual 0.6 contributes to cash, fees and final inventory.
    let result = summary(&command_with_fee(&[path], "1.5"));
    assert_eq!(
        result["late_print_misses"],
        json!({"trades": 1, "qty": "0.4"})
    );
    assert_eq!(
        result["fills"],
        json!({"count": 1, "bought_qty": "0.6", "sold_qty": "0"})
    );
    assert_eq!(result["pnl"]["position"], "0.6");
    assert_eq!(result["pnl"]["notional"], "59.4"); // 99 * 0.6
    assert_eq!(result["pnl"]["fees"], "0.00891"); // 59.4 * 0.00015
    assert_eq!(result["pnl"]["net"], "-2.40891"); // (95-99)*0.6 - 0.00891
}

#[test]
fn summary_keys_follow_the_contract_and_configuration_defaults_are_explicit() {
    let files = TestFiles::new();
    let path = files.recording(
        "summary.hlrec.zst",
        &[header(1_000, 1, Some(0)), bbo(1_001, 1_001, "BTC")],
        Ending::Complete,
    );
    let output = command(&[path]);
    let result = summary(&output);
    assert_eq!(result["coin"], "BTC");
    assert_eq!(result["network"], "mainnet");
    assert_eq!(result["half_spread"], "1");
    assert_eq!(result["qty"], "1");
    assert_eq!(result["order_latency_ms"], 0);
    assert_eq!(result["report_latency_ms"], 0);
    assert_eq!(result["maker_fee_bps"], "0");
    assert_eq!(result["min_notional"], "10");
    assert_eq!(result["actions"]["place"], 2);
    // The only BBO submits both places at A=1001. There are no later
    // inputs, so neither has been proved and both are locally pending.
    assert_eq!(result["reports"]["accepted"], 0);
    assert_eq!(result["open_orders_at_end"], 2);
    assert_eq!(
        result["late_print_misses"],
        json!({"trades": 0, "qty": "0"})
    );
    assert_eq!(
        result["pending_at_end"],
        json!({"actions": 2, "reports": 0, "timers": 0})
    );
    let empty_waits = json!({"count": 0, "delayed": 0, "p50": null,
                             "p90": null, "p99": null, "max": null});
    assert_eq!(result["report_wait_ns"]["action"], empty_waits);
    assert_eq!(result["report_wait_ns"]["fill"], empty_waits);

    let text = String::from_utf8(output.stdout).expect("UTF-8 summary");
    let mut previous = None;
    for key in [
        "coin",
        "network",
        "files",
        "half_spread",
        "qty",
        "order_latency_ms",
        "report_latency_ms",
        "maker_fee_bps",
        "min_notional",
        "first_ts_local_ms",
        "last_ts_local_ms",
        "events",
        "skipped",
        "gaps",
        "time",
        "actions",
        "reports",
        "fills",
        "open_orders_at_end",
        "late_print_misses",
        "pending_at_end",
        "report_wait_ns",
        "pnl",
    ] {
        let position = text.find(&format!("\"{key}\":")).expect("summary key");
        if let Some(before) = previous {
            assert!(position > before, "{key} must follow the preceding key");
        }
        previous = Some(position);
    }
}
