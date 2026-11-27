use super::*;

const RULES: &str = include_str!("../fixtures/synth-rules.json");
const VERSIONS: &str = concat!(
    r#"[{"effective_from":"100","sz_decimals":2,"asset_id":"17"},"#,
    r#"{"effective_from":"200","sz_decimals":3,"asset_id":"21"}]"#,
);

fn write_synth_recording(directory: &Path, first_receipt_ns: u64) {
    let mut opening = header(0);
    edit_header(&mut opening, "url", json!(TESTNET_URL));
    let mut quote = record(
        RecordKind::RecvText,
        0,
        json!({"channel": "bbo", "data": {
            "coin": "SYNTH", "time": 0,
            "bbo": [{"px": "98", "sz": "10", "n": 1}, {"px": "102", "sz": "10", "n": 1}]
        }}),
    );
    quote.recv_mono_ns = first_receipt_ns;
    let proof = record(
        RecordKind::RecvText,
        1,
        json!({"channel": "trades", "data": [{
            "coin": "SYNTH", "side": "A", "px": "100", "sz": "1", "time": 1, "tid": 1,
            "hash": format!("0x{}", "00".repeat(32)),
            "users": [format!("0x{}", "01".repeat(20)), format!("0x{}", "02".repeat(20))]
        }]}),
    );
    write_recording(
        directory,
        "synth.hlrec.zst",
        &[opening, quote, proof],
        false,
    );
}

fn run(directory: &Path, options: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hl-backtest"))
        .current_dir(directory)
        .args([
            "run",
            "--network",
            "testnet",
            "--coin",
            "SYNTH",
            "--half-spread",
            "1",
            "--qty",
            "1.001",
            "--order-latency-ms",
            "0",
            "--report-latency-ms",
            "0",
            "--maker-fee-bps",
            "0",
            "synth.hlrec.zst",
        ])
        .args(options)
        .output()
        .expect("run the synthetic backtest")
}

#[test]
fn rules_reject_orders_with_and_without_session_output_and_replay_after_deletion() {
    let fixture = Fixture::new();
    let dir = &fixture.directory;
    write_synth_recording(dir, 199);
    let rules_path = fixture.session("rules.json", RULES);
    let control = run(dir, &[]);
    assert_success(&control);
    let unchecked: Value = serde_json::from_slice(&control.stdout).expect("summary");
    assert!(unchecked.get("rules").is_none());
    assert_eq!(unchecked["reports"]["accepted"], 2);
    assert_eq!(
        unchecked["reports"]["rejections_by_reason"]["invalid_order"],
        0
    );

    let summary_only = run(dir, &["--rules-file", rules_path.to_str().unwrap()]);
    assert_success(&summary_only);
    let saved = run(
        dir,
        &[
            "--rules-file",
            "rules.json",
            "--session-out",
            "session.jsonl",
        ],
    );
    assert_success(&saved);
    assert_eq!(summary_only.stdout, saved.stdout);
    let stdout = std::str::from_utf8(&saved.stdout).expect("summary text");
    assert!(stdout.contains(&format!(
        r#""min_notional":"10","rules":{VERSIONS},"first_ts_local_ms":"#
    )));
    let summary: Value = serde_json::from_str(stdout).expect("summary");
    assert_eq!(
        summary["reports"]["rejections_by_reason"]["invalid_order"],
        2
    );

    let session_path = dir.join("session.jsonl");
    let session = fs::read_to_string(&session_path).expect("sealed session");
    let start = session.lines().next().expect("start row");
    assert!(start.ends_with(&format!(
        r#""min_order_notional_usd":"10"}},"rules":{VERSIONS}}}"#
    )));
    let rows = rows(&session);
    assert_eq!(rows[0]["schema_version"], 2);
    assert_eq!(rows[0]["rules"], summary["rules"]);
    assert_eq!(
        rows.iter()
            .filter(|row| row["variant"] == "rejected"
                && row["reason"] == "invalid_order"
                && row["ts_exch_ns"] == "199")
            .count(),
        2
    );

    fs::remove_file(rules_path).expect("remove the rules source");
    let checked = rules::cli_check(&session_path);
    assert_success(&checked);
    assert_eq!(
        checked.stdout,
        format!("session matches ({} rows)\n", rows.len()).as_bytes()
    );
}

#[test]
fn rules_setup_errors_precede_reading_recordings_and_creating_session_output() {
    let fixture = Fixture::new();
    fixture.session("invalid.json", "{");
    fixture.session("other.json", &RULES.replace("SYNTH", "OTHER"));
    // No synth recording exists. Its read error would expose incorrect setup order.
    for (path, cause) in [
        ("missing.json", "No such file or directory"),
        ("invalid.json", "invalid rules file ("),
        ("other.json", "missing rules for testnet SYNTH"),
    ] {
        let output = run(
            &fixture.directory,
            &["--rules-file", path, "--session-out", "session.jsonl"],
        );
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(&format!("rules file {path}: {cause}")),
            "{stderr}"
        );
        assert!(!fixture.directory.join("session.jsonl").exists());
        assert!(!fixture.directory.join("session.jsonl.part").exists());
    }
}

#[test]
fn missing_coverage_at_proof_aborts_each_replay_before_output() {
    let fixture = Fixture::new();
    write_synth_recording(&fixture.directory, 99);
    fixture.session("rules.json", RULES);
    for options in [
        &["--rules-file", "rules.json"][..],
        &[
            "--rules-file",
            "rules.json",
            "--session-out",
            "session.jsonl",
        ],
    ] {
        let output = run(&fixture.directory, options);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            concat!(
                "hl-backtest: testnet SYNTH: instrument 3509668803 at effect time 99 ns: ",
                "rule coverage starts at 100 ns\n",
            )
        );
        assert!(!fixture.directory.join("session.jsonl").exists());
        assert!(!fixture.directory.join("session.jsonl.part").exists());
    }
}

#[test]
fn repeated_missing_and_check_session_rules_options_are_usage_errors() {
    for (args, message) in [
        (
            &["--rules-file", "a.json", "--rules-file", "b.json"][..],
            "duplicate option --rules-file",
        ),
        (&["--rules-file"], "missing value for --rules-file"),
        (
            &["--rules-file", "--session-out", "session.jsonl"],
            "missing value for --rules-file",
        ),
        (
            &["--check-session", "session.jsonl", "--rules-file", "a.json"],
            "unknown option --check-session",
        ),
        (
            &["--rules-file", "a.json", "--check-session", "session.jsonl"],
            "unknown option --check-session",
        ),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_hl-backtest"))
            .arg("run")
            .args(args)
            .output()
            .expect("run CLI with invalid arguments");
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.starts_with(&format!("hl-backtest: {message}\nusage:")),
            "{stderr}"
        );
    }
}
