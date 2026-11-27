use super::*;
use hl_run::recording::RawPosition;
use hl_run::session::{CheckpointBoundary, checkpoint_line};

const DAY_MS: i64 = 86_400_000;

fn numbered_header(seq: u64, time: i64) -> Record {
    let mut header = header(time);
    edit_header(&mut header, "process_start_wall_ns", json!(0));
    edit_header(&mut header, "file_seq", json!(seq));
    header
}

fn control(time: i64) -> Record {
    record(RecordKind::SentText, time, json!({"method":"ping"}))
}

// The only order action is the first gap's cancel-all. There is no market
// evidence to prove it. At M1 the last two pre-cutoff work rows are the gap
// and cancel. At M2 the gap delivered at M1 has joined them. A control with
// a forward wall stamp followed by a backward step still precedes the gap at
// M1 in raw order. The ignored control at position 0/4 is the first extent.
fn across_two_midnights(fixture: &Fixture) -> String {
    let files = [
        vec![
            numbered_header(0, 0),
            metadata(0, "connected"),
            metadata(DAY_MS - 1, "disconnected"),
            control(DAY_MS + 1),
            control(DAY_MS - 1),
            metadata(DAY_MS, "dropped"),
        ],
        vec![
            numbered_header(1, DAY_MS + 1),
            metadata(DAY_MS + 1, "connected"),
            control(2 * DAY_MS - 1),
            metadata(2 * DAY_MS, "dropped"),
        ],
        vec![
            numbered_header(2, 2 * DAY_MS + 1),
            metadata(2 * DAY_MS + 2, "shutdown"),
        ],
    ];
    let mut expected = include_str!("../checkpoint-session.jsonl").to_owned();
    for (index, records) in files.iter().enumerate() {
        let name = format!("day-{index}.zst");
        let path = write_recording(&fixture.directory, &name, records, false);
        expected = expected.replace(&format!("SHA_{index}"), &independent_hash(&path));
    }
    expected
}

fn first_lines(text: &str, count: usize) -> String {
    text.split_inclusive('\n').take(count).collect()
}

fn check_three_midnight_extents(files: &[&[Record]], last_raw: [(usize, u64); 3]) {
    let fixture = Fixture::new();
    let mut manifest = Vec::new();
    for (file_seq, records) in files.iter().enumerate() {
        let name = format!("control-{file_seq}.zst");
        let path = write_recording(&fixture.directory, &name, records, false);
        manifest.push(ManifestEntry {
            path: name,
            file_seq: Some(file_seq as u64),
            sha256: independent_hash(&path),
        });
    }

    let build = hl_backtest::build_identity();
    let start = SessionStart {
        commit: build.commit,
        dirty: build.dirty,
        driver: "paper".into(),
        source: Some("hl".into()),
        process_start_wall_ns: Some(0),
        strategy_name: "fixed_spread".into(),
        strategy: vec![
            ("half_spread_quote_per_base".into(), "1".into()),
            ("qty_base".into(), "1".into()),
        ],
        ..start()
    };
    let mut text = Session::new(&start).lines().concat();
    for (day, (file_index, record_index)) in (1..).zip(last_raw) {
        text.push_str(&checkpoint_line(
            CheckpointBoundary {
                cutoff: Timestamp::from_millis(day * DAY_MS),
                last_seq: None,
                late_print_misses: Default::default(),
            },
            Some(RawPosition {
                file_index,
                record_index,
            }),
            &manifest[..=file_index],
        ));
    }
    let path = fixture.session("session.jsonl.part", &text);
    let checked = Command::new(env!("CARGO_BIN_EXE_hl-backtest"))
        .args(["run", "--check-session"])
        .arg(path)
        .output()
        .expect("run the paper session checker");
    assert_success(&checked);
    assert_eq!(
        checked.stdout,
        b"checkpoint 1970-01-04T00:00:00Z matches (4 rows)\n"
    );
}

#[test]
fn checkpoint_raw_extent_and_manifest_carry_across_an_empty_day() {
    // File 1 opens exactly at the second midnight. The first two checkpoints
    // both end at the ping in file 0 and carry only that file's digest.
    check_three_midnight_extents(
        &[
            &[header(0), control(DAY_MS - 1)],
            &[
                numbered_header(1, 2 * DAY_MS),
                control(2 * DAY_MS + 1),
                control(3 * DAY_MS),
            ],
        ],
        [(0, 1), (0, 1), (1, 1)],
    );
}

#[test]
fn checkpoint_raw_extent_follows_position_when_control_times_step_back() {
    // All three days contain records. Position 4 moves back more than a day
    // from position 3, so it remains the last position at the third midnight.
    check_three_midnight_extents(
        &[&[
            header(0),
            control(2 * DAY_MS + 1),
            control(DAY_MS - 1),
            control(2 * DAY_MS + 2),
            control(DAY_MS),
            control(3 * DAY_MS),
        ]],
        [(0, 2), (0, 4), (0, 4)],
    );
}

#[test]
fn checks_two_midnights_in_a_sealed_file_and_both_open_prefixes() {
    let fixture = Fixture::new();
    let text = across_two_midnights(&fixture);
    for (lines, rows) in [(5, 4), (7, 6), (9, 9)] {
        // The live writer emitted one next-day gap before each checkpoint.
        // Its bytes belong to a later comparison.
        let path = fixture.session("session.jsonl", &first_lines(&text, lines));
        let summary = check_session(&path, &build(), components).expect("valid fixture");
        assert_eq!(summary.rows, rows);
    }
}

#[test]
fn open_checkpoint_ignores_everything_after_its_complete_line() {
    let fixture = Fixture::new();
    let text = across_two_midnights(&fixture);
    let mut open = first_lines(&text, 7).into_bytes();
    open.extend_from_slice(b"not even JSON\n{\"truncated\":\"\xf0\x9f");
    let path = fixture.directory.join("session.jsonl.part");
    fs::write(&path, open).expect("write complete and torn suffix");
    let summary = check_session(&path, &build(), components).expect("matching prefix");
    assert_eq!(summary.rows, 6);
}

#[test]
fn open_paper_session_without_a_checkpoint_has_nothing_to_check_yet() {
    let fixture = Fixture::new();
    let text = across_two_midnights(&fixture);
    let path = fixture.session("session.jsonl.part", &first_lines(&text, 4));
    let error = check(&path).expect_err("no completed boundary");
    assert!(error.contains("nothing to check yet"), "{error}");
}

#[test]
fn replay_rejects_tampered_checkpoint_values_extents_and_bytes() {
    let fixture = Fixture::new();
    let text = across_two_midnights(&fixture);
    let lines: Vec<_> = text.lines().collect();
    let checkpoint = lines[6];
    for (before, after, reason) in [
        (r#""last_seq":"2""#, r#""last_seq":"1""#, "last_seq"),
        (r#""last_seq":"2""#, r#""last_seq":"3""#, "last_seq"),
        (r#""last_seq":"2""#, r#""seq":"2""#, "last_seq"),
        (r#""trades":0"#, r#""trades":1"#, "late_print_misses.trades"),
        (r#""qty":"0""#, r#""qty":"0.1""#, "late_print_misses.qty"),
        (r#""record_index":"2""#, r#""record_index":"1""#, "last_raw"),
        (r#""record_index":"2""#, r#""record_index":"3""#, "last_raw"),
        (
            r#""last_seq":"2""#,
            r#""last_seq": "2""#,
            "encoding differs",
        ),
    ] {
        let edited = checkpoint.replace(before, after);
        assert_ne!(edited, checkpoint);
        for length in [7, 9] {
            let damaged = first_lines(&text, length).replace(checkpoint, &edited);
            let error = check(&fixture.session("tampered.jsonl", &damaged))
                .expect_err("tampered checkpoint");
            assert!(
                error.contains("checkpoint") && error.contains(reason),
                "{error}"
            );
        }
    }
}

#[test]
fn hash_and_successive_manifest_prefixes_are_checked_before_replay() {
    let fixture = Fixture::new();
    let text = across_two_midnights(&fixture);
    let lines: Vec<_> = text.lines().collect();
    let first = lines[4];
    let second = lines[6];
    let sha = independent_hash(&fixture.directory.join("day-0.zst"));
    let cases = [
        (
            first_lines(&text, 5).replace(&sha, &"0".repeat(64)),
            "sha256",
        ),
        // Both entries still name the very same file with the very same hash.
        (
            first_lines(&text, 7).replace(first, &first.replace("day-0.zst", "./day-0.zst")),
            "manifest",
        ),
        (
            text.replace(second, &second.replace("day-1.zst", "./day-1.zst")),
            "manifest",
        ),
    ];
    for (damaged, field) in cases {
        let path = fixture.session("manifest.jsonl", &damaged);
        let error = check_session::<FixtureStrategy>(&path, &build(), |_, _| {
            panic!("checkpoint manifests and hashes must precede strategy construction")
        })
        .expect_err("invalid checkpoint manifest")
        .to_string();
        assert!(
            error.contains("checkpoint") && error.contains(field),
            "{error}"
        );
    }
}

#[test]
fn skipped_and_extra_midnights_fail_before_strategy_construction() {
    let fixture = Fixture::new();
    let text = across_two_midnights(&fixture);
    let lines: Vec<_> = text.split_inclusive('\n').collect();
    let none = lines
        .iter()
        .copied()
        .filter(|line| !line.contains(r#""kind":"checkpoint""#))
        .collect::<String>();
    for damaged in [
        none, // The base checker accepted this complete two-day session.
        text.replace(lines[4], ""),
        text.replace(lines[6], ""),
        first_lines(&text, 7).replace(lines[4], ""),
        text.replace(lines[6], &lines[6].repeat(2)),
        text.replace(
            lines[6],
            &format!(
                "{}{}",
                lines[6],
                lines[6].replace("172800000000000", "259200000000000")
            ),
        ),
    ] {
        let error = check_session::<FixtureStrategy>(
            &fixture.session("midnights.jsonl", &damaged),
            &build(),
            |_, _| panic!("missing or extra checkpoint must fail before strategy construction"),
        )
        .expect_err("missing or extra checkpoint")
        .to_string();
        assert!(error.contains("checkpoint"), "{error}");
    }
}

#[test]
fn open_checkpoint_checks_all_and_only_work_through_the_cutoff() {
    let fixture = Fixture::new();
    let text = across_two_midnights(&fixture);
    let prefix = first_lines(&text, 7);
    let lines: Vec<_> = prefix.split_inclusive('\n').collect();
    let error = check(&fixture.session("missing.jsonl.part", &prefix.replace(lines[3], "")))
        .expect_err("gap before the cutoff is required");
    assert!(error.contains("seq"), "{error}");
    // A further row bearing a pre-cutoff time is not a legitimate next-day
    // row just because it was written ahead of the delayed checkpoint.
    let spurious = prefix.replace(
        lines[5],
        &lines[5].replace("172800000000000", "172799999999999"),
    );
    let error = check(&fixture.session("extra.jsonl.part", &spurious))
        .expect_err("extra work before cutoff");
    assert!(
        error.contains("seq") || error.contains("checkpoint"),
        "{error}"
    );
}

#[test]
fn a_next_day_timestamp_cannot_hide_an_extra_checked_sequence() {
    let fixture = Fixture::new();
    let text = across_two_midnights(&fixture);
    let mut lines: Vec<_> = text
        .split_inclusive('\n')
        .take(7)
        .map(str::to_owned)
        .collect();
    let duplicate = lines[5].replace(r#""seq":"3""#, r#""seq":"2""#);
    lines.insert(6, duplicate);
    let error = check(&fixture.session("duplicate.jsonl.part", &lines.concat()))
        .expect_err("one seq per work row");
    assert!(error.contains("seq"), "{error}");
}

#[test]
fn a_bad_hash_shared_with_the_footer_names_the_affected_checkpoint() {
    let fixture = Fixture::new();
    let text = across_two_midnights(&fixture);
    let sha = independent_hash(&fixture.directory.join("day-0.zst"));
    let changed = text.replace(&sha, &"0".repeat(64));
    let error = check(&fixture.session("bad-shared-hash.jsonl", &changed)).expect_err("bad digest");
    assert!(
        error.contains("checkpoint")
            && error.contains("86400000000000")
            && error.contains("sha256"),
        "{error}"
    );
}

#[test]
fn quiet_checkpoint_before_the_first_raw_header_checks_an_empty_prefix() {
    let fixture = Fixture::new();
    let mut open = first_lines(include_str!("../checkpoint-session.jsonl"), 1);
    open.push_str(concat!(r#"{"kind":"checkpoint","cutoff_ns":"86400000000000","last_seq":null,"last_raw":null,"manifest":[],"late_print_misses":{"trades":0,"qty":"0"}}"#, "\n"));
    open.push_str(r#"{"kind":"end"}"#); // Complete JSON in an unpublished line.
    let path = fixture.session("before-first-admission.jsonl.part", &open);
    let summary = check_session(&path, &build(), components).expect("empty verified prefix");
    assert_eq!(summary.rows, 2);
}
