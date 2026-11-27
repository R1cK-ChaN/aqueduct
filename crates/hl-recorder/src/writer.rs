//! Writes records to one compressed file per source per UTC hour.
//!
//! Layout under the output directory:
//!
//! ```text
//! <source>/<YYYYMMDD>/<source>-<YYYYMMDD>T<HH>-<open_ms>.hlrec.zst
//! ```
//!
//! A file being written has the extra suffix `.part`. It is renamed when its
//! hour ends or the recorder shuts down cleanly. See
//! `docs/rfc/0002-raw-recording-format.md`.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use hl_codec::rawlog::{self, RecordKind};
use serde_json::json;

use crate::clock::{Stamp, UtcHour};

/// Suffix of a finished recording.
pub const FINISHED_SUFFIX: &str = ".hlrec.zst";

/// Suffix of a recording that is still being written.
pub const PARTIAL_SUFFIX: &str = ".hlrec.zst.part";

/// Suffix given to a partial recording left behind by a process that did not
/// shut down cleanly. Its compressed stream ends abruptly.
pub const TRUNCATED_SUFFIX: &str = ".truncated.hlrec.zst";

/// What the writer needs to know about one source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceInfo {
    /// Name used in directory and file names.
    pub name: String,
    /// URL recorded in each file's `file_open` record.
    pub url: String,
}

struct OpenFile {
    hour: UtcHour,
    part_path: PathBuf,
    final_path: PathBuf,
    encoder: zstd::Encoder<'static, BufWriter<File>>,
}

struct Slot {
    info: SourceInfo,
    open: Option<OpenFile>,
    next_file_seq: u64,
}

/// Owns every open recording file.
pub struct LogWriter {
    out_dir: PathBuf,
    zstd_level: i32,
    process_start_wall_ns: i64,
    slots: Vec<Slot>,
    sealed: Vec<PathBuf>,
}

impl std::fmt::Debug for LogWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogWriter")
            .field("out_dir", &self.out_dir)
            .field("sources", &self.slots.len())
            .finish_non_exhaustive()
    }
}

/// Renames partial recordings left under `source_dir` by an earlier process
/// and returns their new paths.
fn recover_partial_files(source_dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut recovered = Vec::new();
    if !source_dir.is_dir() {
        return Ok(recovered);
    }
    for day in fs::read_dir(source_dir)? {
        let day = day?.path();
        if !day.is_dir() {
            continue;
        }
        for entry in fs::read_dir(&day)? {
            let path = entry?.path();
            let Some(stem) = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_suffix(PARTIAL_SUFFIX))
            else {
                continue;
            };
            let renamed = day.join(format!("{stem}{TRUNCATED_SUFFIX}"));
            fs::rename(&path, &renamed)?;
            recovered.push(renamed);
        }
    }
    recovered.sort();
    Ok(recovered)
}

impl LogWriter {
    /// Prepares the output directory. Partial recordings left by an earlier
    /// process are renamed with [`TRUNCATED_SUFFIX`]; their new paths are
    /// returned next to the writer.
    ///
    /// `process_start_wall_ns` is the wall clock at which the monotonic
    /// clock was zero.
    pub fn new(
        out_dir: &Path,
        zstd_level: i32,
        process_start_wall_ns: i64,
        sources: Vec<SourceInfo>,
    ) -> io::Result<(Self, Vec<PathBuf>)> {
        fs::create_dir_all(out_dir)?;
        let mut recovered = Vec::new();
        for source in &sources {
            recovered.extend(recover_partial_files(&out_dir.join(&source.name))?);
        }
        let slots = sources
            .into_iter()
            .map(|info| Slot {
                info,
                open: None,
                next_file_seq: 0,
            })
            .collect();
        let writer = Self {
            out_dir: out_dir.to_owned(),
            zstd_level,
            process_start_wall_ns,
            slots,
            sealed: Vec::new(),
        };
        Ok((writer, recovered))
    }

    /// Appends one record to the file for `source` and the hour of
    /// `stamp.wall_ns`, opening or rotating the file as needed.
    ///
    /// # Panics
    ///
    /// Panics when `source` is not an index into the sources given to
    /// [`LogWriter::new`].
    pub fn write(
        &mut self,
        source: usize,
        kind: RecordKind,
        stamp: Stamp,
        payload: &[u8],
    ) -> io::Result<()> {
        let hour = UtcHour::containing(stamp.wall_ns);
        if self.slots[source]
            .open
            .as_ref()
            .is_some_and(|open| open.hour != hour)
        {
            self.seal(source)?;
        }
        if self.slots[source].open.is_none() {
            let open = self.open_file(source, hour, stamp)?;
            self.slots[source].open = Some(open);
            self.slots[source].next_file_seq += 1;
        }
        let open = self.slots[source].open.as_mut().expect("opened above");
        rawlog::write_record(
            &mut open.encoder,
            kind,
            stamp.wall_ns,
            stamp.mono_ns,
            payload,
        )
    }

    /// Flushes every open file to the operating system and finishes files
    /// whose hour ended before `now_wall_ns`. Call about once a second.
    pub fn tick(&mut self, now_wall_ns: i64) -> io::Result<()> {
        let current = UtcHour::containing(now_wall_ns);
        for source in 0..self.slots.len() {
            let Some(open) = self.slots[source].open.as_mut() else {
                continue;
            };
            if open.hour < current {
                self.seal(source)?;
            } else {
                open.encoder.flush()?;
            }
        }
        Ok(())
    }

    /// Finishes every open file.
    pub fn finish(&mut self) -> io::Result<()> {
        for source in 0..self.slots.len() {
            self.seal(source)?;
        }
        Ok(())
    }

    /// Paths of the files finished since the last call.
    pub fn take_sealed(&mut self) -> Vec<PathBuf> {
        std::mem::take(&mut self.sealed)
    }

    fn open_file(&self, source: usize, hour: UtcHour, stamp: Stamp) -> io::Result<OpenFile> {
        let info = &self.slots[source].info;
        let dir = self.out_dir.join(&info.name).join(hour.date());
        fs::create_dir_all(&dir)?;

        // The open time in milliseconds keeps names unique across restarts.
        // If the clock stepped back onto an existing name, move forward.
        let mut open_ms = stamp.wall_ns.div_euclid(1_000_000);
        let (file, part_path, final_path) = loop {
            let stem = format!("{}-{}-{open_ms}", info.name, hour.label());
            let part_path = dir.join(format!("{stem}{PARTIAL_SUFFIX}"));
            let final_path = dir.join(format!("{stem}{FINISHED_SUFFIX}"));
            let taken =
                final_path.exists() || dir.join(format!("{stem}{TRUNCATED_SUFFIX}")).exists();
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&part_path)
            {
                Ok(file) if !taken => break (file, part_path, final_path),
                Ok(_) => fs::remove_file(&part_path)?,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
            open_ms += 1;
        };

        let mut encoder = zstd::Encoder::new(BufWriter::new(file), self.zstd_level)?;
        encoder.include_checksum(true)?;
        let header = json!({
            "event": "file_open",
            "format": rawlog::FORMAT_VERSION,
            "source": info.name,
            "url": info.url,
            "recorder_version": env!("CARGO_PKG_VERSION"),
            "process_start_wall_ns": self.process_start_wall_ns,
            "file_seq": self.slots[source].next_file_seq,
        });
        rawlog::write_record(
            &mut encoder,
            RecordKind::Meta,
            stamp.wall_ns,
            stamp.mono_ns,
            header.to_string().as_bytes(),
        )?;
        Ok(OpenFile {
            hour,
            part_path,
            final_path,
            encoder,
        })
    }

    /// Finishes the open file of `source`, if any: ends the compressed
    /// frame, syncs it to disk and gives it its final name.
    fn seal(&mut self, source: usize) -> io::Result<()> {
        let Some(open) = self.slots[source].open.take() else {
            return Ok(());
        };
        let file = open
            .encoder
            .finish()?
            .into_inner()
            .map_err(io::IntoInnerError::into_error)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&open.part_path, &open.final_path)?;
        self.sealed.push(open.final_path);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use hl_codec::rawlog::{Record, RecordReader};

    use super::*;

    const HOUR_NS: i64 = 3_600 * 1_000_000_000;
    /// 2026-11-24 10:00:00 UTC.
    const TEN_OCLOCK_NS: i64 = 1_795_514_400 * 1_000_000_000;

    fn temp_dir(test: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("hl-recorder-writer-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn sources() -> Vec<SourceInfo> {
        vec![
            SourceInfo {
                name: "hl-main".to_owned(),
                url: "wss://hl.test/ws".to_owned(),
            },
            SourceInfo {
                name: "binance".to_owned(),
                url: "wss://bn.test/ws".to_owned(),
            },
        ]
    }

    fn writer(dir: &Path) -> LogWriter {
        LogWriter::new(dir, 3, 42, sources()).unwrap().0
    }

    fn stamp(wall_ns: i64) -> Stamp {
        Stamp {
            wall_ns,
            mono_ns: 7,
        }
    }

    fn files_under(dir: &Path) -> Vec<String> {
        let mut found = Vec::new();
        let mut pending = vec![dir.to_owned()];
        while let Some(next) = pending.pop() {
            for entry in fs::read_dir(next).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    pending.push(path);
                } else {
                    found.push(path.strip_prefix(dir).unwrap().to_str().unwrap().to_owned());
                }
            }
        }
        found.sort();
        found
    }

    fn read(path: &Path) -> Vec<Record> {
        let decoder = zstd::Decoder::new(File::open(path).unwrap()).unwrap();
        RecordReader::new(decoder).map(Result::unwrap).collect()
    }

    #[test]
    fn writes_a_header_then_records_and_seals_on_finish() {
        let dir = temp_dir("basic");
        let mut writer = writer(&dir);
        let at = TEN_OCLOCK_NS + 15 * 60 * 1_000_000_000 + 123_000_000;
        writer
            .write(0, RecordKind::RecvText, stamp(at), br#"{"channel":"bbo"}"#)
            .unwrap();
        writer
            .write(0, RecordKind::RecvBinary, stamp(at + 1), &[1, 2, 3])
            .unwrap();

        let name = format!("hl-main/20261124/hl-main-20261124T10-{}", at / 1_000_000);
        assert_eq!(files_under(&dir), [format!("{name}.hlrec.zst.part")]);

        writer.finish().unwrap();
        assert_eq!(files_under(&dir), [format!("{name}.hlrec.zst")]);
        assert_eq!(
            writer.take_sealed(),
            [dir.join(format!("{name}.hlrec.zst"))]
        );
        assert!(writer.take_sealed().is_empty());

        let records = read(&dir.join(format!("{name}.hlrec.zst")));
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].kind, RecordKind::Meta);
        let header: serde_json::Value = serde_json::from_slice(&records[0].payload).unwrap();
        assert_eq!(
            header,
            json!({
                "event": "file_open",
                "format": 1,
                "source": "hl-main",
                "url": "wss://hl.test/ws",
                "recorder_version": env!("CARGO_PKG_VERSION"),
                "process_start_wall_ns": 42,
                "file_seq": 0,
            })
        );
        assert_eq!(
            records[1],
            Record {
                kind: RecordKind::RecvText,
                recv_wall_ns: at,
                recv_mono_ns: 7,
                payload: br#"{"channel":"bbo"}"#.to_vec(),
            }
        );
        assert_eq!(records[2].payload, [1, 2, 3]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_record_in_a_new_hour_starts_a_new_file() {
        let dir = temp_dir("rotate");
        let mut writer = writer(&dir);
        let before_midnight = TEN_OCLOCK_NS + 14 * HOUR_NS - 1;
        writer
            .write(0, RecordKind::RecvText, stamp(before_midnight), b"old")
            .unwrap();
        writer
            .write(0, RecordKind::RecvText, stamp(before_midnight + 1), b"new")
            .unwrap();

        let files = files_under(&dir);
        assert_eq!(files.len(), 2);
        assert!(files[0].starts_with("hl-main/20261124/hl-main-20261124T23-"));
        assert!(
            files[0].ends_with(".hlrec.zst"),
            "the earlier hour is sealed"
        );
        assert!(files[1].starts_with("hl-main/20261125/hl-main-20261125T00-"));
        assert!(files[1].ends_with(".hlrec.zst.part"));

        assert_eq!(read(&dir.join(&files[0]))[1].payload, b"old");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn file_sequences_advance_across_two_rotations_independently_per_source() {
        let dir = temp_dir("file-sequences");
        let mut writer = writer(&dir);
        writer
            .write(0, RecordKind::RecvText, stamp(TEN_OCLOCK_NS), b"first")
            .unwrap();
        writer
            .write(
                0,
                RecordKind::RecvText,
                stamp(TEN_OCLOCK_NS + HOUR_NS),
                b"second",
            )
            .unwrap();
        writer
            .write(
                1,
                RecordKind::RecvText,
                stamp(TEN_OCLOCK_NS + HOUR_NS + 1),
                b"first",
            )
            .unwrap();

        writer.tick(TEN_OCLOCK_NS + 2 * HOUR_NS).unwrap();
        for source in 0..2 {
            writer
                .write(
                    source,
                    RecordKind::RecvText,
                    stamp(TEN_OCLOCK_NS + 2 * HOUR_NS + 1),
                    b"after tick",
                )
                .unwrap();
        }
        writer.finish().unwrap();

        let headers: Vec<serde_json::Value> = files_under(&dir)
            .iter()
            .map(|file| {
                let records = read(&dir.join(file));
                serde_json::from_slice(&records[0].payload).unwrap()
            })
            .collect();
        let sequences: Vec<_> = headers
            .iter()
            .map(|header| {
                (
                    header["source"].as_str().unwrap(),
                    header["file_seq"].as_u64().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            sequences,
            [
                ("binance", 0),
                ("binance", 1),
                ("hl-main", 0),
                ("hl-main", 1),
                ("hl-main", 2),
            ]
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn tick_seals_files_whose_hour_has_ended_and_keeps_the_rest_open() {
        let dir = temp_dir("tick");
        let mut writer = writer(&dir);
        writer
            .write(0, RecordKind::RecvText, stamp(TEN_OCLOCK_NS + 5), b"a")
            .unwrap();
        writer
            .write(
                1,
                RecordKind::RecvText,
                stamp(TEN_OCLOCK_NS + HOUR_NS + 5),
                b"b",
            )
            .unwrap();

        writer.tick(TEN_OCLOCK_NS + HOUR_NS - 1).unwrap();
        assert!(
            writer.take_sealed().is_empty(),
            "the hour has not ended yet"
        );

        writer.tick(TEN_OCLOCK_NS + HOUR_NS + 10).unwrap();
        let sealed = writer.take_sealed();
        assert_eq!(sealed.len(), 1);
        assert!(sealed[0].to_str().unwrap().contains("hl-main-20261124T10-"));
        let open: Vec<_> = files_under(&dir)
            .into_iter()
            .filter(|f| f.ends_with(".part"))
            .collect();
        assert_eq!(open.len(), 1);
        assert!(open[0].starts_with("binance/20261124/binance-20261124T11-"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn flushed_records_are_readable_from_a_file_that_was_never_finished() {
        let dir = temp_dir("crash");
        let mut writer = writer(&dir);
        writer
            .write(0, RecordKind::RecvText, stamp(TEN_OCLOCK_NS), b"survives")
            .unwrap();
        writer.tick(TEN_OCLOCK_NS + 1).unwrap();
        writer
            .write(0, RecordKind::RecvText, stamp(TEN_OCLOCK_NS + 2), b"lost")
            .unwrap();
        // Simulate a kill: the file is neither flushed again nor finished.
        std::mem::forget(writer);

        let part = dir.join(&files_under(&dir)[0]);
        let decoder = zstd::Decoder::new(File::open(part).unwrap()).unwrap();
        let results: Vec<_> = RecordReader::new(decoder).collect();
        let readable: Vec<_> = results.iter().filter_map(|r| r.as_ref().ok()).collect();
        assert_eq!(readable.len(), 2, "header and the flushed record");
        assert_eq!(readable[1].payload, b"survives");
        assert!(results.last().unwrap().is_err(), "the stream ends abruptly");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn leftover_partial_files_are_marked_truncated_on_start() {
        let dir = temp_dir("recover");
        let day = dir.join("hl-main/20261124");
        fs::create_dir_all(&day).unwrap();
        fs::write(day.join("hl-main-20261124T09-1.hlrec.zst.part"), b"x").unwrap();
        fs::write(day.join("hl-main-20261124T08-1.hlrec.zst"), b"y").unwrap();
        fs::create_dir_all(dir.join("other/20261124")).unwrap();
        fs::write(
            dir.join("other/20261124/other-20261124T09-1.hlrec.zst.part"),
            b"z",
        )
        .unwrap();

        let (_writer, recovered) = LogWriter::new(&dir, 3, 0, sources()).unwrap();
        assert_eq!(
            recovered,
            [day.join("hl-main-20261124T09-1.truncated.hlrec.zst")]
        );
        assert_eq!(
            files_under(&dir),
            [
                "hl-main/20261124/hl-main-20261124T08-1.hlrec.zst",
                "hl-main/20261124/hl-main-20261124T09-1.truncated.hlrec.zst",
                "other/20261124/other-20261124T09-1.hlrec.zst.part",
            ],
            "only directories of configured sources are touched"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_name_that_is_already_taken_is_never_reused() {
        let dir = temp_dir("collide");
        let at = TEN_OCLOCK_NS + 5_000_000;
        let open_ms = at / 1_000_000;
        let day = dir.join("hl-main/20261124");
        fs::create_dir_all(&day).unwrap();
        fs::write(
            day.join(format!("hl-main-20261124T10-{open_ms}.hlrec.zst")),
            b"old",
        )
        .unwrap();
        fs::write(
            day.join(format!(
                "hl-main-20261124T10-{}.truncated.hlrec.zst",
                open_ms + 1
            )),
            b"old",
        )
        .unwrap();

        let mut writer = writer(&dir);
        writer
            .write(0, RecordKind::RecvText, stamp(at), b"new")
            .unwrap();
        writer.finish().unwrap();

        let sealed = writer.take_sealed();
        assert_eq!(
            sealed,
            [day.join(format!("hl-main-20261124T10-{}.hlrec.zst", open_ms + 2))]
        );
        assert_eq!(
            fs::read(day.join(format!("hl-main-20261124T10-{open_ms}.hlrec.zst"))).unwrap(),
            b"old"
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}
