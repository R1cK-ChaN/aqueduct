//! Command line for one paper session.

use std::fs;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use hl_live::{PaperConfig, run_paper};

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let [command, flag, path] = args.as_slice() else {
        return usage();
    };
    if (command != "paper" && command != "check") || flag != "--config" {
        return usage();
    }
    let run = || -> Result<(), Box<dyn std::error::Error>> {
        let config = PaperConfig::from_toml(&fs::read_to_string(path)?)?;
        if command == "check" {
            println!("paper config OK");
            return Ok(());
        }
        let shutdown = Arc::new(AtomicBool::new(false));
        for signal in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
            signal_hook::flag::register(signal, Arc::clone(&shutdown))?;
        }
        let dir = run_paper(&config, &shutdown)?;
        println!("{}", dir.join("session.jsonl").display());
        Ok(())
    };
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("hl-live: {error}");
            ExitCode::FAILURE
        }
    }
}

fn usage() -> ExitCode {
    eprintln!("usage: hl-live <paper|check> --config <file.toml>");
    ExitCode::from(2)
}
