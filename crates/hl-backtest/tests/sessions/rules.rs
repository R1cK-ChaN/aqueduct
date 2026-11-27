use super::*;
use hl_backtest::{build_identity, replay_session};
use hl_rules::RuleVersion;
use hl_strategy::{FixedSpreadConfig, FixedSpreadQuoter};

fn fixed_components(start: &SessionStart) -> Result<ReplayComponents<FixedSpreadQuoter>, String> {
    let strategy = FixedSpreadConfig::from_parameters(start.instrument, &start.strategy)?;
    let config = SimConfig::from_parameters(&start.sim)?;
    let sim = match start.rule_book().map_err(|error| error.to_string())? {
        Some(book) => SimGateway::with_rules(config, book),
        None => SimGateway::new(config),
    };
    Ok(ReplayComponents {
        host: StrategyHost::new(FixedSpreadQuoter::new(strategy), start.strategy_tag),
        sim,
        strategy_parameters: strategy.parameters(),
        sim_parameters: config.parameters(),
        rules: start.rules.clone(),
    })
}

fn dated_session(fixture: &Fixture) -> String {
    let build = build_identity();
    let start = SessionStart {
        commit: build.commit,
        dirty: build.dirty,
        strategy_name: "fixed_spread".into(),
        strategy: vec![
            ("half_spread_quote_per_base".into(), "1".into()),
            ("qty_base".into(), "1.001".into()),
        ],
        rules: vec![
            RuleVersion::new(Timestamp::from_nanos(100), 2, 17).expect("two-decimal rule"),
            RuleVersion::new(Timestamp::from_millis(6), 3, 21).expect("three-decimal rule"),
        ],
        ..start()
    };
    let recording = read_recordings(&fixture.files[..1], start.network, &start.coin)
        .expect("read the synthetic BTC recording");
    let manifest = vec![ManifestEntry {
        path: "first.hlrec.zst".into(),
        file_seq: Some(0),
        sha256: fixture.hashes[0].clone(),
    }];
    let mut components = fixed_components(&start).expect("build the quoter and dated simulator");
    let (session, _, _) = replay_session(
        &recording,
        &start,
        manifest,
        &mut components.host,
        &mut components.sim,
    )
    .expect("write the complete session with both rule versions");
    session.lines().concat()
}

fn cli_check(path: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hl-backtest"))
        .args(["run", "--check-session"])
        .arg(path)
        .output()
        .expect("run the session checker")
}

#[test]
fn dated_rule_rejections_and_acceptances_replay_from_the_session() {
    let fixture = Fixture::new();
    let text = dated_session(&fixture);
    let rows = rows(&text);
    // The same size needs three decimals at both effect times.
    let first_report = &rows[3];
    assert_eq!(first_report["kind"], "report");
    assert_eq!(first_report["seq"], "2");
    assert_eq!(first_report["ts_exch_ns"], "2000000");
    assert_eq!(first_report["variant"], "rejected");
    assert_eq!(first_report["reason"], "invalid_order");
    assert!(
        rows.iter()
            .any(|row| row["ts_exch_ns"] == "6000000" && row["variant"] == "accepted")
    );

    let path = fixture.session("dated.jsonl", &text);
    let checked = check_session(&path, &build_identity(), |start, _| fixed_components(start))
        .expect("the embedded history reproduces every row");
    assert_eq!(checked.rows, rows.len());
    let output = cli_check(&path);
    assert_success(&output);
    assert_eq!(
        output.stdout,
        format!("session matches ({} rows)\n", rows.len()).as_bytes()
    );
}

#[test]
fn a_factory_that_drops_versions_fails_at_the_first_difference() {
    let fixture = Fixture::new();
    let path = fixture.session("dated.jsonl", &dated_session(&fixture));
    let with_first_versions = |start: &SessionStart, count| {
        let mut installed = start.clone();
        installed.rules.truncate(count);
        fixed_components(&installed)
    };
    let error = check_session(&path, &build_identity(), |start, _| {
        with_first_versions(start, 0)
    })
    .expect_err("returning no versions regenerates a different start")
    .to_string();
    assert_difference(&error, "start", "schema_version");

    for (installed_count, first_difference, variants) in [
        (0, "seq 2", r#"expected "rejected", replayed "accepted""#),
        (1, "seq 6", r#"expected "accepted", replayed "rejected""#),
    ] {
        let error = check_session(&path, &build_identity(), |start, _| {
            let mut components = with_first_versions(start, installed_count)?;
            components.rules = start.rules.clone();
            Ok(components)
        })
        .expect_err("reporting an uninstalled version changes the first report it governs")
        .to_string();
        assert_difference(&error, first_difference, "variant");
        assert!(error.contains(variants), "{error}");
    }
}

#[test]
fn the_regenerated_start_uses_the_installed_asset_ids() {
    let fixture = Fixture::new();
    let path = fixture.session("dated.jsonl", &dated_session(&fixture));
    let error = check_session(&path, &build_identity(), |start, _| {
        let mut installed = start.clone();
        let version = installed.rules[0];
        installed.rules[0] = RuleVersion::new(version.effective_from(), version.sz_decimals(), 18)
            .expect("retain the original size precision");
        fixed_components(&installed)
    })
    .expect_err("changing only the installed asset id changes the start row")
    .to_string();
    assert_difference(&error, "start", "rules[0].asset_id");
}

#[test]
fn invalid_embedded_histories_fail_before_the_factory() {
    let fixture = Fixture::new();
    let text = dated_session(&fixture);
    let rules_field = concat!(
        r#","rules":[{"effective_from":"100","sz_decimals":2,"asset_id":"17"},"#,
        r#"{"effective_from":"6000000","sz_decimals":3,"asset_id":"21"}]"#,
    );
    let assert_invalid = |changed: &str, field: &str| {
        assert_ne!(text, changed, "the edit must change the start row");
        let path = fixture.session("invalid.jsonl", changed);
        let error = check_session::<FixedSpreadQuoter>(&path, &build_identity(), |_, _| {
            panic!("the reader must reject an invalid history before constructing the simulator")
        })
        .expect_err("invalid embedded history")
        .to_string();
        assert!(error.contains("start") && error.contains(field), "{error}");
    };
    for (before, after, field) in [
        (rules_field, "", "rules"),
        (rules_field, r#","rules":[]"#, "rules"),
        (rules_field, r#","rules":{}"#, "rules"),
        (rules_field, r#","rules":[null]"#, "rules"),
        (r#""schema_version":2"#, r#""schema_version":1"#, "rules"),
        (
            r#""schema_version":2"#,
            r#""schema_version":3"#,
            "schema_version",
        ),
        (r#","asset_id":"17""#, "", "rules"),
        (
            r#""asset_id":"17""#,
            r#""asset_id":"17","extra":1"#,
            "rules",
        ),
        (
            r#""asset_id":"17""#,
            r#""asset_id":"17","asset_id":"17""#,
            "rules",
        ),
        (
            r#""sz_decimals":2,"asset_id":"17""#,
            r#""asset_id":"17","sz_decimals":2"#,
            "rules",
        ),
        (r#""sz_decimals":2"#, r#""sz_decimals":7"#, "rules"),
        (r#""sz_decimals":2"#, r#""sz_decimals":"2""#, "rules"),
        (
            r#""effective_from":"100""#,
            r#""effective_from":100"#,
            "rules",
        ),
        (
            r#""effective_from":"100""#,
            r#""effective_from":"0100""#,
            "rules",
        ),
        (r#""asset_id":"17""#, r#""asset_id":17"#, "rules"),
        (r#""asset_id":"17""#, r#""asset_id":"017""#, "rules"),
        (
            r#""effective_from":"6000000""#,
            r#""effective_from":"100""#,
            "rules",
        ),
        (
            r#""effective_from":"6000000""#,
            r#""effective_from":"99""#,
            "rules",
        ),
    ] {
        assert_invalid(&text.replacen(before, after, 1), field);
    }
    let misplaced = text.replacen(rules_field, "", 1).replacen(
        r#","sim":"#,
        &format!(r#"{rules_field},"sim":"#),
        1,
    );
    assert_invalid(&misplaced, "rules");
}

#[test]
fn coverage_starting_after_the_first_effect_time_fails_the_check() {
    let fixture = Fixture::new();
    let text = dated_session(&fixture).replacen(
        r#""effective_from":"100""#,
        r#""effective_from":"2000001""#,
        1,
    );
    let path = fixture.session("late-coverage.jsonl", &text);
    let message =
        "instrument 1637510701 at effect time 2000000 ns: rule coverage starts at 2000001 ns";
    let error = check_session(&path, &build_identity(), |start, _| fixed_components(start))
        .expect_err("the first placement precedes coverage")
        .to_string();
    assert!(error.contains(message), "{error}");
    let output = cli_check(&path);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).expect("UTF-8 checker diagnostics");
    assert!(stderr.contains(message), "{stderr}");
}

#[test]
fn start_encoding_is_checked_before_raw_file_hashes() {
    let fixture = Fixture::new();
    let sessions = [
        (1, fixture.expected(), build()),
        (2, dated_session(&fixture), build_identity()),
    ];
    fs::write(&fixture.files[0], b"damaged recording").expect("damage the first raw file");

    for (version, mut text, identity) in sessions {
        text.insert(1, ' ');
        let path = fixture.session("spacing.jsonl", &text);
        let error = check_session::<FixtureStrategy>(&path, &identity, |_, _| {
            panic!("the invalid start must fail before the factory")
        })
        .expect_err("start spacing must be checked before the damaged raw file")
        .to_string();
        assert!(
            error.starts_with("start encoding differs:"),
            "version {version}: {error}"
        );
    }
}
