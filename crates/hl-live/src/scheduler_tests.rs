//! Lock-boundary tests use real loopback reads and an explicitly advanced clock.
//! Channels control admission and handoff. Timeouts only bound test failures.

use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use hl_codec::hyperliquid::{Network, instrument_id};
use hl_codec::rawlog::{Record, RecordKind};
use hl_recorder::{SourceConfig, SourceSink, Stamp};
use hl_run::recording::{EventOrigin, RecordConverter};
use hl_run::session::{Session, SessionStart};
use hl_run::{IncrementalDriver, replay_with_session};
use hl_sim::{SimConfig, SimGateway};
use hl_strategy::{Actions, Context, Strategy, StrategyHost};
use hl_types::{
    Event, MarketEvent, MarketEventKind, OrderSpec, Side, TimeInForce, TimerId, Timestamp,
};
use tungstenite::{Message, WebSocket};

use crate::admission::{Admission, AdmissionSink};
use crate::scheduler::{Pacing, ProcessedRecord};

const LIMIT: Duration = Duration::from_secs(5);
const DEADLINE: Timestamp = Timestamp::from_nanos(2_000_000);

struct Gate {
    stamped: mpsc::Sender<()>,
    release: mpsc::Receiver<()>,
}

struct ObservedSink {
    inner: AdmissionSink,
    admitted: mpsc::Sender<RecordKind>,
    pause_next: Arc<AtomicBool>,
    gate: Gate,
}

impl SourceSink for ObservedSink {
    fn begin(&mut self, source: usize, kind: RecordKind) -> impl FnOnce(Vec<u8>) + '_ {
        let admitted = &self.admitted;
        let admit = self.inner.begin(source, kind);
        if kind == RecordKind::RecvText && self.pause_next.swap(false, Ordering::SeqCst) {
            self.gate.stamped.send(()).expect("test receives stamp");
            self.gate
                .release
                .recv_timeout(LIMIT)
                .expect("test releases reader");
        }
        move |payload| {
            admit(payload);
            admitted.send(kind).expect("test receives admission");
        }
    }
}

struct Source {
    admission: Arc<Admission>,
    config: SourceConfig,
    clock: Arc<Mutex<Stamp>>,
    send: mpsc::Sender<String>,
    admitted: mpsc::Receiver<RecordKind>,
    pause_next: Arc<AtomicBool>,
    stamped: mpsc::Receiver<()>,
    release: mpsc::Sender<()>,
    shutdown: Arc<AtomicBool>,
    client: JoinHandle<()>,
    server: JoinHandle<()>,
}

impl Source {
    fn start(capacity: usize) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback port");
        let address = listener.local_addr().expect("bound address");
        let (send, frames) = mpsc::channel::<String>();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("source connects");
            stream.set_read_timeout(Some(LIMIT)).expect("read bound");
            let mut socket: WebSocket<TcpStream> = tungstenite::accept(stream).expect("handshake");
            assert_eq!(
                socket.read().expect("subscribe").into_text().expect("text"),
                "subscribe"
            );
            for frame in frames {
                socket.send(Message::text(frame)).expect("send fixture");
            }
        });
        let config: SourceConfig = toml::from_str(&format!(
            "name = 'loopback'\nurl = 'ws://{address}/ws'\nstale_after_ms = 60000\nsubscribe_once = ['subscribe']"
        )).expect("source config");
        let clock = Arc::new(Mutex::new(Stamp {
            wall_ns: 0,
            mono_ns: 0,
        }));
        let admission = Arc::new(Admission::with_test_clock(Arc::clone(&clock), capacity));
        let (admitted_tx, admitted) = mpsc::channel();
        let (stamped_tx, stamped) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let pause_next = Arc::new(AtomicBool::new(false));
        let mut sink = ObservedSink {
            inner: admission.sink(),
            admitted: admitted_tx,
            pause_next: Arc::clone(&pause_next),
            gate: Gate {
                stamped: stamped_tx,
                release: release_rx,
            },
        };
        let shutdown = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&shutdown);
        let source_config = config.clone();
        let client =
            thread::spawn(move || hl_recorder::run_source(0, &source_config, &stop, &mut sink));
        let source = Self {
            admission,
            config,
            clock,
            send,
            admitted,
            pause_next,
            stamped,
            release,
            shutdown,
            client,
            server,
        };
        source.wait_for(RecordKind::SentText);
        source
    }

    fn time(&self, wall: i64, mono: u64) {
        *self.clock.lock().expect("test clock") = Stamp {
            wall_ns: wall,
            mono_ns: mono,
        };
    }

    fn wait_for(&self, kind: RecordKind) {
        while self.admitted.recv_timeout(LIMIT).expect("source admission") != kind {}
    }

    fn bbo(&self, millis: i64, bid: &str) {
        self.send.send(format!(r#"{{"channel":"bbo","data":{{"coin":"BTC","time":{millis},"bbo":[{{"px":"{bid}","sz":"1","n":1}},{{"px":"110","sz":"1","n":1}}]}}}}"#)).expect("server running");
    }

    fn trade(&self, millis: i64) {
        self.send.send(format!(r#"{{"channel":"trades","data":[{{"coin":"BTC","side":"A","px":"95","sz":"1","time":{millis},"tid":1,"hash":"0x0000000000000000000000000000000000000000000000000000000000000000","users":["0x0000000000000000000000000000000000000000","0x0000000000000000000000000000000000000000"]}}]}}"#)).expect("server running");
    }

    fn stop(self) -> Vec<Record> {
        self.shutdown.store(true, Ordering::Relaxed);
        self.client.join().expect("source completes");
        drop(self.send);
        self.server.join().expect("server completes");
        self.admission.take_prefix().expect("final prefix").records
    }
}

#[derive(Debug, Default)]
struct Probe {
    seen: Arc<Mutex<Vec<String>>>,
    initialized: bool,
}

impl Strategy for Probe {
    fn on_event(&mut self, ctx: &Context<'_>, event: &Event, actions: &mut Actions) {
        let label = match event {
            Event::Market(market) => match &market.kind {
                MarketEventKind::Bbo(bbo) => format!("bbo {}", bbo.bid.expect("bid").px),
                MarketEventKind::Trade(_) => "trade".to_owned(),
                MarketEventKind::FeedGap => "gap".to_owned(),
                _ => unreachable!(),
            },
            Event::Order(_) => "report".to_owned(),
            Event::Timer(timer) => format!("timer {} at {}", timer.id.0, ctx.now().as_nanos()),
        };
        self.seen.lock().expect("probe log").push(label);
        if matches!(event, Event::Market(market) if matches!(market.kind, MarketEventKind::Bbo(_)))
            && !self.initialized
        {
            self.initialized = true;
            actions.place(OrderSpec {
                instrument: instrument_id(Network::Mainnet, "BTC"),
                side: Side::Buy,
                px: "100".parse().expect("price"),
                qty: "1".parse().expect("size"),
                tif: TimeInForce::Alo,
                reduce_only: false,
            });
            actions.set_timer(DEADLINE, TimerId(9));
            actions.set_timer(DEADLINE, TimerId(3));
        }
    }
}

fn sim() -> SimConfig {
    SimConfig {
        order_latency: Duration::ZERO,
        report_latency: Duration::ZERO,
        maker_fee: hl_types::FeeRate::ZERO,
        min_order_notional: hl_types::Usd::ZERO,
    }
}

fn start() -> SessionStart {
    SessionStart {
        commit: env!("HL_BUILD_COMMIT").to_owned(),
        dirty: env!("HL_BUILD_DIRTY") == "true",
        driver: "paper".to_owned(),
        network: Network::Mainnet,
        source: Some("loopback".to_owned()),
        coin: "BTC".to_owned(),
        instrument: instrument_id(Network::Mainnet, "BTC"),
        process_start_wall_ns: Some(0),
        strategy_name: "test_probe".to_owned(),
        strategy_tag: 1,
        strategy: vec![],
        sim_model: "strict_trade_through".to_owned(),
        sim: sim().parameters(),
    }
}

struct Inputs {
    converter: RecordConverter<'static>,
    market: Vec<MarketEvent>,
    origins: Vec<EventOrigin>,
    records: Vec<Record>,
}

impl Inputs {
    fn new() -> Self {
        let mut converter = RecordConverter::new(Network::Mainnet, "BTC");
        converter
            .file_start(0, 0, Some(0), false, &mut Vec::new())
            .expect("header");
        Self {
            converter,
            market: vec![],
            origins: vec![],
            records: vec![],
        }
    }

    fn process<S: Strategy>(
        &mut self,
        driver: &mut IncrementalDriver<'_, S>,
        record: Record,
    ) -> std::io::Result<ProcessedRecord> {
        let mut events = Vec::new();
        self.converter
            .record(&record, &mut events)
            .expect("convert admitted record");
        let origins = self.converter.take_origins();
        let latest_input = events.last().map(|event| event.ts_local);
        let shutdown = origins
            .iter()
            .any(|origin| origin.gap_cause == Some(hl_run::recording::GapCause::Shutdown));
        for (event, origin) in events.iter().zip(&origins) {
            driver
                .on_market_event_with_origin(event, origin)
                .expect("admitted time remains open");
        }
        self.records.push(record);
        self.market.extend(events);
        self.origins.extend(origins);
        Ok(ProcessedRecord {
            latest_input,
            shutdown,
        })
    }

    fn handoff<S: Strategy>(
        &mut self,
        source: &Source,
        pacing: &mut Pacing,
        driver: &mut IncrementalDriver<'_, S>,
    ) {
        let prefix = source.admission.take_prefix().expect("prefix");
        let stopped = pacing
            .process_records(prefix.records, driver, |driver, record| {
                self.process(driver, record)
            })
            .expect("process whole prefix");
        if !stopped {
            let sample = pacing.anchored(prefix.sample).expect("anchored sample");
            pacing.close(driver, sample).expect("close complete prefix");
        }
    }
}

#[test]
fn loopback_paused_stamp_holds_admission_until_handoff() {
    let source = Source::start(32);
    let mut inputs = Inputs::new();
    let mut host = StrategyHost::new(Probe::default(), 1);
    let seen = Arc::clone(&host.strategy().seen);
    let mut gateway = SimGateway::new(sim());
    let mut session = Session::new(&start());
    let mut driver = IncrementalDriver::with_session(&mut host, &mut gateway, &mut session);
    let mut pacing = Pacing::new(0);
    source.time(1_000_000, 1_000_000);
    source.bbo(1, "101");
    source.wait_for(RecordKind::RecvText);
    inputs.handoff(&source, &mut pacing, &mut driver);
    source.time(2_000_000, 2_000_000);
    source.pause_next.store(true, Ordering::SeqCst);
    source.bbo(2, "102");
    source
        .stamped
        .recv_timeout(LIMIT)
        .expect("reader paused after stamp");
    source.time(3_000_000, 3_000_000);
    assert!(
        source
            .admission
            .try_take_prefix()
            .expect("unpoisoned")
            .is_none(),
        "scheduler cannot sample while stamped frame is held"
    );
    assert_eq!(*seen.lock().expect("log"), ["bbo 101"]);
    source.release.send(()).expect("release stamp");
    source.wait_for(RecordKind::RecvText);
    inputs.handoff(&source, &mut pacing, &mut driver);
    assert_eq!(
        *seen.lock().expect("log"),
        [
            "bbo 101",
            "bbo 102",
            "report",
            "timer 9 at 2000000",
            "timer 3 at 2000000"
        ]
    );
    source.stop();
}

#[test]
fn loopback_equal_stamps_keep_admission_order_before_reports_and_timers() {
    let source = Source::start(32);
    let mut inputs = Inputs::new();
    let mut host = StrategyHost::new(Probe::default(), 1);
    let seen = Arc::clone(&host.strategy().seen);
    let mut gateway = SimGateway::new(sim());
    let mut session = Session::new(&start());
    let mut driver = IncrementalDriver::with_session(&mut host, &mut gateway, &mut session);
    let mut pacing = Pacing::new(0);
    source.time(1_000_000, 1_000_000);
    source.bbo(1, "101");
    source.wait_for(RecordKind::RecvText);
    inputs.handoff(&source, &mut pacing, &mut driver);
    source.time(2_000_000, 2_000_000);
    source.bbo(2, "102");
    source.wait_for(RecordKind::RecvText);
    source.trade(2);
    source.wait_for(RecordKind::RecvText);
    inputs.handoff(&source, &mut pacing, &mut driver);
    assert_eq!(
        *seen.lock().expect("log"),
        ["bbo 101", "bbo 102", "trade"],
        "a sample equal to T must leave T open"
    );
    source.time(2_000_001, 2_000_001);
    inputs.handoff(&source, &mut pacing, &mut driver);
    assert_eq!(
        *seen.lock().expect("log"),
        [
            "bbo 101",
            "bbo 102",
            "trade",
            "report",
            "report",
            "timer 9 at 2000000",
            "timer 3 at 2000000"
        ]
    );
    source.stop();
}

#[test]
fn loopback_quiet_timer_waits_for_strict_sample_and_replays_exact_rows() {
    let source = Source::start(32);
    let source_config = source.config.clone();
    let mut inputs = Inputs::new();
    let probe = Probe::default();
    let seen = Arc::clone(&probe.seen);
    let observation = std::cell::RefCell::new(crate::observe::Observation::default());
    let observed = crate::observe::Observed {
        strategy: probe,
        sim: sim(),
        clock: crate::admission::PaperClock::Real(hl_recorder::Clock::start()),
        observation: &observation,
    };
    let mut host = StrategyHost::new(observed, 1);
    let mut gateway = SimGateway::new(sim());
    let mut session = Session::new(&start());
    let mut driver = IncrementalDriver::with_session(&mut host, &mut gateway, &mut session);
    let mut pacing = Pacing::new(0);
    source.time(1_000_000, 1_000_000);
    source.bbo(1, "101");
    source.wait_for(RecordKind::RecvText);
    inputs.handoff(&source, &mut pacing, &mut driver);
    source.time(1_000_001, 1_000_001);
    inputs.handoff(&source, &mut pacing, &mut driver);
    let wake = observation
        .borrow_mut()
        .next_wakeup(pacing.closed_through());
    assert_eq!(wake, Some(DEADLINE));
    let remaining = Duration::from_secs(1);
    let stamp = *source.clock.lock().expect("test clock");
    assert_eq!(
        pacing
            .wait_duration(stamp, wake, remaining)
            .expect("deadline sleep"),
        Duration::from_nanos(1_000_000)
    );
    // Even a forward wall step cannot close a monotonic deadline.
    source.time(9_000_000, 2_000_000);
    inputs.handoff(&source, &mut pacing, &mut driver);
    assert_eq!(*seen.lock().expect("log"), ["bbo 101"]);
    let stamp = *source.clock.lock().expect("test clock");
    let wait = pacing
        .wait_duration(stamp, wake, remaining)
        .expect("deadline resample");
    assert_eq!(wait, Duration::from_nanos(1));
    source
        .admission
        .wait(wait)
        .expect("empty queue wakes on deadline");
    source.time(2_000_001, 2_000_001);
    inputs.handoff(&source, &mut pacing, &mut driver);
    assert_eq!(
        *seen.lock().expect("log"),
        ["bbo 101", "timer 9 at 2000000", "timer 3 at 2000000"]
    );
    source.time(3_000_000, 3_000_000);
    for record in source.stop() {
        inputs.process(&mut driver, record).expect("shutdown input");
    }
    let stop = inputs.converter.stop_time().expect("recorded stop");
    let trace = driver.finish(stop).expect("finish");
    let mut replay_host = StrategyHost::new(Probe::default(), 1);
    let mut replay_sim = SimGateway::new(sim());
    let mut replay_session = Session::new(&start());
    replay_with_session(
        &mut replay_host,
        &mut replay_sim,
        &inputs.market,
        &inputs.origins,
        stop,
        &mut replay_session,
    )
    .expect("offline replay");
    assert_eq!(
        session.lines(),
        replay_session.lines(),
        "quiet timer rows and logical times are reproducible"
    );
    assert_eq!(
        session
            .lines()
            .iter()
            .filter(|line| line.contains("\"timer_fired\""))
            .count(),
        2
    );
    save_fixture(
        "HL_TEST_TIMER_SESSION",
        &source_config,
        &inputs,
        &mut session,
        trace,
        host.orders().len(),
    );
}

#[test]
fn loopback_full_queue_preserves_lifecycle_and_publishes_losses_in_order() {
    let source = Source::start(1);
    let source_config = source.config.clone();
    let mut records = source
        .admission
        .take_prefix()
        .expect("startup prefix")
        .records;
    source.time(1_000_000, 1_000_000);
    source.bbo(1, "101");
    source.wait_for(RecordKind::RecvText);
    source.bbo(1, "102");
    source.wait_for(RecordKind::RecvText);
    source.trade(1);
    source.wait_for(RecordKind::RecvText);
    // Meta has no data-slot cost and must retain its position while full.
    let mut marker_sink = source.admission.sink();
    marker_sink.begin(0, RecordKind::Meta)(br#"{"event":"disconnected"}"#.to_vec());
    let prefix = source.admission.take_prefix().expect("full prefix");
    assert_eq!(prefix.records.len(), 2);
    assert_eq!(prefix.records[0].kind, RecordKind::RecvText);
    assert_eq!(prefix.records[1].payload, br#"{"event":"disconnected"}"#);
    records.extend(prefix.records);
    source.time(2_000_000, 2_000_000);
    source.bbo(2, "103");
    source.wait_for(RecordKind::RecvText);
    let prefix = source
        .admission
        .take_prefix()
        .expect("accepted data prefix");
    assert_eq!(prefix.records.len(), 2);
    let dropped: serde_json::Value =
        serde_json::from_slice(&prefix.records[0].payload).expect("drop JSON");
    assert_eq!(dropped, serde_json::json!({"event":"dropped","count":2}));
    assert_eq!(prefix.records[1].kind, RecordKind::RecvText);
    records.extend(prefix.records);
    source.time(3_000_000, 3_000_000);
    source.bbo(3, "104");
    source.wait_for(RecordKind::RecvText);
    source.bbo(3, "105");
    source.wait_for(RecordKind::RecvText);
    let final_prefix = source.stop();
    assert_eq!(final_prefix.len(), 3);
    assert_eq!(final_prefix[0].kind, RecordKind::RecvText);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&final_prefix[1].payload).expect("drop JSON"),
        serde_json::json!({"event":"dropped","count":1})
    );
    assert_eq!(final_prefix[2].payload, br#"{"event":"shutdown"}"#);
    records.extend(final_prefix);
    let mut inputs = Inputs::new();
    let mut host = StrategyHost::new(Probe::default(), 1);
    let mut gateway = SimGateway::new(sim());
    let mut session = Session::new(&start());
    let mut driver = IncrementalDriver::with_session(&mut host, &mut gateway, &mut session);
    for record in records {
        inputs
            .process(&mut driver, record)
            .expect("same admitted stream");
    }
    let stop = inputs.converter.stop_time().expect("stop");
    let trace = driver.finish(stop).expect("finish");
    assert_eq!(inputs.converter.stats().gaps.dropped, 2);
    assert_eq!(inputs.converter.stats().gaps.disconnected, 1);
    assert_eq!(inputs.converter.stats().gaps.shutdown, 1);
    let mut replay_host = StrategyHost::new(Probe::default(), 1);
    let mut replay_gateway = SimGateway::new(sim());
    let mut replay_session = Session::new(&start());
    replay_with_session(
        &mut replay_host,
        &mut replay_gateway,
        &inputs.market,
        &inputs.origins,
        stop,
        &mut replay_session,
    )
    .expect("replay admitted stream");
    assert_eq!(session.lines(), replay_session.lines());
    save_fixture(
        "HL_TEST_DROP_SESSION",
        &source_config,
        &inputs,
        &mut session,
        trace,
        host.orders().len(),
    );
}

fn save_fixture(
    variable: &str,
    source: &SourceConfig,
    inputs: &Inputs,
    session: &mut Session,
    trace: hl_run::ReplayTrace,
    open_orders: usize,
) {
    let Some(directory) = std::env::var_os(variable) else {
        return;
    };
    let directory = std::path::Path::new(&directory);
    std::fs::create_dir_all(directory).expect("fixture directory");
    let identity = start();
    let mut capture =
        crate::output::Capture::new(directory, source, 1, &identity).expect("fixture capture");
    let mut actual_events = Vec::new();
    let mut actual_origins = Vec::new();
    for record in &inputs.records {
        let mut events = Vec::new();
        actual_origins.extend(
            capture
                .record(record, &mut events)
                .expect("write admitted raw"),
        );
        actual_events.extend(events);
    }
    assert_eq!(actual_events, inputs.market);
    assert_eq!(actual_origins, inputs.origins);
    capture
        .seal(session, trace, open_orders)
        .expect("seal loopback session");
}

#[test]
fn loopback_forward_wall_input_closes_only_on_later_input_while_anchor_lags() {
    let source = Source::start(32);
    let mut inputs = Inputs::new();
    let mut host = StrategyHost::new(Probe::default(), 1);
    let seen = Arc::clone(&host.strategy().seen);
    let mut gateway = SimGateway::new(sim());
    let mut driver = IncrementalDriver::new(&mut host, &mut gateway);
    let mut pacing = Pacing::new(0);
    source.time(2_000_000, 100);
    source.bbo(1, "101");
    source.wait_for(RecordKind::RecvText);
    inputs.handoff(&source, &mut pacing, &mut driver);
    // The wall jumps back. previous_R keeps the second callback tied at T.
    source.time(400, 200);
    source.bbo(1, "102");
    source.wait_for(RecordKind::RecvText);
    inputs.handoff(&source, &mut pacing, &mut driver);
    assert_eq!(*seen.lock().expect("log"), ["bbo 101", "bbo 102"]);
    // Exchange time moves the next input forward and closes T even though
    // neither clock has caught up. The timers precede this third BBO.
    source.bbo(3, "103");
    source.wait_for(RecordKind::RecvText);
    inputs.handoff(&source, &mut pacing, &mut driver);
    assert_eq!(
        *seen.lock().expect("log"),
        [
            "bbo 101",
            "bbo 102",
            "timer 9 at 2000000",
            "timer 3 at 2000000",
            "bbo 103"
        ]
    );
    source.stop();
}
