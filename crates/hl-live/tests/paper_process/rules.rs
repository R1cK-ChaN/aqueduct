use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use hl_codec::hyperliquid::{Network, instrument_id, parse_rules_file};
use serde_json::json;

use super::support::{
    SYNTH_RULES, bbo_for_coin, config_text_for_coin, connect_for_coin, count, listener, read_rows,
    trade_for_coin,
};
use super::{Fixture, after_last_callback};

fn write_config(fixture: &Fixture, port: u16, rules: &Path) -> PathBuf {
    let config = fixture.directory.join("paper.toml");
    let settings = config_text_for_coin(&fixture.out_dir(), port, "SYNTH");
    fs::write(
        &config,
        format!(
            "rules_file = {:?}\n{settings}",
            rules.to_str().expect("UTF-8 path")
        ),
    )
    .expect("write paper settings");
    config
}

#[test]
fn both_commands_validate_the_rules_before_creating_output_or_connecting() {
    let fixture = Fixture::new(false);
    let listener = listener();
    let port = listener.local_addr().expect("loopback port").port();
    let relative = Path::new("rules.json");
    let rules = fixture.directory.join(relative);
    let config = write_config(&fixture, port, relative);
    assert_ne!(
        std::env::current_dir().expect("working directory"),
        fixture.directory
    );
    let wrong_pair = SYNTH_RULES.replace("testnet", "mainnet");
    let read_error = fs::read(&rules)
        .expect_err("rules not written yet")
        .to_string();
    let parse_error = parse_rules_file("{").expect_err("invalid JSON").to_string();
    for (contents, error) in [
        (None, read_error),
        (Some("{"), parse_error),
        (
            Some(wrong_pair.as_str()),
            "missing rules for testnet SYNTH".into(),
        ),
    ] {
        if let Some(contents) = contents {
            fs::write(&rules, contents).expect("write invalid rules");
        }
        for command in ["paper", "check"] {
            let mut child = fixture.command(command, &config);
            assert_eq!(child.wait_for_exit().code(), Some(1));
            assert_eq!(
                child.log_text(),
                format!("hl-live: rules file {}: {error}\n", rules.display())
            );
            assert!(!fixture.out_dir().exists());
            assert_eq!(
                listener.accept().expect_err("no connection").kind(),
                ErrorKind::WouldBlock
            );
        }
    }

    fs::write(&rules, SYNTH_RULES).expect("write valid rules");
    let absolute = rules.canonicalize().expect("absolute rules path");
    for path in [relative, &absolute] {
        let config = write_config(&fixture, port, path);
        let mut check = fixture.command("check", &config);
        assert!(check.wait_for_exit().success(), "{}", check.log_text());
        assert!(!fixture.out_dir().exists());
        assert_eq!(
            listener.accept().expect_err("no connection").kind(),
            ErrorKind::WouldBlock
        );
    }
}

#[test]
fn coverage_failure_exits_with_an_unsealed_session() {
    let fixture = Fixture::new(false);
    let listener = listener();
    let port = listener.local_addr().expect("loopback port").port();
    let rules = fixture.directory.join("rules.json");
    let first = i64::MAX;
    let history = json!({"instruments": [{"network": "testnet", "coin": "SYNTH",
        "versions": [{"effective_from": first.to_string(), "sz_decimals": 2, "asset_id": "17"}]
    }]});
    fs::write(&rules, history.to_string()).expect("write rules starting after the run");
    let config = write_config(&fixture, port, &rules);
    let mut paper = fixture.command("paper", &config);
    let mut socket = connect_for_coin(&listener, "SYNTH");
    bbo_for_coin(&mut socket, "SYNTH", 0, "98", "102");
    let rows = paper.wait_rows(|rows| count(rows, "action", "place") == 2);
    let placement = rows
        .iter()
        .find(|row| row["variant"] == "place")
        .expect("placement");
    let effect = placement["ts_local_ns"]
        .as_str()
        .expect("zero order latency");
    let partial = paper.partial().expect("open paper session");
    trade_for_coin(&mut socket, "SYNTH", after_last_callback(&rows), 1, "0.5");

    assert_eq!(paper.wait_for_exit().code(), Some(1));
    let instrument = instrument_id(Network::Testnet, "SYNTH");
    let message = format!(
        "hl-live: testnet SYNTH: instrument {instrument} at effect time {effect} ns: rule coverage starts at {first} ns"
    );
    let stderr = paper.log_text();
    assert!(stderr.lines().any(|line| line == message), "{stderr}");
    assert!(!stderr.contains("no grid check applied"));
    assert!(!partial.with_file_name("session.jsonl").exists());
    assert!(
        read_rows(&partial)
            .iter()
            .all(|row| row["kind"] != "end" && row["kind"] != "report")
    );
}
