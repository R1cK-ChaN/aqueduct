//! Import order, decoder state and the ingest log for a load command.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use data_encoding::HEXLOWER;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::rows::{IngestLogRow, RawlogStateRow, STATUS_DONE, STATUS_FAILED};
use crate::sources::{self, RawlogChain, RawlogMetadata, RawlogState};
use crate::{Db, InputKind, LoadError, Outcome, SourceError, input, now_ms, now_ns, store};

/// Options shared by the files in one load command.
#[derive(Clone, Copy, Debug, Default)]
pub struct LoadOptions {
    /// Reload a previously successful file under a new ingest id.
    pub force: bool,
    /// Allow a numbered recording to start empty if its predecessor is missing.
    pub no_carry: bool,
}

struct InputFile<'a> {
    path: &'a Path,
    header: Result<Option<RawlogMetadata>, SourceError>,
}

impl<'a> InputFile<'a> {
    fn new(kind: InputKind, path: &'a Path) -> Self {
        let header = if kind == InputKind::Rawlog && !is_partial(path) {
            input::open_decompressed(path)
                .map_err(SourceError::from)
                .and_then(sources::read_rawlog_metadata)
                .map(Some)
        } else {
            Ok(None)
        };
        Self { path, header }
    }

    fn order(&self) -> Option<(&RawlogChain, Option<u64>, u64, i64)> {
        let header = self.header.as_ref().ok()?.as_ref()?;
        Some((
            header.chain.as_ref()?,
            header.file_seq,
            header.opened_mono_ns,
            header.opened_wall_ns,
        ))
    }

    fn legacy_chain(&self) -> Option<&RawlogChain> {
        match self.order() {
            Some((chain, None, ..)) => Some(chain),
            _ => None,
        }
    }
}

fn is_partial(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "part")
}

#[derive(Default)]
struct LegacyCarry {
    end: Option<RawlogState>,
    // An overlapping glob can supply the same file twice in one command.
    // Applying its positional book diffs twice would corrupt the next hour.
    files: BTreeSet<String>,
}

/// Loads one file with the normal predecessor requirement.
pub async fn load_file(
    db: &Db,
    kind: InputKind,
    path: &Path,
    force: bool,
) -> Result<Outcome, LoadError> {
    let loader = Loader {
        db,
        kind,
        options: LoadOptions {
            force,
            no_carry: false,
        },
    };
    loader
        .file(
            InputFile::new(kind, path),
            &mut LegacyCarry::default(),
            false,
        )
        .await
}

/// Loads files in recording-chain order and reports each outcome as it finishes.
/// Archives retain argument order. Only headers and one legacy decoder state
/// are kept between files.
pub async fn load_files(
    db: &Db,
    kind: InputKind,
    paths: &[PathBuf],
    options: LoadOptions,
    mut report: impl FnMut(&Path, Result<Outcome, LoadError>),
) {
    let loader = Loader { db, kind, options };
    let mut files: Vec<_> = paths
        .iter()
        .map(|path| InputFile::new(kind, path))
        .collect();
    if kind == InputKind::Rawlog {
        files.sort_by(|a, b| a.order().cmp(&b.order()));
    }
    let mut files = files.into_iter().peekable();
    let mut legacy = LegacyCarry::default();
    let mut failed_legacy: Option<&Path> = None;
    while let Some(file) = files.next() {
        let path = file.path;
        let continues = file
            .legacy_chain()
            .is_some_and(|chain| files.peek().and_then(InputFile::legacy_chain) == Some(chain));
        let result = match failed_legacy {
            Some(previous) => Err(SourceError::Invalid(format!(
                "preceding legacy file {} failed in this command",
                previous.display()
            ))
            .into()),
            None => loader.file(file, &mut legacy, continues).await,
        };
        if continues && result.is_err() {
            failed_legacy.get_or_insert(path);
        }
        if !continues {
            legacy = LegacyCarry::default();
            failed_legacy = None;
        }
        report(path, result);
    }
}

struct Loader<'a> {
    db: &'a Db,
    kind: InputKind,
    options: LoadOptions,
}

impl Loader<'_> {
    async fn file(
        &self,
        file: InputFile<'_>,
        legacy: &mut LegacyCarry,
        continues: bool,
    ) -> Result<Outcome, LoadError> {
        if is_partial(file.path) {
            return Ok(Outcome::Unsupported("still being written".to_owned()));
        }
        let (sha256, bytes) = input::sha256_file(file.path)?;
        let mut log = IngestLogRow {
            ingest_id: u64::try_from(now_ns()).unwrap_or(0),
            source: self.kind.source().to_owned(),
            file_name: file
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            sha256,
            bytes,
            rows: 0,
            ts_min: 0,
            ts_max: 0,
            loader_version: env!("CARGO_PKG_VERSION").to_owned(),
            started_at: now_ms(),
            finished_at: 0,
            status: STATUS_FAILED,
            details: String::new(),
            error: String::new(),
        };

        let result = self.ingest(file, &mut log, legacy, continues).await;
        log.finished_at = now_ms();
        match &result {
            Ok(Outcome::Loaded { .. }) => {
                log.status = STATUS_DONE;
                self.db.log(&log).await?;
            }
            Err(LoadError::Source(SourceError::Unsupported(reason))) => {
                return Ok(Outcome::Unsupported(reason.clone()));
            }
            Err(error) => {
                log.error = error.to_string();
                // The database itself may be unavailable.
                let _ = self.db.log(&log).await;
            }
            Ok(_) => {}
        }
        result
    }

    async fn ingest(
        &self,
        file: InputFile<'_>,
        log: &mut IngestLogRow,
        legacy: &mut LegacyCarry,
        continues: bool,
    ) -> Result<Outcome, LoadError> {
        if file.legacy_chain().is_some() && !legacy.files.insert(log.sha256.clone()) {
            return Ok(Outcome::AlreadyLoaded);
        }
        let header = file.header?;
        let numbered = header
            .as_ref()
            .and_then(|h| h.chain.as_ref().zip(h.file_seq));
        let mut start = legacy.end.take();
        let mut missing_predecessor = false;
        if let Some((chain, seq)) = numbered
            && let Some(previous) = seq.checked_sub(1)
        {
            match self.db.rawlog_state(chain, previous).await? {
                Some(row) => start = Some(RawlogState::from_json(&row.state_json)?),
                None if self.options.no_carry => missing_predecessor = true,
                None => {
                    return Ok(Outcome::WaitingForPredecessor {
                        chain: chain.clone(),
                        missing_file_seq: previous,
                    });
                }
            }
        }
        let done = if self.options.force {
            None
        } else {
            self.db.last_done_ingest(&log.sha256).await?
        };
        if let Some(ingest_id) = done {
            if let Some((chain, seq)) = numbered {
                self.check_completed_state(chain, seq, &log.sha256, ingest_id)
                    .await?;
            }
            if !continues {
                return Ok(Outcome::AlreadyLoaded);
            }
        }
        let token = insert_token(log, self.options.force, start.as_ref())?;
        let input = input::open_decompressed(file.path)?;
        let (batch, end) = match self.kind {
            InputKind::ArchiveL2 => (sources::read_archive_l2(input, log.ingest_id)?, None),
            InputKind::NodeFills => (sources::read_node_fills(input, log.ingest_id)?, None),
            InputKind::Rawlog => {
                let (batch, end) = sources::read_rawlog_with_state(input, log.ingest_id, start)?;
                (batch, Some(end))
            }
        };
        if done.is_some() {
            // A later legacy file still needs this file's end state.
            legacy.end = end;
            return Ok(Outcome::AlreadyLoaded);
        }

        log.rows = batch.rows();
        (log.ts_min, log.ts_max) = batch.ts_range.unwrap_or((0, 0));
        let mut details = json!({ "counters": batch.counters, "problems": batch.problems });
        if missing_predecessor {
            details["carry"] = json!("missing_predecessor");
        }
        log.details = details.to_string();
        let conflicts = store(self.db, &batch, &token, log.ingest_id).await?;
        if let Some(end) = end {
            if let Some((chain, seq)) = numbered {
                self.db
                    .store_rawlog_state(&RawlogStateRow {
                        source: chain.source.clone(),
                        process_start_wall_ns: chain.process_start_wall_ns,
                        file_seq: seq,
                        sha256: log.sha256.clone(),
                        state_json: end.to_json()?,
                        ingest_id: log.ingest_id,
                    })
                    .await?;
            } else if continues {
                legacy.end = Some(end);
            }
        }
        let rows = vec![
            ("l2_snapshot", batch.l2.len()),
            ("fills", batch.fills.len()),
            ("bbo", batch.bbo.len()),
            ("ws_trades", batch.ws_trades.len()),
        ];
        let mut counters = batch.counters;
        if conflicts > 0 {
            counters.insert("key_conflicts".to_owned(), conflicts);
            details["counters"] = json!(counters);
            log.details = details.to_string();
        }
        Ok(Outcome::Loaded {
            ingest_id: log.ingest_id,
            rows,
            counters,
        })
    }

    async fn check_completed_state(
        &self,
        chain: &RawlogChain,
        seq: u64,
        sha256: &str,
        ingest_id: u64,
    ) -> Result<(), LoadError> {
        if let Some(row) = self.db.rawlog_state(chain, seq).await?
            && row.sha256 == sha256
            && row.ingest_id == ingest_id
        {
            RawlogState::from_json(&row.state_json)?;
            return Ok(());
        }
        Err(LoadError::IncompleteState(format!(
            "ingest {ingest_id} is done but matching state is missing for source {:?}, process {}, file_seq {seq}. Reload with --force",
            chain.source, chain.process_start_wall_ns,
        )))
    }
}

fn insert_token(
    log: &IngestLogRow,
    force: bool,
    start: Option<&RawlogState>,
) -> Result<String, SourceError> {
    if force {
        return Ok(format!("{}:{}", log.sha256, log.ingest_id));
    }
    // A retry can find a predecessor that was unavailable on an earlier
    // attempt. Deduplicate only chunks decoded from the same starting state.
    if let Some(state) = start {
        let digest = Sha256::digest(state.to_json()?.as_bytes());
        return Ok(format!("{}:{}", log.sha256, HEXLOWER.encode(&digest)));
    }
    Ok(log.sha256.clone())
}
