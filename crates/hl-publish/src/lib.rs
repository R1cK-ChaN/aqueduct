//! Offline sealing of one staged day of public recordings (RFC 0010 D5).

mod date;
mod inputs;
mod manifest;
mod output;
mod recording;
mod staging;

use std::collections::BTreeSet;
use std::io::Write;
use std::path::PathBuf;

use date::Day;
use hl_recorder::public_v1;
use serde_json::Value;

/// Local inputs and output directory for one UTC recording day.
#[derive(Debug)]
pub struct SealOptions {
    /// UTC day as YYYY-MM-DD.
    pub day: String,
    /// Directory containing the recorder's source and day directories.
    pub staging_dir: PathBuf,
    /// Directory in which to install manifest.json.
    pub output_dir: PathBuf,
    /// UTC expiry date as YYYY-MM-DD, on or after the recording day.
    pub expires: String,
    /// UTF-8 file containing one private substring per line.
    pub deny_list: Option<PathBuf>,
    /// JSON results of a completed quality run for this day.
    pub quality_results: Option<PathBuf>,
    /// Manifest of the immediately preceding calendar day.
    pub previous_manifest: Option<PathBuf>,
    /// Optional delistings as COIN=UTC-time.
    pub coverage_ends: Vec<String>,
}

/// Screen every file and write the daily manifest once completed quality results are available.
/// The report identifies directory entries by one-based position in sorted basename order.
/// Errors use fixed messages. An empty error suppresses a diagnostic matching protected text.
pub fn seal(options: &SealOptions, report: impl Write) -> Result<(), &'static str> {
    let mut deny = inputs::deny_list(options.deny_list.as_deref())?;
    seal_day(options, report, &mut deny).map_err(|err| {
        let diagnostic = format!("{err}\n");
        if deny.iter().any(|s| diagnostic.contains(s)) {
            ""
        } else {
            err
        }
    })
}

fn seal_day(
    options: &SealOptions,
    mut report: impl Write,
    deny: &mut Vec<String>,
) -> Result<(), &'static str> {
    let day = Day::parse(&options.day).ok_or("invalid UTC day")?;
    let expires = Day::parse(&options.expires)
        .filter(|d| d.iso() >= day.iso())
        .ok_or("invalid expiry date")?;
    let ends = inputs::coverage_ends(&options.coverage_ends, &day)?;
    let previous = inputs::previous(options.previous_manifest.as_deref(), &day, deny)?;
    let mut rejected = BTreeSet::new();
    let entries = staging::inspect_day(&options.staging_dir, &day, deny, &mut rejected);
    deny.extend(rejected);
    let mut entries = entries?;
    let mut manifest = manifest::assemble(
        &day,
        expires.iso(),
        &mut entries,
        previous.as_ref(),
        ends,
        Value::Null,
    );

    let mut lines = String::new();
    for (index, entry) in entries.iter().enumerate() {
        let number = index + 1;
        let line = match &entry.check {
            Ok(_) => format!(
                "accepted\t{number}\t{}\n",
                entry.name.as_deref().ok_or("missing recording name")?
            ),
            Err(reason) => format!("withheld\t{number}\t{reason}\n"),
        };
        lines.push_str(&line);
    }
    if deny.iter().any(|s| lines.contains(s)) {
        return Err("day not ready: report failed disclosure check");
    }
    report
        .write_all(lines.as_bytes())
        .map_err(|_| "unable to write report")?;
    report.flush().map_err(|_| "unable to write report")?;
    manifest["quality"] = inputs::quality(options.quality_results.as_deref(), &day, deny)?;
    let mut json =
        serde_json::to_string_pretty(&manifest).map_err(|_| "unable to encode manifest")?;
    json.push('\n');
    if recording::json_contains_denied(&json, deny) {
        return Err("day not ready: manifest failed disclosure check");
    }
    output::write_manifest(
        &options.output_dir,
        &options.staging_dir.join(public_v1::SOURCE_NAME),
        json.as_bytes(),
    )
    .map_err(|_| "unable to install manifest in output directory")
}
