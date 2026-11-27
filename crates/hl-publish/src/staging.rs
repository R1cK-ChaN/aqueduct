//! Inspect the final directory listing in basename order. Foreign names stay local.

use std::collections::BTreeSet;
use std::fs::{self, File, Metadata};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use hl_recorder::{FINISHED_SUFFIX, PARTIAL_SUFFIX, TRUNCATED_SUFFIX, UtcHour, public_v1};

use crate::date::Day;
use crate::manifest::Entry;
use crate::recording::{self, Recording};

fn stem_hour(stem: &str, compact_day: &str) -> Option<u8> {
    let local = stem
        .strip_prefix(public_v1::SOURCE_NAME)?
        .strip_prefix('-')?
        .strip_prefix(compact_day)?
        .strip_prefix('T')?;
    let (hour, millis) = local.split_once('-')?;
    if hour.len() != 2
        || !hour.bytes().all(|b| b.is_ascii_digit())
        || millis.parse::<i64>().ok()?.to_string() != millis
    {
        return None;
    }
    hour.parse().ok().filter(|&h| h < 24)
}

pub(crate) fn recording_hour(name: &str, compact_day: &str) -> Option<u8> {
    stem_hour(name.strip_suffix(FINISHED_SUFFIX)?, compact_day)
}

// Compare both the path and the descriptor with the initial observation, including
// inode change time so an in-place rewrite with restored length/mtime is detected.
fn stamp(m: &Metadata) -> (u64, u64, u64, i64, i64) {
    (m.dev(), m.ino(), m.len(), m.ctime(), m.ctime_nsec())
}

fn directory(path: &Path) -> Result<Metadata, &'static str> {
    fs::symlink_metadata(path)
        .ok()
        .filter(Metadata::is_dir)
        .ok_or("invalid staging directory")
}

fn inspect_entry(
    path: &Path,
    compact: &str,
    deny: &[String],
    rejected: &mut BTreeSet<String>,
) -> Entry {
    let name = path.file_name().and_then(|n| n.to_str());
    let sealed_hour = name.and_then(|n| recording_hour(n, compact));
    let unfinished = name.and_then(|n| {
        let (stem, reason) = n
            .strip_suffix(PARTIAL_SUFFIX)
            .map(|s| (s, "partial recording"))
            .or_else(|| {
                n.strip_suffix(TRUNCATED_SUFFIX)
                    .map(|s| (s, "truncated recording"))
            })?;
        Some((stem_hour(stem, compact)?, reason))
    });
    let hour = sealed_hour.or(unfinished.map(|(h, _)| h));
    let check = (|| -> Result<Recording, &'static str> {
        let before = fs::symlink_metadata(path).map_err(|_| "unreadable file")?;
        if !before.is_file() {
            return Err("not a regular file");
        }
        if let Some((_, reason)) = unfinished {
            return Err(reason);
        }
        let h = sealed_hour.ok_or("invalid filename")?;
        let name = name.ok_or("invalid filename")?;
        if deny.iter().any(|s| name.contains(s)) {
            return Err("filename denied");
        }
        let file = File::open(path).map_err(|_| "unreadable file")?;
        let expected = stamp(&before);
        if stamp(&file.metadata().map_err(|_| "unreadable file")?) != expected {
            return Err("file changed");
        }
        let recording = recording::inspect(&file, deny, rejected)?;
        if recording.size_bytes != before.len()
            || stamp(&file.metadata().map_err(|_| "unreadable file")?) != expected
            || stamp(&fs::symlink_metadata(path).map_err(|_| "unreadable file")?) != expected
        {
            return Err("file changed");
        }
        if UtcHour::containing(recording.first_wall_ns).label() != format!("{compact}T{h:02}") {
            return Err("wrong opening hour");
        }
        Ok(recording)
    })();
    Entry {
        name: name.filter(|_| check.is_ok()).map(str::to_owned),
        hour,
        check,
    }
}

pub(crate) fn inspect_day(
    root: &Path,
    day: &Day,
    deny: &[String],
    rejected: &mut BTreeSet<String>,
) -> Result<Vec<Entry>, &'static str> {
    let compact = day.compact();
    let source = root.join(public_v1::SOURCE_NAME);
    let dir = source.join(&compact);
    directory(root)?;
    let source_before = directory(&source)?;
    let day_before = directory(&dir)?;
    let mut names = fs::read_dir(&dir)
        .map_err(|_| "unreadable staging directory")?
        .map(|entry| entry.map(|e| e.file_name()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "incomplete staging directory")?;
    names.sort_unstable();
    let entries = names
        .iter()
        .map(|name| inspect_entry(&dir.join(name), &compact, deny, rejected))
        .collect();
    // An incomplete or changing inventory cannot describe a sealed day.
    let source_after = directory(&source)?;
    if (source_before.dev(), source_before.ino()) != (source_after.dev(), source_after.ino())
        || stamp(&day_before) != stamp(&directory(&dir)?)
    {
        return Err("staging directory changed");
    }
    Ok(entries)
}
