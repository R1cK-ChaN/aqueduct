//! Parse and independently check the session format in `docs/session-file.md`.

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};

use hl_codec::hyperliquid::{Network, instrument_id};
use hl_run::IncrementalDriver;
use hl_run::session::{ManifestEntry, Session, SessionStart, checkpoint_line, next_utc_midnight};
use hl_sim::SimGateway;
use hl_strategy::{Strategy, StrategyHost};
use hl_types::{InstrumentId, Timestamp};
use serde_json::{Value, json};

use crate::recording::{PaperIdentity, Recording, read_recordings_from, sha256_file_stamped};

/// The source tree from which the checking binary was built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildIdentity {
    /// Full Git commit hash, or `unknown` when Git was unavailable.
    pub commit: String,
    /// Whether any tracked file differed from that commit.
    pub dirty: bool,
}

/// Fresh components reconstructed from the session's declared parameters.
#[derive(Debug)]
pub struct ReplayComponents<S> {
    /// Empty strategy host, using the recorded strategy tag.
    pub host: StrategyHost<S>,
    /// Empty simulated venue.
    pub sim: SimGateway,
    /// Canonical parameter list returned by the reconstructed strategy config.
    pub strategy_parameters: Vec<(String, String)>,
    /// Canonical parameter list returned by the reconstructed simulator config.
    pub sim_parameters: Vec<(String, String)>,
}

/// The result of comparing every non-diagnostic row successfully.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckSummary {
    /// Number of compared rows, including the start and every checked boundary.
    pub rows: usize,
    /// Latest exclusive cutoff for an open session.
    pub checkpoint: Option<Timestamp>,
}

/// An invalid session, unavailable input, or first replay difference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionError(String);

impl fmt::Display for SessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for SessionError {}

impl From<String> for SessionError {
    fn from(message: String) -> Self {
        Self(message)
    }
}

struct RecordedCheckpoint<'a> {
    line: &'a str,
    cutoff: Timestamp,
    manifest: Vec<ManifestEntry>,
}

struct ParsedSession<'a> {
    start: SessionStart,
    // Start, compared work, and (when present) end, in append order.
    rows: Vec<&'a str>,
    checkpoints: Vec<RecordedCheckpoint<'a>>,
    // The end's manifest and its claimed inclusive stop.
    end: Option<(Vec<ManifestEntry>, Value)>,
}

impl ParsedSession<'_> {
    fn manifest(&self) -> &[ManifestEntry] {
        match &self.end {
            Some((manifest, _)) => manifest,
            None => {
                &self
                    .checkpoints
                    .last()
                    .expect("open session has a checkpoint")
                    .manifest
            }
        }
    }
}

fn difference(
    location: &str,
    field: &str,
    expected: Option<&Value>,
    replayed: Option<&Value>,
) -> SessionError {
    let describe =
        |value: Option<&Value>| value.map_or_else(|| "<missing>".to_owned(), Value::to_string);
    SessionError(format!(
        "{location} field {field}: expected {}, replayed {}",
        describe(expected),
        describe(replayed)
    ))
}

fn invalid(location: &str, field: &str, requirement: &str) -> SessionError {
    SessionError(format!("{location} field {field}: {requirement}"))
}

fn field<'a>(row: &'a Value, name: &str, location: &str) -> Result<&'a Value, SessionError> {
    row.get(name)
        .ok_or_else(|| invalid(location, name, "missing required field"))
}

fn string<'a>(value: &'a Value, location: &str, name: &str) -> Result<&'a str, SessionError> {
    value
        .as_str()
        .ok_or_else(|| invalid(location, name, "expected a string"))
}

fn string_field<'a>(row: &'a Value, name: &str, location: &str) -> Result<&'a str, SessionError> {
    string(field(row, name, location)?, location, name)
}

fn integer<T>(value: &Value, location: &str, name: &str) -> Result<T, SessionError>
where
    T: std::str::FromStr + ToString,
{
    let text = string(value, location, name)?;
    let parsed = text
        .parse::<T>()
        .map_err(|_| invalid(location, name, "expected an integer string in range"))?;
    if parsed.to_string() != text {
        return Err(invalid(
            location,
            name,
            "expected a canonical integer string",
        ));
    }
    Ok(parsed)
}

fn parameters(
    row: &Value,
    order: &JsonOrder,
    name: &str,
    exclude: Option<&str>,
) -> Result<Vec<(String, String)>, SessionError> {
    let object = field(row, name, "start")?
        .as_object()
        .ok_or_else(|| invalid("start", name, "expected an object"))?;
    let JsonOrder::Object(fields) = order else {
        return Err(invalid("start", name, "expected an object"));
    };
    let (_, JsonOrder::Object(parameter_order)) = fields
        .iter()
        .find(|(key, _)| key == name)
        .ok_or_else(|| invalid("start", name, "missing required field"))?
    else {
        return Err(invalid("start", name, "expected an object"));
    };
    parameter_order
        .iter()
        .filter(|(key, _)| Some(key.as_str()) != exclude)
        .map(|(key, _)| {
            Ok((
                key.clone(),
                string(&object[key], "start", &format!("{name}.{key}"))?.to_owned(),
            ))
        })
        .collect()
}

fn parse_start(row: &Value, order: &JsonOrder) -> Result<SessionStart, SessionError> {
    if field(row, "schema_version", "start")?.as_u64() != Some(1) {
        return Err(invalid("start", "schema_version", "expected version 1"));
    }
    let network = Network::from_short_name(string_field(row, "network", "start")?)
        .ok_or_else(|| invalid("start", "network", "unknown network"))?;
    let source = match field(row, "source", "start")? {
        Value::Null => None,
        value => Some(string(value, "start", "source")?.to_owned()),
    };
    let process_start_wall_ns = match field(row, "process_start_wall_ns", "start")? {
        Value::Null => None,
        value => Some(integer(value, "start", "process_start_wall_ns")?),
    };
    Ok(SessionStart {
        commit: string_field(row, "commit", "start")?.to_owned(),
        dirty: field(row, "dirty", "start")?
            .as_bool()
            .ok_or_else(|| invalid("start", "dirty", "expected a boolean"))?,
        driver: string_field(row, "driver", "start")?.to_owned(),
        network,
        source,
        coin: string_field(row, "coin", "start")?.to_owned(),
        instrument: InstrumentId::new(integer(
            field(row, "instrument", "start")?,
            "start",
            "instrument",
        )?),
        process_start_wall_ns,
        strategy_name: string_field(row, "strategy_name", "start")?.to_owned(),
        strategy_tag: integer(
            field(row, "strategy_tag", "start")?,
            "start",
            "strategy_tag",
        )?,
        strategy: parameters(row, order, "strategy", None)?,
        sim_model: string_field(field(row, "sim", "start")?, "model", "start.sim")?.to_owned(),
        sim: parameters(row, order, "sim", Some("model"))?,
    })
}

fn parse_manifest(row: &Value, location: &str) -> Result<Vec<ManifestEntry>, SessionError> {
    let entries = field(row, "manifest", location)?
        .as_array()
        .ok_or_else(|| invalid(location, "manifest", "expected an array"))?;
    if location == "end" && entries.is_empty() {
        return Err(invalid(location, "manifest", "expected a nonempty array"));
    }
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let location = format!("{location}.manifest[{index}]");
            let path = string_field(entry, "path", &location)?;
            if path.is_empty() {
                return Err(invalid(&location, "path", "expected a nonempty path"));
            }
            let sha256 = string_field(entry, "sha256", &location)?;
            if sha256.len() != 64
                || !sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(invalid(
                    &location,
                    "sha256",
                    "expected 64 lowercase hexadecimal digits",
                ));
            }
            let file_seq = match field(entry, "file_seq", &location)? {
                Value::Null => None,
                value => Some(integer(value, &location, "file_seq")?),
            };
            Ok(ManifestEntry {
                path: path.to_owned(),
                file_seq,
                sha256: sha256.to_owned(),
            })
        })
        .collect()
}

// Locate the last published boundary before decoding the prefix. A running
// writer can leave arbitrary bytes after it, including a split UTF-8 scalar.
fn checked_lines(bytes: &[u8]) -> Result<(Vec<&[u8]>, Option<Timestamp>), SessionError> {
    let mut lines: Vec<_> = bytes.split_inclusive(|&b| b == b'\n').collect();
    for index in (0..lines.len()).rev() {
        let line = lines[index];
        let Ok(value) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        match value.get("kind").and_then(Value::as_str) {
            Some("end") if line.ends_with(b"\n") => {
                if index + 1 != lines.len() {
                    return Err(invalid("end", "kind", "end must be the final row"));
                }
                return Ok((lines, None));
            }
            Some("checkpoint") if line.ends_with(b"\n") => {
                let cutoff = integer(
                    field(&value, "cutoff_ns", "checkpoint")?,
                    "checkpoint",
                    "cutoff_ns",
                )?;
                lines.truncate(index + 1);
                return Ok((lines, Some(Timestamp::from_nanos(cutoff))));
            }
            _ => {}
        }
    }
    // A backtest always requires a footer. Preserve the encoding error for a
    // complete final JSON object that is missing only its newline.
    if let Some(line) = lines.last()
        && serde_json::from_slice::<Value>(line)
            .ok()
            .is_some_and(|row| row["kind"] == "end")
    {
        return Err(SessionError(
            "end encoding differs: missing terminating newline".into(),
        ));
    }
    Err(SessionError(
        "incomplete session: no complete end or checkpoint; nothing to check yet".into(),
    ))
}

fn parse_session<'a>(path: &Path, bytes: &'a [u8]) -> Result<ParsedSession<'a>, SessionError> {
    let (lines, open_cutoff) = checked_lines(bytes)?;
    let mut rows = Vec::new();
    let mut next_sequence = 0_u64;
    let mut checkpoints = Vec::new();
    let mut end = None;
    let mut start = None;
    for (index, line) in lines.into_iter().enumerate() {
        let location = format!("{} line {}", path.display(), index + 1);
        let line = std::str::from_utf8(line)
            .map_err(|error| SessionError(format!("{location}: {error}")))?;
        let (value, order) = parse_json(line, &location)?;
        let kind = string_field(&value, "kind", &location)?;
        if end.is_some() {
            return Err(invalid(&location, "kind", "end must be the final row"));
        }
        if start.is_none() {
            if kind != "start" {
                return Err(invalid(&location, "kind", "first row must be start"));
            }
            start = Some(parse_start(&value, &order)?);
        } else {
            match kind {
                "end" => {
                    end = Some((
                        parse_manifest(&value, "end")?,
                        value.get("stop_ns").cloned().unwrap_or(Value::Null),
                    ))
                }
                "checkpoint" => {
                    let cutoff: i64 = integer(
                        field(&value, "cutoff_ns", "checkpoint")?,
                        "checkpoint",
                        "cutoff_ns",
                    )?;
                    let manifest = parse_manifest(&value, &format!("checkpoint {cutoff}"))?;
                    checkpoints.push(RecordedCheckpoint {
                        line,
                        cutoff: Timestamp::from_nanos(cutoff),
                        manifest,
                    });
                    continue;
                }
                "diagnostic" => {
                    if value.get("seq").is_some() {
                        return Err(invalid(&location, "seq", "diagnostic rows have no seq"));
                    }
                    continue;
                }
                "action" | "report" | "gap" | "timer_set" | "timer_fired" => {
                    let sequence = field(&value, "seq", &location)?;
                    if integer::<u64>(sequence, &location, "seq")? != next_sequence {
                        return Err(difference(
                            &format!("seq {next_sequence}"),
                            "seq",
                            Some(sequence),
                            Some(&json!(next_sequence.to_string())),
                        ));
                    }
                    next_sequence = next_sequence
                        .checked_add(1)
                        .ok_or_else(|| invalid(&location, "seq", "sequence overflow"))?;
                    if let Some(cutoff) = open_cutoff {
                        let time: i64 = integer(
                            field(&value, "ts_local_ns", &location)?,
                            &location,
                            "ts_local_ns",
                        )?;
                        if time >= cutoff.as_nanos() {
                            continue;
                        }
                    }
                }
                _ => {
                    return Err(invalid(
                        &location,
                        "kind",
                        &format!("unexpected kind {kind:?}"),
                    ));
                }
            }
        }
        rows.push(line);
    }
    let start =
        start.ok_or_else(|| SessionError("incomplete session: missing start".to_owned()))?;
    Ok(ParsedSession {
        start,
        rows,
        checkpoints,
        end,
    })
}

fn verify_midnights(
    recorded: &ParsedSession<'_>,
    anchor: i64,
    stop: Option<Timestamp>,
) -> Result<(), SessionError> {
    let mut next = next_utc_midnight(Timestamp::from_nanos(anchor));
    let mut previous: &[ManifestEntry] = &[];
    for checkpoint in &recorded.checkpoints {
        if next != Some(checkpoint.cutoff) || stop.is_some_and(|stop| checkpoint.cutoff > stop) {
            return Err(SessionError(format!(
                "unexpected checkpoint cutoff_ns {}: next midnight {:?}, inclusive stop {:?}",
                checkpoint.cutoff.as_nanos(),
                next.map(Timestamp::as_nanos),
                stop.map(Timestamp::as_nanos)
            )));
        }
        if !checkpoint.manifest.starts_with(previous) {
            return Err(SessionError(format!(
                "checkpoint {} manifest does not extend the previous checkpoint manifest",
                checkpoint.cutoff.as_nanos()
            )));
        }
        previous = &checkpoint.manifest;
        next = next_utc_midnight(checkpoint.cutoff);
    }
    if let (Some(next), Some(stop)) = (next, stop)
        && next <= stop
    {
        return Err(SessionError(format!(
            "missing checkpoint at midnight {}",
            next.as_nanos()
        )));
    }
    if !recorded.manifest().starts_with(previous) {
        let cutoff = recorded
            .checkpoints
            .last()
            .expect("nonempty previous manifest")
            .cutoff;
        return Err(SessionError(format!(
            "checkpoint {} manifest is not a prefix of end.manifest",
            cutoff.as_nanos()
        )));
    }
    Ok(())
}

/// Rebuild the session from verified raw files and compare every recorded field.
///
/// `build` must identify the running binary. The factory selects components by
/// name, rejects invalid parameters, and returns empty components plus their
/// canonical parameter lists. It receives only the start parameters and decoded
/// raw input, never the recorded decisions. All file hashes are checked before
/// the factory is called. The first mismatch includes its row, field and values.
pub fn check_session<S: Strategy>(
    path: &Path,
    build: &BuildIdentity,
    factory: impl FnOnce(&SessionStart, &Recording) -> Result<ReplayComponents<S>, String>,
) -> Result<CheckSummary, SessionError> {
    let bytes = std::fs::read(path)
        .map_err(|error| SessionError(format!("{}: {error}", path.display())))?;
    let recorded = parse_session(path, &bytes)?;
    if recorded.start.commit != build.commit || recorded.start.dirty != build.dirty {
        return Err(SessionError(format!(
            "build identity mismatch: session commit={} dirty={}; binary commit={} dirty={}",
            recorded.start.commit, recorded.start.dirty, build.commit, build.dirty
        )));
    }
    if build.dirty {
        eprintln!("warning: both builds are dirty; equal dirty flags do not prove equal code");
    }

    let mut start = recorded.start.clone();
    let mut paper = match start.driver.as_str() {
        "paper" => Some(PaperIdentity {
            source: recorded
                .start
                .source
                .as_deref()
                .filter(|source| !source.is_empty())
                .ok_or_else(|| invalid("start", "source", "paper requires a nonempty source"))?,
            process_start_wall_ns: start.process_start_wall_ns.ok_or_else(|| {
                invalid(
                    "start",
                    "process_start_wall_ns",
                    "paper requires a process anchor",
                )
            })?,
            sealed: recorded.end.is_some(),
            extents: recorded
                .checkpoints
                .iter()
                .map(|c| (c.cutoff, None))
                .collect(),
        }),
        "backtest" => {
            if !recorded.checkpoints.is_empty() {
                return Err(invalid(
                    "checkpoint",
                    "kind",
                    "checkpoints require a paper session",
                ));
            }
            start.source = None;
            start.process_start_wall_ns = None;
            None
        }
        _ => return Err(invalid("start", "driver", "unknown driver")),
    };
    let manifest_location = if recorded.end.is_some() {
        "end".to_owned()
    } else {
        format!(
            "checkpoint {}",
            recorded
                .checkpoints
                .last()
                .expect("open checkpoint")
                .cutoff
                .as_nanos()
        )
    };
    if let Some(identity) = paper.as_ref() {
        let stop = recorded
            .end
            .as_ref()
            .map(|(_, stop)| integer(stop, "end", "stop_ns").map(Timestamp::from_nanos))
            .transpose()?;
        verify_midnights(&recorded, identity.process_start_wall_ns, stop)?;
        for (index, entry) in recorded.manifest().iter().enumerate() {
            if entry.file_seq != Some(index as u64) {
                return Err(invalid(
                    &format!("{manifest_location}.manifest[{index}]"),
                    "file_seq",
                    &format!("paper chain must start at zero and continue with {index}"),
                ));
            }
        }
    }
    start.instrument = instrument_id(start.network, &start.coin);
    compare_row(recorded.rows[0], &Session::new(&start).lines()[0])?;

    let session_path = std::path::absolute(path)
        .map_err(|error| SessionError(format!("{}: {error}", path.display())))?;
    let directory = session_path
        .parent()
        .expect("absolute session path has a parent");
    let names: Vec<_> = recorded
        .manifest()
        .iter()
        .map(|entry| PathBuf::from(&entry.path))
        .collect();
    let paths: Vec<_> = names.iter().map(|name| directory.join(name)).collect();
    let mut hashes = Vec::with_capacity(paths.len());
    let mut stamps = Vec::with_capacity(paths.len());
    for (index, (entry, path)) in recorded.manifest().iter().zip(&paths).enumerate() {
        let hash_location = recorded
            .checkpoints
            .iter()
            .find(|checkpoint| checkpoint.manifest.len() > index)
            .map_or_else(
                || manifest_location.clone(),
                |checkpoint| format!("checkpoint {}", checkpoint.cutoff.as_nanos()),
            );
        let (hash, stamp) = sha256_file_stamped(path, None).map_err(|error| {
            SessionError(format!(
                "raw file {} at {hash_location}.manifest[{index}]: {error}",
                path.display()
            ))
        })?;
        if hash != entry.sha256 {
            return Err(SessionError(format!(
                "raw file {} SHA-256 mismatch at {hash_location}.manifest[{index}].sha256: expected {}, replayed {hash}",
                path.display(),
                entry.sha256
            )));
        }
        hashes.push(hash);
        stamps.push(stamp);
    }
    let mut recording = read_recordings_from(
        directory,
        &names,
        start.network,
        &start.coin,
        Some(&stamps),
        paper.as_mut(),
    )
    .map_err(|error| SessionError(format!("{manifest_location}: {error}")))?;
    if paper.is_some()
        && let Some((_, claimed_stop)) = &recorded.end
    {
        let stop = Timestamp::from_nanos(integer(claimed_stop, "end", "stop_ns")?);
        if recording.stop_time != Some(stop) {
            return Err(difference(
                "end",
                "stop_ns",
                Some(claimed_stop),
                recording
                    .stop_time
                    .map(|time| json!(time.as_nanos().to_string()))
                    .as_ref(),
            ));
        }
    }
    let open_cutoff = recorded
        .end
        .is_none()
        .then(|| recorded.checkpoints.last().expect("open checkpoint").cutoff);
    if let Some(cutoff) = open_cutoff {
        // A sealed file can contain more than the published extent. Decode
        // it for extent verification, then expose only the prefix to the
        // strategy factory and replay, even if one frame straddles midnight.
        let count = recording
            .events
            .partition_point(|event| event.ts_local < cutoff);
        recording.events.truncate(count);
        recording.origins.truncate(count);
    }
    let ReplayComponents {
        mut host,
        mut sim,
        strategy_parameters,
        sim_parameters,
    } = factory(&start, &recording).map_err(SessionError)?;
    start.strategy = strategy_parameters;
    start.sim = sim_parameters;
    compare_row(recorded.rows[0], &Session::new(&start).lines()[0])?;

    let manifest = ordered_manifest(&recording, recorded.manifest(), &paths, &hashes)?;
    if let Some(identity) = paper {
        check_paper_replay(
            &recorded, &recording, &identity, &start, manifest, &mut host, &mut sim,
        )?;
    } else {
        let (replayed, _, _) =
            crate::replay_session(&recording, &start, manifest, &mut host, &mut sim)
                .map_err(SessionError)?;
        compare_rows(&recorded.rows, replayed.lines())?;
    }
    Ok(CheckSummary {
        rows: recorded.rows.len() + recorded.checkpoints.len(),
        checkpoint: open_cutoff,
    })
}

fn check_paper_replay<S: Strategy>(
    recorded: &ParsedSession<'_>,
    recording: &Recording,
    identity: &PaperIdentity<'_>,
    start: &SessionStart,
    manifest: Vec<ManifestEntry>,
    host: &mut StrategyHost<S>,
    sim: &mut SimGateway,
) -> Result<(), SessionError> {
    let mut session = Session::new(start);
    let mut driver = IncrementalDriver::with_session(host, sim, &mut session);
    let mut events = recording.events.iter().zip(&recording.origins).peekable();
    let mut remaining = recorded.rows.as_slice();
    for checkpoint in &recorded.checkpoints {
        while let Some((event, origin)) =
            events.next_if(|(event, _)| event.ts_local < checkpoint.cutoff)
        {
            driver
                .on_market_event_with_origin(event, origin)
                .map_err(|error| SessionError(error.to_string()))?;
        }
        let boundary = driver
            .checkpoint_before(checkpoint.cutoff)
            .map_err(|error| SessionError(error.to_string()))?;
        let emitted = driver.take_session_lines();
        let count = remaining.len().min(emitted.len());
        compare_rows(&remaining[..count], &emitted)?;
        remaining = &remaining[count..];

        let last_raw = identity.extents[&checkpoint.cutoff];
        let file_count = last_raw.map_or(0, |raw| raw.file_index + 1);
        compare_row(
            checkpoint.line,
            &checkpoint_line(boundary, last_raw, &manifest[..file_count]),
        )?;
    }
    if recorded.end.is_some() {
        for (event, origin) in events {
            driver
                .on_market_event_with_origin(event, origin)
                .map_err(|error| SessionError(error.to_string()))?;
        }
        let stop = recording
            .stop_time
            .expect("sealed paper chain ends with shutdown");
        let trace = driver
            .finish(stop)
            .map_err(|error| SessionError(error.to_string()))?;
        let end = crate::session_end(recording, start, manifest, host.orders().len(), &trace)?;
        session.finish(&end);
        compare_rows(remaining, session.lines())?;
    } else {
        compare_rows(remaining, &[])?;
    }
    Ok(())
}

fn ordered_manifest(
    recording: &Recording,
    expected: &[ManifestEntry],
    paths: &[PathBuf],
    hashes: &[String],
) -> Result<Vec<ManifestEntry>, SessionError> {
    let mut used = vec![false; paths.len()];
    recording
        .files
        .iter()
        .map(|(path, file_seq)| {
            let index = paths
                .iter()
                .enumerate()
                .position(|(index, candidate)| !used[index] && candidate == path)
                .ok_or_else(|| {
                    SessionError("recording reader returned an undeclared file".to_owned())
                })?;
            used[index] = true;
            Ok(ManifestEntry {
                path: expected[index].path.clone(),
                file_seq: *file_seq,
                sha256: hashes[index].clone(),
            })
        })
        .collect()
}

fn row_location(value: &Value) -> String {
    match value.get("kind").and_then(Value::as_str) {
        Some("start") => "start".to_owned(),
        Some("end") => "end".to_owned(),
        Some("checkpoint") => format!("checkpoint {}", value["cutoff_ns"]),
        _ => value
            .get("seq")
            .and_then(Value::as_str)
            .map_or_else(|| "row".to_owned(), |sequence| format!("seq {sequence}")),
    }
}

fn compare_rows(expected: &[&str], replayed: &[String]) -> Result<(), SessionError> {
    for index in 0..expected.len().max(replayed.len()) {
        match (expected.get(index), replayed.get(index)) {
            (Some(expected), Some(replayed)) => compare_row(expected, replayed)?,
            (expected, replayed) => {
                let parse = |row: Option<&str>| {
                    row.map(|text| parse_json(text, "row").map(|(value, _)| value))
                        .transpose()
                };
                let expected = parse(expected.copied())?;
                let replayed = parse(replayed.map(String::as_str))?;
                let value = expected
                    .as_ref()
                    .or(replayed.as_ref())
                    .expect("one row exists");
                return Err(difference(
                    &row_location(value),
                    "row",
                    expected.as_ref(),
                    replayed.as_ref(),
                ));
            }
        }
    }
    Ok(())
}

fn compare_row(expected: &str, replayed: &str) -> Result<(), SessionError> {
    // Recorded rows were already strictly parsed, and generated rows come from
    // the session encoder. Most checks can compare their complete bytes.
    if expected == replayed {
        return Ok(());
    }
    let (expected, _) = parse_json(expected, "recorded row")?;
    let (replayed, order) = parse_json(replayed, "replayed row")?;
    let location = if expected.get("kind") == Some(&json!("end"))
        && replayed.get("kind") != Some(&json!("end"))
    {
        row_location(&replayed)
    } else {
        row_location(&expected)
    };
    compare_value(&expected, &replayed, &order, &location, "")?;
    Err(SessionError(format!(
        "{location} encoding differs: values match, expected canonical field order and spacing from the replayed row"
    )))
}

fn compare_value(
    expected: &Value,
    replayed: &Value,
    order: &JsonOrder,
    location: &str,
    path: &str,
) -> Result<(), SessionError> {
    match (expected, replayed, order) {
        (Value::Object(expected), Value::Object(replayed), JsonOrder::Object(fields)) => {
            for (name, child_order) in fields {
                let child_path = if path.is_empty() {
                    name.clone()
                } else {
                    format!("{path}.{name}")
                };
                let replayed_value = &replayed[name];
                let Some(expected_value) = expected.get(name) else {
                    return Err(difference(
                        location,
                        &child_path,
                        None,
                        Some(replayed_value),
                    ));
                };
                compare_value(
                    expected_value,
                    replayed_value,
                    child_order,
                    location,
                    &child_path,
                )?;
            }
            for (name, value) in expected {
                if !replayed.contains_key(name) {
                    let child_path = if path.is_empty() {
                        name.clone()
                    } else {
                        format!("{path}.{name}")
                    };
                    return Err(difference(location, &child_path, Some(value), None));
                }
            }
        }
        (Value::Array(expected), Value::Array(replayed), JsonOrder::Array(orders)) => {
            for (index, ((expected, replayed), order)) in
                expected.iter().zip(replayed).zip(orders).enumerate()
            {
                let child_path = format!("{path}[{index}]");
                compare_value(expected, replayed, order, location, &child_path)?;
            }
            if expected.len() != replayed.len() {
                let index = expected.len().min(replayed.len());
                return Err(difference(
                    location,
                    &format!("{path}[{index}]"),
                    expected.get(index),
                    replayed.get(index),
                ));
            }
        }
        _ if expected != replayed => {
            return Err(difference(location, path, Some(expected), Some(replayed)));
        }
        _ => {}
    }
    Ok(())
}

// Retain the encoder's field order for first-difference reporting and reject
// duplicate keys before serde_json::Value can hide an earlier occurrence.
enum JsonOrder {
    Object(Vec<(String, JsonOrder)>),
    Array(Vec<JsonOrder>),
    Leaf,
}

struct JsonCursor<'a> {
    text: &'a str,
    position: usize,
}

impl JsonCursor<'_> {
    fn skip_space(&mut self) {
        while self
            .text
            .as_bytes()
            .get(self.position)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.position += 1;
        }
    }

    fn string(&mut self) -> String {
        let start = self.position;
        self.position += 1;
        let bytes = self.text.as_bytes();
        while bytes[self.position] != b'"' {
            self.position += if bytes[self.position] == b'\\' { 2 } else { 1 };
        }
        self.position += 1;
        serde_json::from_str(&self.text[start..self.position]).expect("JSON was validated")
    }

    fn order(&mut self) -> Result<JsonOrder, SessionError> {
        self.skip_space();
        match self.text.as_bytes()[self.position] {
            b'{' => {
                self.position += 1;
                let mut fields = Vec::new();
                let mut names = BTreeSet::new();
                loop {
                    self.skip_space();
                    if self.text.as_bytes()[self.position] == b'}' {
                        self.position += 1;
                        return Ok(JsonOrder::Object(fields));
                    }
                    let name = self.string();
                    if !names.insert(name.clone()) {
                        return Err(SessionError(format!("duplicate field {name:?}")));
                    }
                    self.skip_space();
                    self.position += 1; // The validated colon.
                    fields.push((name, self.order()?));
                    self.skip_space();
                    if self.text.as_bytes()[self.position] == b',' {
                        self.position += 1;
                    }
                }
            }
            b'[' => {
                self.position += 1;
                let mut elements = Vec::new();
                loop {
                    self.skip_space();
                    if self.text.as_bytes()[self.position] == b']' {
                        self.position += 1;
                        return Ok(JsonOrder::Array(elements));
                    }
                    elements.push(self.order()?);
                    self.skip_space();
                    if self.text.as_bytes()[self.position] == b',' {
                        self.position += 1;
                    }
                }
            }
            b'"' => {
                self.string();
                Ok(JsonOrder::Leaf)
            }
            _ => {
                while self
                    .text
                    .as_bytes()
                    .get(self.position)
                    .is_some_and(|byte| !byte.is_ascii_whitespace() && !b",]}".contains(byte))
                {
                    self.position += 1;
                }
                Ok(JsonOrder::Leaf)
            }
        }
    }
}

fn parse_json(text: &str, location: &str) -> Result<(Value, JsonOrder), SessionError> {
    let value: Value =
        serde_json::from_str(text).map_err(|error| SessionError(format!("{location}: {error}")))?;
    if !value.is_object() {
        return Err(SessionError(format!("{location}: expected a JSON object")));
    }
    let order = JsonCursor { text, position: 0 }
        .order()
        .map_err(|error| SessionError(format!("{location}: {error}")))?;
    Ok((value, order))
}
