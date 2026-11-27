//! Command line for the loader.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clickhouse::Row;
use hl_loader::quality::{CheckKind, CheckOutcome, run_day};
use hl_loader::{Db, DbError, InputKind, LoadError, LoadOptions, Outcome, load_files, now_ms};
use serde::Deserialize;
use serde_json::json;

const USAGE: &str = "\
usage:
  hl-loader migrate                              create or update the tables
  hl-loader load rawlog [--force] [--no-carry] <file>...
                                                 load recordings in chain order
  hl-loader load <kind> [--force] <file>...      load archives; kind is archive-l2 or node-fills
  hl-loader status                               table sizes and recent loads
  hl-loader check <YYYY-MM-DD>                   recompute quality statistics and run the
                                                 daily checks for one UTC day; exit 1 on an alarm

`load` and `check` first compare the migrations recorded in the database with
the ones built into this loader, and exit 3 without writing anything if they
differ.

Connection: HLMM_CLICKHOUSE_URL, HLMM_CLICKHOUSE_USER, HLMM_CLICKHOUSE_PASSWORD.";

/// Exit status when the database schema and this loader do not match.
const EXIT_SCHEMA_MISMATCH: u8 = 3;

type CommandResult = Result<bool, Box<dyn std::error::Error>>;

async fn migrate(db: &Db) -> CommandResult {
    let applied = db.migrate(now_ms()).await?;
    println!("{}", json!({ "applied": applied }));
    Ok(true)
}

fn report_file(path: &Path, result: Result<Outcome, LoadError>) -> bool {
    let file = path.display().to_string();
    let ok = !matches!(result, Err(_) | Ok(Outcome::WaitingForPredecessor { .. }));
    let line = match result {
        Ok(Outcome::Loaded {
            ingest_id,
            rows,
            counters,
        }) => {
            let rows: serde_json::Map<String, serde_json::Value> = rows
                .into_iter()
                .filter(|(_, count)| *count > 0)
                .map(|(table, count)| (table.to_owned(), json!(count)))
                .collect();
            json!({ "file": file, "status": "loaded", "ingest_id": ingest_id, "rows": rows, "counters": counters })
        }
        Ok(Outcome::AlreadyLoaded) => json!({ "file": file, "status": "already_loaded" }),
        Ok(Outcome::WaitingForPredecessor {
            chain,
            missing_file_seq,
        }) => json!({
            "file": file, "status": "waiting_for_predecessor", "source": chain.source,
            "process_start_wall_ns": chain.process_start_wall_ns,
            "missing_file_seq": missing_file_seq,
        }),
        Ok(Outcome::Unsupported(reason)) => {
            json!({ "file": file, "status": "skipped", "reason": reason })
        }
        Err(error) => {
            json!({ "file": file, "status": "failed", "error": error.to_string() })
        }
    };
    println!("{line}");
    ok
}

fn load_args(kind: InputKind, args: &[&str]) -> Result<(LoadOptions, Vec<PathBuf>), String> {
    let mut options = LoadOptions::default();
    let mut remaining = args;
    while let Some((&arg, rest)) = remaining.split_first() {
        match arg {
            "--force" => options.force = true,
            "--no-carry" if kind == InputKind::Rawlog => options.no_carry = true,
            "--" => {
                remaining = rest;
                break;
            }
            "--no-carry" => return Err("--no-carry only applies to recordings".to_owned()),
            option if option.starts_with("--") => {
                return Err(format!("unknown load option {option}"));
            }
            _ => break,
        }
        remaining = rest;
    }
    if remaining.is_empty() {
        return Err("no files given".to_owned());
    }
    Ok((options, remaining.iter().map(PathBuf::from).collect()))
}

async fn load(db: &Db, kind: InputKind, options: LoadOptions, files: &[PathBuf]) -> CommandResult {
    let mut all_ok = true;
    load_files(db, kind, files, options, |path, result| {
        all_ok &= report_file(path, result);
    })
    .await;
    Ok(all_ok)
}

#[derive(Debug, Row, Deserialize)]
struct TableSize {
    table: String,
    rows: u64,
    compressed: u64,
    uncompressed: u64,
}

#[derive(Debug, Row, Deserialize)]
struct LoadSummary {
    source: String,
    status: String,
    files: u64,
    rows: u64,
    bytes: u64,
}

async fn status(db: &Db) -> CommandResult {
    let sizes: Vec<TableSize> = db
        .client()
        .query(
            "SELECT table, sum(rows) AS rows, sum(data_compressed_bytes) AS compressed, \
             sum(data_uncompressed_bytes) AS uncompressed FROM system.parts \
             WHERE database = 'hl' AND active GROUP BY table ORDER BY table",
        )
        .fetch_all()
        .await?;
    println!(
        "{:<14} {:>12} {:>14} {:>14} {:>6}",
        "table", "rows", "compressed", "uncompressed", "ratio"
    );
    for size in sizes {
        let ratio = size.uncompressed as f64 / size.compressed.max(1) as f64;
        println!(
            "{:<14} {:>12} {:>14} {:>14} {:>6.1}",
            size.table, size.rows, size.compressed, size.uncompressed, ratio
        );
    }
    let loads: Vec<LoadSummary> = db
        .client()
        .query(
            "SELECT source, toString(status) AS status, count() AS files, sum(rows) AS rows, \
             sum(bytes) AS bytes FROM hl.ingest_log GROUP BY source, status ORDER BY source, status",
        )
        .fetch_all()
        .await?;
    println!();
    println!(
        "{:<12} {:<7} {:>6} {:>12} {:>14}",
        "source", "status", "files", "rows", "file bytes"
    );
    for load in loads {
        println!(
            "{:<12} {:<7} {:>6} {:>12} {:>14}",
            load.source, load.status, load.files, load.rows, load.bytes
        );
    }
    Ok(true)
}

#[derive(Debug, Row, Deserialize)]
struct InstrumentName {
    instrument_id: u32,
    coin: String,
}

fn print_outcome(outcome: &CheckOutcome, names: &std::collections::BTreeMap<u32, String>) {
    let check = outcome.check;
    let name = |id: u32| {
        if id == 0 {
            "-".to_owned()
        } else {
            names.get(&id).cloned().unwrap_or_else(|| id.to_string())
        }
    };
    if outcome.results.is_empty() {
        println!("{:<5} {:<24} nothing to check", "skip", check.name);
        return;
    }
    let mut subjects: Vec<&str> = outcome
        .results
        .iter()
        .map(|row| row.subject.as_str())
        .collect();
    subjects.dedup();
    for subject in subjects {
        let rows: Vec<_> = outcome
            .results
            .iter()
            .filter(|row| row.subject == subject)
            .collect();
        let compared: u64 = rows.iter().map(|row| row.compared).sum();
        let violations: u64 = rows.iter().map(|row| row.violations).sum();
        let failing: Vec<String> = rows
            .iter()
            .filter(|row| !check.kind.passes(row.compared, row.violations))
            .map(|row| name(row.instrument_id))
            .collect();
        let verdict = match check.kind {
            CheckKind::Info => "info",
            CheckKind::Alarm { .. } if failing.is_empty() => "pass",
            CheckKind::Alarm { .. } => "FAIL",
        };
        let instruments = rows.iter().filter(|row| row.instrument_id != 0).count();
        let scope = if instruments > 0 {
            format!("{instruments} instruments")
        } else {
            String::new()
        };
        println!(
            "{verdict:<5} {:<24} {subject:<24} {scope:<16} {compared:>10} compared {violations:>8} violations",
            check.name
        );
        if instruments > 0 && !failing.is_empty() {
            println!("      failing: {}", failing.join(", "));
        }
        for row in rows
            .iter()
            .filter(|row| !row.detail.is_empty() && row.detail != "{}")
        {
            if rows.len() <= 12 || !check.kind.passes(row.compared, row.violations) {
                println!("      {}: {}", name(row.instrument_id), row.detail);
            }
        }
    }
}

async fn check(db: &Db, day: &str) -> CommandResult {
    let outcomes = run_day(db, day, now_ms()).await?;
    let names: Vec<InstrumentName> = db
        .client()
        .query("SELECT instrument_id, coin FROM hl.instruments FINAL")
        .fetch_all()
        .await?;
    let names = names
        .into_iter()
        .map(|row| (row.instrument_id, row.coin))
        .collect();
    println!("data-quality checks for {day} (UTC)");
    for outcome in &outcomes {
        print_outcome(outcome, &names);
    }
    let failed: Vec<&str> = outcomes
        .iter()
        .filter(|outcome| !outcome.passed())
        .map(|outcome| outcome.check.name)
        .collect();
    if failed.is_empty() {
        println!("all alarm checks passed");
        Ok(true)
    } else {
        println!("alarms: {}", failed.join(", "));
        Ok(false)
    }
}

async fn run(args: &[String]) -> CommandResult {
    let db = Db::from_env();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["migrate"] => migrate(&db).await,
        ["status"] => status(&db).await,
        ["check", day] => {
            db.require_current_schema().await?;
            let passed = check(&db, day).await?;
            if !passed {
                return Err("one or more data-quality checks failed".into());
            }
            Ok(true)
        }
        ["load", kind, rest @ ..] => {
            let kind =
                InputKind::from_name(kind).ok_or_else(|| format!("unknown kind {kind:?}"))?;
            let (options, files) = load_args(kind, rest)?;
            db.require_current_schema().await?;
            load(&db, kind, options, &files).await
        }
        _ => {
            eprintln!("{USAGE}");
            Ok(false)
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args).await {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(2),
        Err(error) => {
            eprintln!("hl-loader: {error}");
            // A schema mismatch does not go away by retrying another file, so
            // callers that load file by file can stop at the first one.
            match error.downcast_ref::<DbError>() {
                Some(DbError::SchemaMismatch(_) | DbError::ChangedMigration(_)) => {
                    ExitCode::from(EXIT_SCHEMA_MISMATCH)
                }
                _ => ExitCode::FAILURE,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rawlog_accepts_either_flag_order_and_archives_reject_no_carry() {
        for flags in [
            ["--force", "--no-carry", "hour"],
            ["--no-carry", "--force", "hour"],
        ] {
            let (options, files) = load_args(InputKind::Rawlog, &flags).unwrap();
            assert!(options.force && options.no_carry);
            assert_eq!(files, [PathBuf::from("hour")]);
        }
        assert!(load_args(InputKind::Rawlog, &["--no-carry"]).is_err());
        assert!(load_args(InputKind::ArchiveL2, &["--no-carry", "hour"]).is_err());
    }
}
