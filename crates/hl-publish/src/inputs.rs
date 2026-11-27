//! Operator-supplied inputs to the public manifest.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use hl_recorder::public_v1;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{date::Day, manifest::Previous, recording::json_contains_denied};

pub(crate) fn deny_list(path: Option<&Path>) -> Result<Vec<String>, &'static str> {
    let Some(path) = path else {
        return Ok(Vec::new());
    };
    let text = fs::read_to_string(path).map_err(|_| "unreadable deny-list")?;
    Ok(text
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

pub(crate) fn coverage_ends(values: &[String], day: &Day) -> Result<Value, &'static str> {
    let mut ends = BTreeMap::new();
    for value in values {
        let (coin, time) = value.split_once('=').ok_or("invalid coverage end")?;
        if !public_v1::COINS.contains(&coin)
            || !day.contains_utc_time(time)
            || ends.insert(coin, time).is_some()
        {
            return Err("invalid coverage end");
        }
    }
    Ok(Value::Array(
        ends.into_iter()
            .map(|(coin, time)| json!({"coin":coin,"end_utc":time}))
            .collect(),
    ))
}

pub(crate) fn previous(
    path: Option<&Path>,
    day: &Day,
    deny: &[String],
) -> Result<Option<Previous>, &'static str> {
    let Some(path) = path else {
        return Ok(None);
    };
    let text = fs::read_to_string(path).map_err(|_| "unreadable previous manifest")?;
    if json_contains_denied(&text, deny) {
        return Err("previous manifest failed disclosure check");
    }
    Previous::parse(&text, &day.previous().ok_or("invalid previous day")?).map(Some)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Quality {
    day: String,
    complete: bool,
    loader_revision: String,
    checks: Vec<Check>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Check {
    name: String,
    subject: String,
    status: Status,
    compared: u64,
    violations: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Status {
    Pass,
    Fail,
    Skip,
    Info,
}

pub(crate) fn quality(
    path: Option<&Path>,
    day: &Day,
    deny: &[String],
) -> Result<Value, &'static str> {
    let path = path.ok_or("day not ready: quality results required")?;
    let text = fs::read_to_string(path).map_err(|_| "day not ready: unreadable quality results")?;
    let quality: Quality =
        serde_json::from_str(&text).map_err(|_| "day not ready: invalid quality results")?;
    if !quality.complete || quality.day != day.iso() || quality.checks.is_empty() {
        return Err("day not ready: incomplete or wrong-day quality results");
    }
    let label = |s: &str| !s.trim().is_empty() && !s.chars().any(char::is_control);
    if !label(&quality.loader_revision)
        || quality
            .checks
            .iter()
            .any(|c| !label(&c.name) || !label(&c.subject))
    {
        return Err("day not ready: invalid quality labels");
    }
    if json_contains_denied(&text, deny) {
        return Err("day not ready: quality results failed disclosure check");
    }
    serde_json::to_value(quality).map_err(|_| "day not ready: invalid quality results")
}
