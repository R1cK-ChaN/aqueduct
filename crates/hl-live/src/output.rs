//! Raw rotation, matching converter positions, and publication of the session.

use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use hl_codec::rawlog::Record;
use hl_recorder::{LogWriter, SourceConfig, SourceInfo, Stamp};
use hl_run::pnl::{BboMark, summarize_marks};
use hl_run::recording::{EventOrigin, RawPosition, RecordConverter};
use hl_run::session::{
    CheckpointBoundary, ManifestEntry, Session, SessionEnd, SessionStart, checkpoint_line,
    next_utc_midnight,
};
use hl_run::{IncrementalDriver, ReplayTrace};
use hl_strategy::Strategy;
use hl_types::{MarketEvent, Timestamp};
use sha2::{Digest, Sha256};

pub(crate) fn create_directory(parent: &Path, anchor: i64) -> io::Result<PathBuf> {
    fs::create_dir_all(parent)?;
    for collision in 0_u64.. {
        let path = parent.join(format!("{anchor:019}-{collision:04}"));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    unreachable!("exhausted session directory names")
}

/// Last eligible raw position at successive midnights. A control record's
/// wall stamp can jump forward and back before a market input or the anchored
/// clock closes the day. Keep only positions that can still define an extent.
#[derive(Default)]
struct RawExtents {
    previous: Option<RawPosition>,
    by_midnight: VecDeque<(Timestamp, RawPosition)>,
}

impl RawExtents {
    fn observe(&mut self, time: Timestamp, raw: RawPosition) {
        let Some(midnight) = next_utc_midnight(time) else {
            return;
        };
        // This later position supersedes all earlier positions first eligible
        // at this midnight or later.
        while self
            .by_midnight
            .back()
            .is_some_and(|&(at, _)| at >= midnight)
        {
            self.by_midnight.pop_back();
        }
        self.by_midnight.push_back((midnight, raw));
    }

    fn through(&mut self, midnight: Timestamp) -> Option<RawPosition> {
        while let Some(&(at, position)) = self.by_midnight.front() {
            if at > midnight {
                break;
            }
            self.previous = Some(position);
            self.by_midnight.pop_front();
        }
        self.previous
    }
}

pub(crate) struct Capture<'a> {
    dir: PathBuf,
    start: &'a SessionStart,
    raw: LogWriter,
    converter: RecordConverter<'a>,
    session: BufWriter<File>,
    manifest: Vec<ManifestEntry>,
    needs_header: bool,
    marks: Vec<BboMark>,
    next_checkpoint: Option<Timestamp>,
    extents: RawExtents,
    pending: VecDeque<(CheckpointBoundary, Option<RawPosition>)>,
}

impl<'a> Capture<'a> {
    pub(crate) fn new(
        dir: &Path,
        source: &SourceConfig,
        zstd_level: i32,
        start: &'a SessionStart,
    ) -> io::Result<Self> {
        let anchor = start
            .process_start_wall_ns
            .ok_or_else(|| io::Error::other("missing paper process anchor"))?;
        let info = SourceInfo {
            name: source.name.clone(),
            url: source.url.clone(),
        };
        let (raw, _) = LogWriter::new(dir, zstd_level, anchor, vec![info])?;
        Ok(Self {
            dir: dir.to_owned(),
            start,
            raw,
            session: BufWriter::new(File::create_new(dir.join("session.jsonl.part"))?),
            converter: RecordConverter::new(start.network, &start.coin),
            manifest: Vec::new(),
            needs_header: true,
            marks: Vec::new(),
            next_checkpoint: next_utc_midnight(Timestamp::from_nanos(anchor)),
            extents: RawExtents::default(),
            pending: VecDeque::new(),
        })
    }

    pub(crate) fn record(
        &mut self,
        record: &Record,
        events: &mut Vec<MarketEvent>,
    ) -> io::Result<Vec<EventOrigin>> {
        let stamp = Stamp {
            wall_ns: record.recv_wall_ns,
            mono_ns: record.recv_mono_ns,
        };
        self.raw.write(0, record.kind, stamp, &record.payload)?;
        self.collect_sealed()?;
        // A write opens a file on demand, including after an idle tick sealed
        // the previous hour. Use the writer's actual rotation notifications.
        let before_header = events.len();
        if self.needs_header {
            self.converter
                .file_start(
                    stamp.wall_ns,
                    stamp.mono_ns,
                    self.start.process_start_wall_ns,
                    false,
                    events,
                )
                .map_err(io::Error::other)?;
            self.note_position(&events[before_header..]);
            self.needs_header = false;
        }
        let before_record = events.len();
        self.converter
            .record(record, events)
            .map_err(io::Error::other)?;
        self.note_position(&events[before_record..]);
        self.marks.extend(
            events[before_record..]
                .iter()
                .filter_map(|event| BboMark::from_event(self.start.instrument, event)),
        );
        Ok(self.converter.take_origins())
    }

    fn note_position(&mut self, events: &[MarketEvent]) {
        let raw = self.converter.last_position().expect("complete raw record");
        let time = self
            .converter
            .checkpoint_record_time(events)
            .expect("record has a clock");
        self.extents.observe(time, raw);
    }

    /// Capture the state before a later input, or after draining the entire
    /// prefix from an admission sample. The writer can seal the extent later.
    pub(crate) fn capture_due_checkpoints<S: Strategy>(
        &mut self,
        driver: &mut IncrementalDriver<'_, S>,
        time: Timestamp,
    ) -> io::Result<()> {
        while let Some(cutoff) = self.next_checkpoint.filter(|&cutoff| cutoff <= time) {
            let boundary = driver.checkpoint_before(cutoff).map_err(io::Error::other)?;
            self.append(driver.take_session_lines())?;
            self.pending
                .push_back((boundary, self.extents.through(cutoff)));
            self.next_checkpoint = next_utc_midnight(cutoff);
            self.publish_checkpoints()?;
        }
        Ok(())
    }

    fn collect_sealed(&mut self) -> io::Result<()> {
        for path in self.raw.take_sealed() {
            self.needs_header = true;
            let relative = path.strip_prefix(&self.dir).map_err(io::Error::other)?;
            self.manifest.push(ManifestEntry {
                path: relative
                    .to_str()
                    .ok_or_else(|| io::Error::other("non-UTF-8 manifest path"))?
                    .into(),
                file_seq: Some(self.manifest.len() as u64),
                sha256: hash_file(&path)?,
            });
        }
        self.publish_checkpoints()
    }

    fn publish_checkpoints(&mut self) -> io::Result<()> {
        while let Some(&(boundary, extent)) = self.pending.front() {
            let len = extent.map_or(0, |raw| raw.file_index + 1);
            let Some(manifest) = self.manifest.get(..len) else {
                break;
            };
            self.session
                .write_all(checkpoint_line(boundary, extent, manifest).as_bytes())?;
            self.session.flush()?;
            self.session.get_ref().sync_all()?;
            self.pending.pop_front();
        }
        Ok(())
    }

    pub(crate) fn stop(&self) -> io::Result<Timestamp> {
        self.converter
            .stop_time()
            .ok_or_else(|| io::Error::other("missing paper stop"))
    }

    pub(crate) fn append(&mut self, lines: impl IntoIterator<Item = String>) -> io::Result<()> {
        for line in lines {
            self.session.write_all(line.as_bytes())?;
        }
        Ok(())
    }

    pub(crate) fn tick(&mut self, wall_ns: i64) -> io::Result<()> {
        self.raw.tick(wall_ns)?;
        self.collect_sealed()?;
        self.session.flush()
    }

    pub(crate) fn seal(
        mut self,
        session: &mut Session,
        trace: ReplayTrace,
        open_orders: usize,
    ) -> io::Result<()> {
        self.raw.finish()?;
        self.collect_sealed()?;
        if !self.pending.is_empty() {
            return Err(io::Error::other(
                "checkpoint extent missing from sealed manifest",
            ));
        }
        let pnl = summarize_marks(self.start.instrument, &self.marks, &trace.order_events)
            .map_err(io::Error::other)?;
        session.finish(&SessionEnd {
            stop: self.stop()?,
            last_raw: self.converter.last_position(),
            manifest: std::mem::take(&mut self.manifest),
            pnl,
            open_orders,
            pending: trace.pending_at_end,
            late_print_misses: trace.late_print_misses,
        });
        self.append(session.take_lines())?;
        self.session.flush()?;
        self.session.get_ref().sync_all()?;
        fs::rename(
            self.dir.join("session.jsonl.part"),
            self.dir.join("session.jsonl"),
        )?;
        File::open(&self.dir)?.sync_all()
    }
}

fn hash_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(data_encoding::HEXLOWER.encode(&hash.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hl_codec::hyperliquid::{Network, instrument_id};
    use hl_recorder::{RecordKind, open_recording};
    use hl_run::IncrementalDriver;
    use hl_sim::{SimConfig, SimGateway};
    use hl_strategy::{Actions, Context, Strategy, StrategyHost};
    use hl_types::{Event, FeeRate, Usd};
    use serde_json::{Value, json};
    use std::time::Duration;

    struct NoActions;

    impl Strategy for NoActions {
        fn on_event(&mut self, _ctx: &Context<'_>, _event: &Event, _actions: &mut Actions) {}
    }

    #[test]
    fn raw_extents_follow_position_when_control_times_step_back_across_midnight() {
        const DAY: i64 = 86_400_000_000_000;
        let mut extents = RawExtents::default();
        let position = |record_index| RawPosition {
            file_index: 0,
            record_index,
        };
        // Record 1 jumps ahead two dates. A later control record at DAY-1
        // belongs to the first checkpoint, and a control at DAY belongs to the second.
        for (index, time) in [DAY - 2, 2 * DAY + 1, DAY - 1, 2 * DAY + 2, DAY, 2 * DAY + 3]
            .into_iter()
            .enumerate()
        {
            extents.observe(Timestamp::from_nanos(time), position(index as u64));
        }
        for (day, last) in [(1, 2), (2, 4), (3, 5), (4, 5)] {
            assert_eq!(
                extents.through(Timestamp::from_nanos(day * DAY)),
                Some(position(last))
            );
        }
    }

    #[test]
    fn gap_positions_follow_sealed_files_across_rotation_idle_tick_and_clock_rollback() {
        const HOUR: i64 = 3_600_000_000_000;
        let source: SourceConfig =
            toml::from_str("name='loopback'\nurl='ws://127.0.0.1'\n").expect("source");
        let sim_config = SimConfig {
            order_latency: Duration::ZERO,
            report_latency: Duration::ZERO,
            maker_fee: FeeRate::ZERO,
            min_order_notional: Usd::ZERO,
        };
        let start = SessionStart {
            driver: "paper".into(),
            commit: env!("HL_BUILD_COMMIT").into(),
            dirty: env!("HL_BUILD_DIRTY") == "true",
            network: Network::Testnet,
            source: Some(source.name.clone()),
            coin: "BTC".into(),
            instrument: instrument_id(Network::Testnet, "BTC"),
            process_start_wall_ns: Some(0),
            strategy_name: "no_actions".into(),
            strategy_tag: 1,
            strategy: vec![],
            sim_model: hl_sim::MODEL_NAME.into(),
            sim: sim_config.parameters(),
            rules: Vec::new(),
        };
        let temp = std::env::temp_dir().join(format!("hl-live-rotation-{}", std::process::id()));
        let dir = create_directory(&temp, 0).expect("new session directory");
        let mut capture = Capture::new(&dir, &source, 1, &start).expect("capture");
        let mut session = Session::new(&start);
        let mut host = StrategyHost::new(NoActions, 1);
        let mut sim = SimGateway::new(sim_config);
        let mut driver = IncrementalDriver::with_session(&mut host, &mut sim, &mut session);
        let records = [
            (HOUR - 1, "connected"),
            (HOUR - 1, "dropped"),
            (HOUR + 10, "disconnected"), // Write rotates at the hour boundary.
            (HOUR + 5, "dropped"),       // Idle tick sealed the previous file.
            (HOUR - 2, "shutdown"),      // The wall clock re-enters the first hour.
        ];
        for (index, (wall, event)) in records.into_iter().enumerate() {
            if index == 3 {
                capture.tick(2 * HOUR).expect("seal while idle");
            }
            let mut events = Vec::new();
            let mut payload = json!({"event": event});
            if event == "dropped" {
                payload["count"] = json!(1);
            }
            let record = Record {
                kind: RecordKind::Meta,
                recv_wall_ns: wall,
                recv_mono_ns: index as u64,
                payload: payload.to_string().into_bytes(),
            };
            let origins = capture
                .record(&record, &mut events)
                .expect("write and convert");
            for (event, origin) in events.iter().zip(&origins) {
                driver
                    .on_market_event_with_origin(event, origin)
                    .expect("process gap");
            }
        }
        let stop = capture.stop().expect("stop");
        assert_eq!(stop, Timestamp::from_nanos(HOUR + 10));
        let trace = driver.finish(stop).expect("finish driver");
        capture
            .seal(&mut session, trace, host.orders().len())
            .expect("sealed capture");

        let session = fs::read_to_string(dir.join("session.jsonl")).expect("sealed session");
        let rows: Vec<Value> = session
            .lines()
            .map(|line| serde_json::from_str(line).expect("session row"))
            .collect();
        let end = rows.last().expect("footer");
        assert_eq!(
            end["last_raw"],
            json!({"file_index":"3", "record_index":"1"})
        );
        let manifest = end["manifest"].as_array().expect("manifest");
        assert_eq!(manifest.len(), 4);
        let expected_gaps = [
            (0, 2, "dropped"),
            (1, 1, "disconnected"),
            (2, 1, "dropped"),
            (3, 1, "shutdown"),
        ];
        let gaps: Vec<_> = rows.iter().filter(|row| row["kind"] == "gap").collect();
        assert_eq!(gaps.len(), expected_gaps.len());
        for ((index, record_index, cause), gap) in expected_gaps.into_iter().zip(gaps) {
            let entry = &manifest[index];
            assert_eq!(entry["file_seq"], index.to_string());
            let relative = Path::new(entry["path"].as_str().expect("manifest path"));
            assert!(relative.is_relative());
            let path = dir.join(relative);
            let stored = fs::read(&path).expect("stored compressed bytes");
            assert_eq!(
                entry["sha256"],
                data_encoding::HEXLOWER.encode(&Sha256::digest(stored))
            );
            let raw: Vec<Record> = open_recording(&path)
                .expect("open sealed raw")
                .collect::<Result<_, _>>()
                .expect("complete raw file");
            let header: Value = serde_json::from_slice(&raw[0].payload).expect("header");
            assert_eq!(header["file_seq"], index);
            assert_eq!(gap["cause"], cause);
            assert_eq!(
                gap["raw"],
                json!({"file_index": index.to_string(), "record_index": record_index.to_string()})
            );
            let raw_gap: Value =
                serde_json::from_slice(&raw[record_index].payload).expect("lifecycle payload");
            assert_eq!(raw_gap["event"], cause);
        }
        fs::remove_dir_all(dir).expect("remove fixture");
        fs::remove_dir(temp).expect("remove empty fixture parent");
    }
}
