//! Command line for the recorder.

use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use hl_recorder::{Config, inspect, open_recording};

const USAGE: &str = "\
usage:
  hl-recorder run <config.toml>      record until SIGTERM or SIGINT
  hl-recorder check <config.toml>    validate a config and list what it subscribes to
  hl-recorder inspect <file>...      summarise recording files
  hl-recorder dump <file> [limit]    print records as text, one per line";

type CommandResult = Result<(), Box<dyn std::error::Error>>;

fn load_config(path: &str) -> Result<Config, Box<dyn std::error::Error>> {
    let text = std::fs::read_to_string(path).map_err(|error| format!("{path}: {error}"))?;
    Ok(Config::from_toml(&text).map_err(|error| format!("{path}: {error}"))?)
}

fn run(path: &str) -> CommandResult {
    let config = load_config(path)?;
    let shutdown = Arc::new(AtomicBool::new(false));
    for signal in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
        signal_hook::flag::register(signal, Arc::clone(&shutdown))?;
    }
    Ok(hl_recorder::run(&config, &shutdown)?)
}

fn check(path: &str) -> CommandResult {
    let config = load_config(path)?;
    let mut out = io::stdout().lock();
    writeln!(out, "out_dir: {}", config.out_dir.display())?;
    for source in &config.sources {
        let subscriptions = source.subscriptions();
        writeln!(
            out,
            "{}: {} ({} subscriptions)",
            source.name,
            source.url,
            subscriptions.len()
        )?;
        if let Some(policy) = source.send_policy() {
            writeln!(out, "  send_policy: {policy}")?;
        }
        for subscription in subscriptions {
            writeln!(out, "  {subscription}")?;
        }
    }
    Ok(())
}

fn inspect_files(paths: &[&str]) -> CommandResult {
    let mut out = io::stdout().lock();
    for path in paths {
        writeln!(out, "{path}")?;
        writeln!(out, "{}", inspect(Path::new(path))?)?;
    }
    Ok(())
}

fn dump(path: &str, limit: usize) -> CommandResult {
    const PREVIEW_CHARS: usize = 300;
    let mut out = io::stdout().lock();
    for record in open_recording(Path::new(path))?.take(limit) {
        let record = record?;
        let text = String::from_utf8_lossy(&record.payload);
        let preview: String = text.chars().take(PREVIEW_CHARS).collect();
        writeln!(
            out,
            "{} {} {:?} {}",
            record.recv_wall_ns,
            record.recv_mono_ns,
            record.kind,
            preview.escape_debug()
        )?;
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["run", path] => run(path),
        ["check", path] => check(path),
        ["inspect", paths @ ..] if !paths.is_empty() => inspect_files(paths),
        ["dump", path] => dump(path, usize::MAX),
        ["dump", path, limit] => match limit.parse() {
            Ok(limit) => dump(path, limit),
            Err(_) => Err(format!("limit {limit:?} is not a number").into()),
        },
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        // Output piped into a command that stopped reading is not a failure.
        Err(error)
            if error
                .downcast_ref::<io::Error>()
                .is_some_and(|error| error.kind() == io::ErrorKind::BrokenPipe) =>
        {
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("hl-recorder: {error}");
            ExitCode::FAILURE
        }
    }
}
