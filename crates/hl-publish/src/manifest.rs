//! Project fully screened recordings onto the daily manifest (RFC 0010 D5).

use std::collections::{BTreeMap, BTreeSet};
use std::{fmt::Display, str::FromStr};

use hl_codec::hyperliquid::TradeCutoff;
use hl_codec::rawlog::ChainKey;
use hl_recorder::public_v1::{COINS, SOURCE_NAME};
use hl_types::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::date::Day;
use crate::recording::{DisconnectReason, Lifecycle, Recording};
use crate::staging::recording_hour;

const VERSION: &str = "v1";
const REGION: &str = "asia-northeast1";
const INVALID_PREVIOUS: &str = "invalid previous manifest";
const HOUR_NS: i128 = 3_600_000_000_000;

pub(crate) struct Entry {
    pub name: Option<String>,
    pub hour: Option<u8>,
    pub check: Result<Recording, &'static str>,
}

// Read only the previous day's continuity information. In particular, an old
// manifest can never supply an arbitrary path, reason or object to publish.
#[derive(Deserialize)]
struct PreviousManifest {
    schema_version: u64,
    day: String,
    source: String,
    region: String,
    dataset_version: String,
    files: Vec<FileRow>,
    gaps: Vec<Value>,
}

#[derive(Deserialize, Serialize)]
struct FileRow {
    path: String,
    size_bytes: u64,
    sha256: String,
    process_start_wall_ns: String,
    file_seq: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_path: Option<String>,
}

#[derive(Clone, Copy)]
struct Position {
    start: i64,
    seq: u64,
}

impl Position {
    fn of(recording: &Recording) -> Self {
        Self {
            start: recording.chain.process_start_wall_ns,
            seq: recording.file_seq,
        }
    }
}

#[derive(Clone, Copy)]
struct Disconnect {
    position: Position,
    through_seq: u64,
    at_ns: i64,
    event: &'static str,
    reason: Option<DisconnectReason>,
}

impl Disconnect {
    fn gap(self, end_ns: Option<i64>, continues: bool) -> Value {
        let mut gap = gap("disconnect", self.position, Some(self.at_ns), end_ns);
        gap["event"] = json!(self.event);
        if let Some(reason) = self.reason {
            gap["reason"] = json!(reason);
        }
        if continues {
            gap["continues"] = json!(true);
            if self.through_seq != self.position.seq {
                gap["through_file_seq"] = json!(self.through_seq.to_string());
            }
        }
        gap
    }

    fn from_gap(gap: &Value) -> Option<Self> {
        if gap["kind"] != "disconnect" || gap.get("end_wall_ns") != Some(&Value::Null) {
            return None;
        }
        let position = Position {
            start: decimal(gap["process_start_wall_ns"].as_str()?)?,
            seq: decimal(gap["file_seq"].as_str()?)?,
        };
        let through_seq = match gap.get("through_file_seq") {
            Some(seq) => decimal(seq.as_str()?)?,
            None => position.seq,
        };
        if through_seq < position.seq {
            return None;
        }
        Some(Self {
            position,
            through_seq,
            at_ns: decimal(gap["start_wall_ns"].as_str()?)?,
            event: disconnect_event(gap["event"].as_str()?)?,
            reason: serde_json::from_value(gap["reason"].clone()).ok()?,
        })
    }
}

fn disconnect_event(event: &str) -> Option<&'static str> {
    match event {
        "disconnected" => Some("disconnected"),
        "shutdown" => Some("shutdown"),
        "connect_failed" => Some("connect_failed"),
        "send_policy_rejected" => Some("send_policy_rejected"),
        _ => None,
    }
}

fn decimal<T: FromStr + Display>(text: &str) -> Option<T> {
    let number = text.parse::<T>().ok()?;
    (number.to_string() == text).then_some(number)
}

pub(crate) struct Previous {
    same_version: bool,
    tails: BTreeMap<ChainKey, (u64, String)>,
    gaps_through: BTreeMap<i64, u64>,
    last: Option<Position>,
    disconnect: Option<Disconnect>,
}

impl Previous {
    fn last_accepted_seq(&self, chain: &ChainKey) -> Option<u64> {
        self.tails.get(chain).map(|(seq, _)| *seq).or_else(|| {
            self.disconnect
                .filter(|d| d.position.start == chain.process_start_wall_ns)
                .map(|d| d.through_seq)
        })
    }

    fn already_sealed(&self, chain: &ChainKey, seq: u64) -> bool {
        self.last_accepted_seq(chain)
            .is_some_and(|last| seq <= last)
            || self
                .gaps_through
                .get(&chain.process_start_wall_ns)
                .is_some_and(|&last| seq <= last)
    }

    pub fn parse(text: &str, expected_previous_day: &str) -> Result<Self, &'static str> {
        let manifest: PreviousManifest =
            serde_json::from_str(text).map_err(|_| INVALID_PREVIOUS)?;
        if manifest.schema_version != 1
            || manifest.day != expected_previous_day
            || manifest.source != SOURCE_NAME
            || manifest.region != REGION
            || manifest
                .dataset_version
                .strip_prefix('v')
                .and_then(decimal::<u64>)
                .is_none()
        {
            return Err(INVALID_PREVIOUS);
        }
        let mut previous = Self {
            same_version: manifest.dataset_version == VERSION,
            tails: BTreeMap::new(),
            gaps_through: BTreeMap::new(),
            last: None,
            disconnect: None,
        };
        let compact = manifest.day.replace('-', "");
        let prefix = format!("{}/{SOURCE_NAME}/{compact}/", manifest.dataset_version);
        let mut paths = BTreeSet::new();
        let mut published = BTreeSet::new();
        for file in manifest.files {
            let name = file.path.strip_prefix(&prefix).ok_or(INVALID_PREVIOUS)?;
            if recording_hour(name, &compact).is_none()
                || !paths.insert(name.to_owned())
                || file.size_bytes == 0
                || file.sha256.len() != 64
                || !file
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(INVALID_PREVIOUS);
            }
            let start = decimal(&file.process_start_wall_ns).ok_or(INVALID_PREVIOUS)?;
            let seq = decimal(&file.file_seq).ok_or(INVALID_PREVIOUS)?;
            published.insert((start, seq));
            let chain = ChainKey {
                source: SOURCE_NAME.to_owned(),
                process_start_wall_ns: start,
            };
            if previous
                .tails
                .insert(chain, (seq, file.path))
                .is_some_and(|(last, _)| last >= seq)
            {
                return Err(INVALID_PREVIOUS);
            }
            previous.last = Some(Position { start, seq });
        }
        for gap in manifest.gaps {
            if gap["kind"] == "missing_sequence" {
                // A trailing conflict can leave this sequence without a published
                // file. A new candidate cannot resolve yesterday's sealed conflict.
                let number = |field: &str| gap[field].as_str().ok_or(INVALID_PREVIOUS);
                let start = decimal(number("process_start_wall_ns")?).ok_or(INVALID_PREVIOUS)?;
                let first: u64 = decimal(number("file_seq")?).ok_or(INVALID_PREVIOUS)?;
                let last = decimal(number("last_file_seq")?).ok_or(INVALID_PREVIOUS)?;
                if first > last
                    || published
                        .range((start, first)..=(start, last))
                        .next()
                        .is_some()
                {
                    return Err(INVALID_PREVIOUS);
                }
                let through = previous.gaps_through.entry(start).or_insert(last);
                *through = (*through).max(last);
            }
            if let Some(carry) = gap.get("continues") {
                if !carry.as_bool().ok_or(INVALID_PREVIOUS)? {
                    continue;
                }
                let open = Disconnect::from_gap(&gap).ok_or(INVALID_PREVIOUS)?;
                if previous.disconnect.replace(open).is_some()
                    || previous.last.is_some_and(|last| {
                        last.start != open.position.start || last.seq != open.through_seq
                    })
                {
                    return Err(INVALID_PREVIOUS);
                }
                previous.last = Some(Position {
                    start: open.position.start,
                    seq: open.through_seq,
                });
            }
        }
        Ok(previous)
    }
}

// Each sequence has one accepted entry or a known hole. Retain even trailing
// conflict positions when every candidate for the chain was withheld.
type Chains = BTreeMap<ChainKey, BTreeMap<u64, Option<usize>>>;

fn screen_sequences(entries: &mut [Entry], previous: Option<&Previous>) -> Chains {
    let mut claims: BTreeMap<(ChainKey, u64), Vec<usize>> = BTreeMap::new();
    for (index, entry) in entries.iter_mut().enumerate() {
        if let Ok(file) = &entry.check {
            if entry.name.is_none() {
                entry.check = Err("invalid filename");
            } else {
                claims
                    .entry((file.chain.clone(), file.file_seq))
                    .or_default()
                    .push(index);
            }
        }
    }
    let mut chains = Chains::new();
    for ((chain, seq), copies) in claims {
        if previous.is_some_and(|p| p.already_sealed(&chain, seq)) {
            for index in copies {
                entries[index].check = Err("previous_sequence");
            }
            continue;
        }
        let position = if copies.len() == 1 {
            Some(copies[0])
        } else {
            let first = recording(&entries[copies[0]]);
            let identical = copies.iter().all(|&i| {
                let file = recording(&entries[i]);
                file.size_bytes == first.size_bytes && file.sha256 == first.sha256
            });
            let reason = if identical {
                "duplicate_sequence"
            } else {
                "sequence_conflict"
            };
            for index in copies {
                entries[index].check = Err(reason);
            }
            None
        };
        chains.entry(chain).or_default().insert(seq, position);
    }
    chains
}

fn recording(entry: &Entry) -> &Recording {
    entry
        .check
        .as_ref()
        .expect("only accepted indices enter replay")
}

fn gap(kind: &str, position: Position, start: Option<i64>, end: Option<i64>) -> Value {
    // A clock step can reverse two receipt stamps. Neither bounds a UTC interval then.
    let (start, end) = if start.zip(end).is_some_and(|(a, b)| a > b) {
        (None, None)
    } else {
        (start, end)
    };
    json!({
        "kind": kind,
        "process_start_wall_ns": position.start.to_string(),
        "file_seq": position.seq.to_string(),
        "start_wall_ns": start.map(|n| n.to_string()),
        "end_wall_ns": end.map(|n| n.to_string()),
    })
}

fn withheld_hours(day: &Day, entries: &[Entry]) -> Vec<Value> {
    let mut withheld = BTreeMap::new();
    for entry in entries {
        if let Err(reason) = entry.check {
            *withheld.entry((entry.hour, reason)).or_insert(0usize) += 1;
        }
    }
    withheld
        .into_iter()
        .map(|((hour, reason), count)| {
            let start = hour.map(|h| day.start_ns() + i128::from(h) * HOUR_NS);
            json!({
                "kind": "withheld", "reason": reason, "files_withheld": count,
                "start_wall_ns": start.map(|n| n.to_string()),
                "end_wall_ns": start.map(|n| (n + HOUR_NS).to_string()),
            })
        })
        .collect()
}

fn missing_sequences(
    chains: &Chains,
    entries: &[Entry],
    previous: Option<&Previous>,
    gaps: &mut Vec<Value>,
) {
    for (chain, claims) in chains {
        let tail = previous.and_then(|p| p.last_accepted_seq(chain));
        let mut next = tail.map_or(Some(0), |seq| seq.checked_add(1));
        let mut last_ns = None;
        let mut missing = |first, last: u64, left, right| {
            let position = Position {
                start: chain.process_start_wall_ns,
                seq: first,
            };
            let mut gap = gap("missing_sequence", position, left, right);
            gap["last_file_seq"] = json!(last.to_string());
            gaps.push(gap);
        };
        for (&seq, index) in claims {
            if let Some(index) = *index {
                let file = recording(&entries[index]);
                if let Some(first) = next
                    && first < seq
                {
                    missing(first, seq - 1, last_ns, Some(file.first_wall_ns));
                }
                last_ns = Some(file.last_wall_ns);
                next = seq.checked_add(1);
            }
        }
        if let Some((&last, _)) = claims.last_key_value()
            && let Some(first) = next
            && first <= last
        {
            missing(first, last, last_ns, None);
        }
    }
}

// As in hl-backtest, a wall-clock step cannot reorder files within a chain.
// Only each chain's next accepted file competes by opening stamp and path.
fn replay_order(chains: &Chains, entries: &[Entry]) -> Vec<usize> {
    let mut streams: Vec<_> = chains
        .values()
        .map(|chain| chain.values().flatten())
        .collect();
    let key = |index: usize, stream| {
        (
            recording(&entries[index]).first_wall_ns,
            entries[index].name.as_deref(),
            stream,
        )
    };
    let mut heads = BTreeMap::new();
    for (stream, chain) in streams.iter_mut().enumerate() {
        if let Some(&index) = chain.next() {
            heads.insert(key(index, stream), index);
        }
    }
    let mut order = Vec::new();
    while let Some(((_, _, stream), index)) = heads.pop_first() {
        order.push(index);
        if let Some(&next) = streams[stream].next() {
            heads.insert(key(next, stream), next);
        }
    }
    order
}

struct Connections {
    last: Option<Position>,
    last_ns: Option<i64>,
    open: Option<Disconnect>,
}

impl Connections {
    fn begin(&mut self, position: Position, opened_ns: i64, gaps: &mut Vec<Value>) {
        let same_process = self.last.is_none_or(|last| last.start == position.start);
        let contiguous = self
            .last
            .is_none_or(|last| last.seq.checked_add(1) == Some(position.seq));
        if (!same_process || !contiguous)
            && let Some(open) = self.open.take()
        {
            // The unknown interval may include an unobserved reconnect.
            gaps.push(open.gap(None, false));
        }
        if let Some(last) = self.last
            && !same_process
        {
            let mut change = gap("process_change", position, self.last_ns, Some(opened_ns));
            change["previous_process_start_wall_ns"] = json!(last.start.to_string());
            change["previous_file_seq"] = json!(last.seq.to_string());
            gaps.push(change);
        }
        self.last = Some(position);
    }

    fn observe(&mut self, event: &Lifecycle, position: Position, gaps: &mut Vec<Value>) {
        // The codec identifies connection transitions. The open interval retains
        // the originating event and its chain position across file boundaries.
        let mut transition = TradeCutoff::UnknownStart;
        transition.observe_meta(event.event, Timestamp::from_nanos(event.at_ns));
        if let TradeCutoff::ConnectedSince(at) = transition {
            if let Some(open) = self.open.take() {
                gaps.push(open.gap(Some(at.as_nanos()), false));
            }
        } else if transition == TradeCutoff::Disconnected
            || matches!(event.event, "connect_failed" | "send_policy_rejected")
        {
            self.open.get_or_insert(Disconnect {
                position,
                through_seq: position.seq,
                at_ns: event.at_ns,
                event: event.event,
                reason: event.reason,
            });
        } else if event.event == "dropped" {
            // The recorder timestamps the counter read. Individual losses have no recorded time.
            let mut dropped = gap("dropped", position, None, Some(event.at_ns));
            dropped["count"] = json!(event.dropped);
            gaps.push(dropped);
        }
    }
}

pub(crate) fn assemble(
    day: &Day,
    expires: &str,
    entries: &mut [Entry],
    previous: Option<&Previous>,
    coverage_ends: Value,
    quality: Value,
) -> Value {
    let chains = screen_sequences(entries, previous);
    let mut gaps = withheld_hours(day, entries);
    missing_sequences(&chains, entries, previous, &mut gaps);
    let mut connections = Connections {
        last: previous.and_then(|p| p.last),
        last_ns: None,
        open: previous.and_then(|p| p.disconnect),
    };
    let mut coins: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut files = Vec::new();
    let prefix = format!("{VERSION}/{SOURCE_NAME}/{}/", day.compact());
    for index in replay_order(&chains, entries) {
        let entry = &entries[index];
        let file = recording(entry);
        let name = entry
            .name
            .as_deref()
            .expect("accepted file has a recorder name");
        let position = Position::of(file);
        connections.begin(position, file.first_wall_ns, &mut gaps);
        for event in &file.lifecycle {
            connections.observe(event, position, &mut gaps);
        }
        if let Some(open) = &mut connections.open {
            open.through_seq = position.seq;
        }
        connections.last_ns = Some(file.last_wall_ns);
        for (coin, channels) in &file.coins_covered {
            coins
                .entry(coin)
                .or_default()
                .extend(channels.iter().copied());
        }
        let predecessor = previous
            .and_then(|p| p.tails.get(&file.chain))
            .filter(|(seq, _)| seq.checked_add(1) == Some(file.file_seq));
        files.push(FileRow {
            path: format!("{prefix}{name}"),
            size_bytes: file.size_bytes,
            sha256: file.sha256.clone(),
            process_start_wall_ns: position.start.to_string(),
            file_seq: position.seq.to_string(),
            previous_path: predecessor.map(|(_, path)| path.clone()),
        });
    }
    if let Some(open) = connections.open {
        let chain = ChainKey {
            source: SOURCE_NAME.to_owned(),
            process_start_wall_ns: open.position.start,
        };
        let last_claim = chains.get(&chain).and_then(BTreeMap::last_key_value);
        let continues = last_claim.is_none_or(|(_, index)| index.is_some());
        gaps.push(open.gap(None, continues));
    }
    let mut manifest = json!({
        "schema_version": 1, "day": day.iso(), "expires": expires, "source": SOURCE_NAME,
        "region": REGION, "dataset_version": VERSION, "coins_covered": coins,
        "coverage_ends": coverage_ends, "files": files, "gaps": gaps, "quality": quality,
    });
    if previous.is_none_or(|p| !p.same_version) {
        manifest["version_change"] = json!({
            "effective_day": day.iso(), "added": COINS,
            "removed": previous.is_none().then(Vec::<String>::new),
        });
    }
    manifest
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_day(day: &str, previous: Option<&Previous>) -> Value {
        let date = Day::parse(day).expect("fixture date");
        let quality = json!({
            "day": day, "complete": true, "loader_revision": "synthetic",
            "checks": [{"name":"coverage","subject":"bbo","status":"info","compared":0,"violations":0}]
        });
        assemble(&date, day, &mut [], previous, json!([]), quality)
    }

    #[test]
    fn previous_day_and_positions_must_agree() {
        let day = "2026-11-25";
        let mut manifest = empty_day(day, None);
        assert!(Previous::parse(&manifest.to_string(), "2026-11-24").is_err());
        let ambiguous = manifest
            .to_string()
            .replacen("\"files\":", "\"files\":[],\"files\":", 1);
        assert!(Previous::parse(&ambiguous, day).is_err());
        let file = |hour| {
            json!({
                "path":format!("v1/hl-public/20261125/hl-public-20261125T{hour:02}-1.hlrec.zst"),
                "size_bytes": 100, "sha256": "0".repeat(64),
                "process_start_wall_ns": "1", "file_seq": "0"
            })
        };
        manifest["files"] = json!([file(0), file(1)]);
        assert!(Previous::parse(&manifest.to_string(), day).is_err());
        manifest["files"] = json!([file(0)]);
        manifest["gaps"] = json!([{
            "kind":"missing_sequence", "process_start_wall_ns":"1", "file_seq":"0", "last_file_seq":"3"
        }]);
        assert!(Previous::parse(&manifest.to_string(), day).is_err());
    }

    #[test]
    fn a_new_version_does_not_treat_previous_observations_as_a_roster() {
        let mut earlier = empty_day("2026-11-25", None);
        earlier["dataset_version"] = json!("v0");
        earlier["coins_covered"] = json!({"BTC": ["trades"]});
        let previous =
            Previous::parse(&earlier.to_string(), "2026-11-25").expect("earlier version");
        let current = empty_day("2026-11-26", Some(&previous));
        assert_eq!(
            current["version_change"],
            json!({
                "effective_day":"2026-11-26", "added":COINS, "removed":null
            })
        );
    }

    #[test]
    fn terminal_conflicts_at_max_sequence_stay_withheld_across_midnight() {
        let chain = ChainKey {
            source: SOURCE_NAME.to_owned(),
            process_start_wall_ns: 1,
        };
        let claims = BTreeMap::from([(chain.clone(), BTreeMap::from([(u64::MAX, None)]))]);
        let mut gaps = Vec::new();
        missing_sequences(&claims, &[], None, &mut gaps);
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0]["file_seq"], "0");
        assert_eq!(gaps[0]["last_file_seq"], u64::MAX.to_string());
        let mut manifest = empty_day("2026-11-25", None);
        manifest["gaps"] = json!(gaps);
        let prior = Previous::parse(&manifest.to_string(), "2026-11-25").expect("sealed conflict");
        assert!(prior.already_sealed(&chain, u64::MAX));
    }

    #[test]
    fn an_empty_day_preserves_the_last_verified_disconnect_sequence() {
        let open = Disconnect {
            position: Position { start: 1, seq: 1 },
            through_seq: 3,
            at_ns: 10,
            event: "disconnected",
            reason: Some(DisconnectReason::Stale),
        };
        let mut earlier = empty_day("2026-11-24", None);
        earlier["gaps"] = json!([open.gap(None, true)]);
        let previous = Previous::parse(&earlier.to_string(), "2026-11-24").expect("open interval");
        let relay = empty_day("2026-11-25", Some(&previous));
        assert_eq!(relay["gaps"][0]["through_file_seq"], "3");
        let previous = Previous::parse(&relay.to_string(), "2026-11-25").expect("relayed interval");
        let mut connections = Connections {
            last: previous.last,
            last_ns: None,
            open: previous.disconnect,
        };
        let position = Position { start: 1, seq: 4 };
        let mut gaps = Vec::new();
        connections.begin(position, 50, &mut gaps);
        connections.observe(
            &Lifecycle {
                at_ns: 55,
                event: "connected",
                reason: None,
                dropped: None,
            },
            position,
            &mut gaps,
        );
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0]["start_wall_ns"], "10");
        assert_eq!(gaps[0]["end_wall_ns"], "55");
        assert!(connections.open.is_none());
    }
}
