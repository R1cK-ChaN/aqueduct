//! Command line for the offline public recording checker.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{self, Write};
use std::process::ExitCode;

use hl_publish::{SealOptions, seal};

const HELP: &str = "\
Usage: hl-publish seal --day YYYY-MM-DD --staging-dir ROOT --output-dir OUT --expires YYYY-MM-DD
                       [--quality-results FILE] [--deny-list FILE] [--previous-manifest FILE]
                       [--coverage-end COIN=YYYY-MM-DDTHH:MM:SS[.fraction]Z]...

Read ROOT/hl-public/YYYYMMDD/ and write OUT/manifest.json after all file and quality checks.
Omit --quality-results to inspect the files and report that the day is not ready.
--previous-manifest takes the preceding UTC day's manifest. --coverage-end can repeat.
";

fn options(mut args: impl Iterator<Item = OsString>) -> Result<SealOptions, &'static str> {
    let mut fields = BTreeMap::new();
    let mut coverage_ends = Vec::new();
    while let Some(key) = args.next() {
        let value = args.next().ok_or("missing option value")?;
        if key == "--coverage-end" {
            coverage_ends.push(value.into_string().map_err(|_| "invalid coverage end")?);
            continue;
        }
        let key = key.into_string().map_err(|_| "unknown option")?;
        match key.as_str() {
            "--day"
            | "--staging-dir"
            | "--output-dir"
            | "--expires"
            | "--quality-results"
            | "--deny-list"
            | "--previous-manifest" => (),
            _ => return Err("unknown option"),
        }
        if fields.insert(key, value).is_some() {
            return Err("duplicate option");
        }
    }
    let mut required = |key: &str| fields.remove(key).ok_or("missing required option");
    Ok(SealOptions {
        day: required("--day")?
            .into_string()
            .map_err(|_| "invalid UTC day")?,
        staging_dir: required("--staging-dir")?.into(),
        output_dir: required("--output-dir")?.into(),
        expires: required("--expires")?
            .into_string()
            .map_err(|_| "invalid expiry date")?,
        quality_results: fields.remove("--quality-results").map(Into::into),
        deny_list: fields.remove("--deny-list").map(Into::into),
        previous_manifest: fields.remove("--previous-manifest").map(Into::into),
        coverage_ends,
    })
}

fn run() -> Result<(), &'static str> {
    let mut args = std::env::args_os().skip(1);
    match args.next().as_deref().and_then(|a| a.to_str()) {
        Some("--help" | "-h") => (),
        Some("seal") => {
            let mut args = args.peekable();
            if !args.peek().is_some_and(|a| a == "--help" || a == "-h") {
                return seal(&options(args)?, io::stdout().lock());
            }
        }
        _ => return Err("expected hl-publish seal (see --help)"),
    }
    io::stdout()
        .lock()
        .write_all(HELP.as_bytes())
        .map_err(|_| "unable to write help")
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            if !message.is_empty() {
                let _ = writeln!(io::stderr().lock(), "{message}");
            }
            ExitCode::FAILURE
        }
    }
}
