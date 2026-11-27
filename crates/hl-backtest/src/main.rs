//! Command-line replay of one coin through the fixed-spread quoter.

mod output;
mod pnl;
mod summary;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt::Display;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::str::FromStr;
use std::time::{Duration, Instant};

use hl_backtest::recording::{Recording, read_recordings};
use hl_backtest::session::{ReplayComponents, check_session};
use hl_backtest::{build_identity, replay_session};
use hl_codec::hyperliquid::{Network, instrument_id};
use hl_run::session::SessionStart;
use hl_run::{replay, select_rules};
use hl_sim::{MODEL_NAME, SimConfig, SimGateway};
use hl_strategy::{FixedSpreadConfig, FixedSpreadQuoter, StrategyHost};
use hl_types::{MarketEventKind, Px, Qty};
use serde_json::json;
use summary::JsonObject;

const USAGE: &str = "\
usage: hl-backtest run --coin <coin> --half-spread <price> --qty <size>
           --order-latency-ms <n> --report-latency-ms <n> --maker-fee-bps <rate>
           [--network mainnet|testnet] [--min-notional <usd>]
           [--rules-file <path>] [--session-out <path>] <file>...
       hl-backtest run --check-session <session.jsonl>";

const STRATEGY_NAME: &str = "fixed_spread";
const STRATEGY_TAG: u8 = 1;

struct Command<'a> {
    coin: &'a str,
    network: Network,
    files: Vec<PathBuf>,
    session_out: Option<PathBuf>,
    rules_file: Option<PathBuf>,
    strategy: FixedSpreadConfig,
    sim: SimConfig,
    // Preserve the supplied decimal spellings in the JSON summary.
    summary: JsonObject,
}

fn number<T: FromStr>(flag: &str, text: &str) -> Result<T, String>
where
    T::Err: Display,
{
    text.parse().map_err(|error| format!("{flag}: {error}"))
}

fn parse(args: &[OsString]) -> Result<Command<'_>, String> {
    let mut args = args.iter();
    if args.next().and_then(|arg| arg.to_str()) != Some("run") {
        return Err("expected the run subcommand".into());
    }
    let mut flags = BTreeMap::new();
    let mut files = Vec::new();
    let mut session_out = None;
    let mut rules_file = None;
    while let Some(arg) = args.next() {
        let Some(flag) = arg.to_str().filter(|word| word.starts_with('-')) else {
            files.push(PathBuf::from(arg));
            continue;
        };
        match flag {
            "--session-out" | "--rules-file" => {
                let path = args
                    .next()
                    .filter(|p| !p.is_empty() && !p.as_encoded_bytes().starts_with(b"--"))
                    .ok_or_else(|| format!("missing value for {flag}"))?;
                let option = if flag == "--rules-file" {
                    &mut rules_file
                } else {
                    &mut session_out
                };
                if option.replace(PathBuf::from(path)).is_some() {
                    return Err(format!("duplicate option {flag}"));
                }
                continue;
            }
            "--" => {
                files.extend(args.map(PathBuf::from));
                break;
            }
            "--coin"
            | "--half-spread"
            | "--qty"
            | "--order-latency-ms"
            | "--report-latency-ms"
            | "--maker-fee-bps"
            | "--network"
            | "--min-notional" => {}
            _ => return Err(format!("unknown option {flag}")),
        }
        let value = args
            .next()
            .and_then(|arg| arg.to_str())
            .filter(|text| !text.is_empty() && !text.starts_with("--"))
            .ok_or_else(|| format!("missing value for {flag}"))?;
        if flags.insert(flag, value).is_some() {
            return Err(format!("duplicate option {flag}"));
        }
    }
    if files.is_empty() {
        return Err("no recording files given".into());
    }
    let required = |key: &str| {
        flags
            .get(key)
            .copied()
            .ok_or_else(|| format!("missing {key}"))
    };
    let coin = required("--coin")?;
    let network = match flags.get("--network") {
        Some(name) => {
            Network::from_short_name(name).ok_or("--network must be mainnet or testnet")?
        }
        None => Network::Mainnet,
    };
    let half_spread = required("--half-spread")?;
    let qty = required("--qty")?;
    let maker_fee_bps = required("--maker-fee-bps")?;
    // Simulation default from HL-27 in docs/venues/hyperliquid.md.
    let min_notional = flags.get("--min-notional").copied().unwrap_or("10");
    let strategy = FixedSpreadConfig {
        instrument: instrument_id(network, coin),
        half_spread: number::<Px>("--half-spread", half_spread)?,
        qty: number::<Qty>("--qty", qty)?,
    };
    if !strategy.half_spread.is_positive() || !strategy.qty.is_positive() {
        return Err("--half-spread and --qty must be positive".into());
    }
    let order_ms = number("--order-latency-ms", required("--order-latency-ms")?)?;
    let report_ms = number("--report-latency-ms", required("--report-latency-ms")?)?;
    let sim = SimConfig::from_parameters(&[
        (
            "order_latency_ns".into(),
            Duration::from_millis(order_ms).as_nanos().to_string(),
        ),
        (
            "report_latency_ns".into(),
            Duration::from_millis(report_ms).as_nanos().to_string(),
        ),
        ("maker_fee_bps".into(), maker_fee_bps.to_owned()),
        ("min_order_notional_usd".into(), min_notional.to_owned()),
    ])?;
    let summary = JsonObject::new([
        ("coin", json!(coin)),
        ("network", json!(network.short_name())),
        ("files", json!(files.len())),
        ("half_spread", json!(half_spread)),
        ("qty", json!(qty)),
        ("order_latency_ms", json!(order_ms)),
        ("report_latency_ms", json!(report_ms)),
        ("maker_fee_bps", json!(maker_fee_bps)),
        ("min_notional", json!(min_notional)),
    ]);
    Ok(Command {
        coin,
        network,
        files,
        session_out,
        rules_file,
        strategy,
        sim,
        summary,
    })
}

fn validate_ranges(
    recording: &Recording,
    strategy: FixedSpreadConfig,
    sim: SimConfig,
) -> Result<(), String> {
    // Check the limits of the fixed-point arithmetic used by the quoter and simulator.
    let spread = strategy.half_spread.as_e8();
    for event in &recording.events {
        if let MarketEventKind::Bbo(bbo) = &event.kind
            && let (Some(bid), Some(ask)) = (bbo.bid, bbo.ask)
        {
            let mid = bid.px.midpoint(ask.px).as_e8();
            if mid.checked_add(spread).is_none() || mid.checked_sub(spread).is_none() {
                return Err("quote price with this half spread exceeds the price range".into());
            }
        }
    }
    let latency_ns = (sim.order_latency + sim.report_latency).as_nanos();
    if let Some(last) = recording.events.last()
        && i128::from(last.ts_local.as_nanos()) + latency_ns as i128 > i128::from(i64::MAX)
    {
        return Err("report time with these latencies exceeds the timestamp range".into());
    }

    Ok(())
}

fn components(
    start: &SessionStart,
    recording: &Recording,
) -> Result<ReplayComponents<FixedSpreadQuoter>, String> {
    if start.strategy_name != STRATEGY_NAME {
        return Err(format!("unknown strategy_name {:?}", start.strategy_name));
    }
    if start.sim_model != MODEL_NAME {
        return Err(format!("unknown sim.model {:?}", start.sim_model));
    }
    let strategy = FixedSpreadConfig::from_parameters(start.instrument, &start.strategy)?;
    let config = SimConfig::from_parameters(&start.sim)?;
    validate_ranges(recording, strategy, config)?;
    let (sim, rules) = match start.rule_book().map_err(|error| error.to_string())? {
        Some(book) => (SimGateway::with_rules(config, book), start.rules.clone()),
        None => (SimGateway::new(config), Vec::new()),
    };
    Ok(ReplayComponents {
        host: StrategyHost::new(FixedSpreadQuoter::new(strategy), start.strategy_tag),
        sim,
        strategy_parameters: strategy.parameters(),
        sim_parameters: config.parameters(),
        rules,
    })
}

fn run(mut command: Command<'_>) -> Result<(), Box<dyn std::error::Error>> {
    let start = Instant::now();
    if let Some(path) = &command.session_out {
        output::require_unused(path)?;
    }
    let (mut sim, rules) = match &command.rules_file {
        Some(path) => {
            let text = fs::read_to_string(path);
            let selected = select_rules(path, text, command.network, command.coin)?;
            command.summary.append_rules(&selected.versions);
            (
                SimGateway::with_rules(command.sim, selected.book),
                selected.versions,
            )
        }
        None => {
            eprintln!("hl-backtest: no grid check applied");
            (SimGateway::new(command.sim), Vec::new())
        }
    };
    let recording = read_recordings(&command.files, command.network, command.coin)?;
    let read_decode_ms = start.elapsed().as_millis();
    if recording.stats.bbo == 0 {
        return Err(format!("no bbo event for coin {}", command.coin).into());
    }
    validate_ranges(&recording, command.strategy, command.sim)?;
    let mut host = StrategyHost::new(FixedSpreadQuoter::new(command.strategy), STRATEGY_TAG);
    let start = Instant::now();
    let stop = recording
        .stop_time
        .expect("a recording with a BBO has a complete record");
    let (trace, pnl, session) = if let Some(path) = &command.session_out {
        let build = build_identity();
        let start = SessionStart {
            commit: build.commit,
            dirty: build.dirty,
            driver: "backtest".into(),
            network: command.network,
            source: None,
            coin: command.coin.into(),
            instrument: command.strategy.instrument,
            process_start_wall_ns: None,
            strategy_name: STRATEGY_NAME.into(),
            strategy_tag: STRATEGY_TAG,
            strategy: command.strategy.parameters(),
            sim_model: MODEL_NAME.into(),
            sim: command.sim.parameters(),
            rules,
        };
        let manifest = output::manifest(&recording, path)?;
        let (session, trace, pnl) =
            replay_session(&recording, &start, manifest, &mut host, &mut sim)?;
        (trace, pnl, Some((path, session)))
    } else {
        let trace = replay(&mut host, &mut sim, &recording.events, stop)
            .map_err(|error| error.for_pair(command.network, command.coin))?;
        let pnl = hl_run::pnl::summarize(
            command.strategy.instrument,
            &recording.events,
            &trace.order_events,
        )?;
        (trace, pnl, None)
    };
    let replay_ms = start.elapsed().as_millis();

    summary::add_results(
        &mut command.summary,
        &recording,
        &trace,
        host.orders().len(),
        command.sim.report_latency,
    )?;
    command.summary.append_object("pnl", pnl::json(&pnl));
    if let Some((path, session)) = session {
        output::seal(path, &session)?;
    }
    writeln!(io::stdout().lock(), "{}", command.summary)?;
    eprintln!("read_decode_ms={read_decode_ms} replay_ms={replay_ms}");
    Ok(())
}

// Gregorian civil date of a UTC midnight, using the same March-based
// 400-year cycle as the recorder's UTC file labels.
fn utc_midnight(cutoff: hl_types::Timestamp) -> String {
    let days = cutoff.as_nanos().div_euclid(86_400_000_000_000);
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T00:00:00Z")
}

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if let [run, check, path] = args.as_slice()
        && run == "run"
        && check == "--check-session"
        && !path.is_empty()
        && !path.as_encoded_bytes().starts_with(b"--")
    {
        return match check_session(path.as_ref(), &build_identity(), components) {
            Ok(summary) => {
                if let Some(cutoff) = summary.checkpoint {
                    println!(
                        "checkpoint {} matches ({} rows)",
                        utc_midnight(cutoff),
                        summary.rows
                    );
                } else {
                    println!("session matches ({} rows)", summary.rows);
                }
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("hl-backtest: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let command = match parse(&args) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("hl-backtest: {error}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("hl-backtest: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basis_points_are_exact_in_simulator_units() {
        for (input, raw) in [
            ("1.50000", 15_000),
            ("-0.1", -1_000),
            ("0.0001", 1),
            ("10000", 100_000_000),
            ("-10000.0000", -100_000_000),
        ] {
            assert_eq!(fee_from_args(input).unwrap(), raw);
        }
        for invalid in [
            "0.00001",
            "1e-3",
            "1.",
            "",
            ".1",
            "0..1",
            "-10000.001",
            "10001",
        ] {
            assert!(fee_from_args(invalid).is_err(), "{invalid}");
        }
    }

    fn fee_from_args(value: &str) -> Result<i64, String> {
        let mut args = args();
        *args.last_mut().unwrap() = value.into();
        args.push("one.zst".into());
        Ok(parse(&args)?.sim.maker_fee.as_e8())
    }

    fn args() -> Vec<OsString> {
        "run --coin BTC --half-spread 2.00 --qty 0.020 --order-latency-ms 400 --report-latency-ms 250 --maker-fee-bps -0.1"
            .split_whitespace().map(OsString::from).collect()
    }

    #[test]
    fn accepts_explicit_network_minimum_and_literal_paths() {
        let mut input = args();
        input.extend(
            [
                "--network",
                "testnet",
                "--min-notional",
                "20.00",
                "--",
                "--one.zst",
            ]
            .map(OsString::from),
        );
        let command = parse(&input).unwrap();
        assert_eq!(command.network, Network::Testnet);
        assert_eq!(command.files, [PathBuf::from("--one.zst")]);
        assert_eq!(command.sim.min_order_notional, "20".parse().unwrap());
        let summary: serde_json::Value =
            serde_json::from_str(&command.summary.to_string()).unwrap();
        assert_eq!(summary["half_spread"], "2.00");
        assert_eq!(summary["min_notional"], "20.00");
    }

    #[test]
    fn rejects_incomplete_and_invalid_options_before_opening_files() {
        assert!(parse(&args()).is_err());
        for flag in [
            "--coin",
            "--half-spread",
            "--qty",
            "--order-latency-ms",
            "--report-latency-ms",
            "--maker-fee-bps",
        ] {
            let mut input = args();
            let index = input.iter().position(|arg| arg == flag).unwrap();
            input.drain(index..=index + 1);
            input.push("one.zst".into());
            assert!(parse(&input).is_err(), "{flag}");
        }
        for (key, value) in [
            ("--network", "unknown"),
            ("--min-notional", "-1"),
            ("--bogus", "1"),
            ("--qty", "1"),
        ] {
            let mut input = args();
            input.extend(["one.zst", key, value].map(OsString::from));
            assert!(parse(&input).is_err(), "{key}");
        }
    }
}
