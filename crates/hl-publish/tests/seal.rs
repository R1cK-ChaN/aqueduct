//! Exercise the local publication boundary through its command-line interface.

mod support;

use std::fs;

use hl_codec::rawlog::{HEADER_LEN, RecordKind::*};
use hl_recorder::public_v1;
use serde_json::{Value, json};
use support::*;

#[test]
fn clean_rotations_cover_only_observed_channels_and_seal_identically() {
    let day = Day::today();
    // Different creation and replay orders exercise the directory enumeration boundary.
    let last = day.put(
        2,
        PROCESS_NS,
        2,
        &[(RecvText, SOL_CONTEXT), (RecvText, BTC_TRADE)],
    );
    let first = day.put(
        0,
        PROCESS_NS,
        0,
        &[
            (Meta, r#"{"event":"connecting"}"#),
            (Meta, r#"{"event":"connected"}"#),
            (SentText, public_v1::PING),
            (
                SentText,
                r#"{"method":"subscribe","subscription":{"type":"trades","coin":"HYPE"}}"#,
            ),
            (RecvText, BTC_BBO),
        ],
    );
    let middle = day.put(
        1,
        PROCESS_NS,
        1,
        &[
            (RecvText, ETH_L2),
            // A shared frame with a synthetic off-universe coin.
            (
                RecvText,
                r#"{"channel":"fastAssetCtxs","data":"q1byd3Pz8QwOUbKqVspNLMoOqFCyUjJUqq0FAA=="}"#,
            ),
            (RecvText, r#"{"channel":"subscriptionResponse"}"#),
        ],
    );
    let report = accepted(1, &first) + &accepted(2, &middle) + &accepted(3, &last);
    let names = [&first, &middle, &last];
    let before = names.map(|name| fs::read(day.source_dir().join(name)).expect("fixture bytes"));
    let manifest = sealed(&day, &mut day.with_quality(), &report);
    assert_eq!(manifest["gaps"], json!([]));
    assert_eq!(manifest["coverage_ends"], json!([]));
    assert_eq!(
        manifest["coins_covered"],
        json!({
            "BTC": ["bbo", "trades"], "ETH": ["l2"], "SOL": ["activeAssetCtx"]
        })
    );
    assert_eq!(manifest["files"].as_array().expect("files").len(), 3);
    let bytes = fs::read(day.manifest()).expect("first manifest");
    fs::rename(day.manifest(), day.root.join("first.json")).expect("empty the output directory");
    sealed(&day, &mut day.with_quality(), &report);
    assert_eq!(fs::read(day.manifest()).expect("second manifest"), bytes);
    for (name, original) in names.into_iter().zip(before) {
        assert_eq!(
            fs::read(day.source_dir().join(name)).expect("sealed input"),
            original
        );
    }
    snapshot(&day, include_str!("manifests/clean.json"));
}

#[test]
fn midnight_link_disconnect_drops_holes_and_process_restart() {
    let prev = Day::yesterday();
    let anchor = prev.hour_ms(23) * 1_000_000;
    let prev_name = prev.put(
        23,
        anchor,
        0,
        &[
            (Meta, r#"{"event":"connected"}"#),
            (
                Meta,
                r#"{"event":"disconnected","reason":"synthetic outage"}"#,
            ),
        ],
    );
    let prev_manifest = sealed(&prev, &mut prev.with_quality(), &accepted(1, &prev_name));
    let open = gap(&prev_manifest, "disconnect");
    let disconnect_ns = (anchor + 2_000_000_000).to_string();
    assert_eq!(open["start_wall_ns"], disconnect_ns);
    assert_eq!(open["end_wall_ns"], Value::Null);

    let day = Day::today();
    let first = day.put(
        0,
        anchor,
        1,
        &[
            (Meta, r#"{"event":"connected"}"#),
            (RecvText, BTC_BBO),
            (Meta, r#"{"event":"dropped","count":7}"#),
        ],
    );
    let third = day.put(2, anchor, 3, &[(RecvText, ETH_L2)]);
    let restarted = day.put(3, day.hour_ms(3) * 1_000_000, 0, &[(RecvText, SOL_CONTEXT)]);
    let partial = day.name(1) + ".part";
    let bytes = compress(
        &encode(day.hour_ms(1), anchor, header(anchor, 2), &[]),
        true,
    );
    fs::write(day.source_dir().join(partial), &bytes[..bytes.len() - 4]).expect("stage partial");
    let mut quality = quality(day.date);
    quality["checks"][0] = json!({
        "name": "load_problems", "subject": "synthetic", "status": "fail",
        "compared": 2, "violations": 1
    });
    fs::write(day.quality_file(), quality.to_string()).expect("write completed alarm");
    let mut command = day.with_quality();
    command
        .arg("--previous-manifest")
        .arg(prev.manifest())
        .args(["--coverage-end", "SOL=2026-11-26T03:45:00Z"]);
    let report = accepted(1, &first)
        + &withheld(2, "partial recording")
        + &accepted(3, &third)
        + &accepted(4, &restarted);
    let manifest = sealed(&day, &mut command, &report);
    assert_eq!(
        manifest["files"][0]["previous_path"],
        prev_manifest["files"][0]["path"]
    );
    assert!(manifest.get("version_change").is_none());
    let gaps = manifest["gaps"].as_array().expect("gaps array");
    assert_eq!(gaps.len(), 5);
    let missing = gap(&manifest, "missing_sequence");
    assert_eq!(missing["file_seq"], "2");
    assert_eq!(missing["last_file_seq"], "2");
    let carried = gap(&manifest, "disconnect");
    assert_eq!(carried["start_wall_ns"], disconnect_ns);
    assert_eq!(
        carried["end_wall_ns"],
        (PROCESS_NS + 1_000_000_000).to_string()
    );
    assert_eq!(gap(&manifest, "dropped")["count"], 7);
    assert_eq!(gap(&manifest, "process_change")["file_seq"], "0");
    assert!(gap(&manifest, "withheld").is_object());
    snapshot(&day, include_str!("manifests/gaps.json"));
}

fn gap<'a>(manifest: &'a Value, kind: &str) -> &'a Value {
    manifest["gaps"]
        .as_array()
        .expect("gaps array")
        .iter()
        .find(|gap| gap["kind"] == kind)
        .expect("expected gap kind")
}

#[test]
fn conflicting_sequences_withhold_every_copy_and_their_coverage() {
    let day = Day::today();
    let first = day.put(0, PROCESS_NS, 0, &[(RecvText, BTC_BBO)]);
    day.put(1, PROCESS_NS, 1, &[(RecvText, ETH_L2)]);
    day.put(2, PROCESS_NS, 1, &[(RecvText, SOL_CONTEXT)]);
    let last = day.put(3, PROCESS_NS, 2, &[(SentText, public_v1::PING)]);
    let report = accepted(1, &first)
        + &withheld(2, "sequence_conflict")
        + &withheld(3, "sequence_conflict")
        + &accepted(4, &last);
    let manifest = sealed(&day, &mut day.with_quality(), &report);
    assert_eq!(manifest["coins_covered"], json!({"BTC": ["bbo"]}));
    let files = manifest["files"].as_array().expect("files array");
    assert_eq!(
        files
            .iter()
            .map(|file| &file["file_seq"])
            .collect::<Vec<_>>(),
        ["0", "2"]
    );
    let missing = gap(&manifest, "missing_sequence");
    assert_eq!(missing["file_seq"], "1");
    assert_eq!(missing["last_file_seq"], "1");
}

#[test]
fn replay_uses_sequence_when_filename_time_goes_backwards() {
    let day = Day::today();
    let first = day.put(2, PROCESS_NS, 0, &[]);
    let second = day.put(1, PROCESS_NS, 1, &[]);
    let report = accepted(1, &second) + &accepted(2, &first);
    let manifest = sealed(&day, &mut day.with_quality(), &report);
    assert_eq!(manifest["files"][0]["file_seq"], "0");
    assert_eq!(manifest["files"][1]["file_seq"], "1");
    assert_eq!(manifest["gaps"], json!([]));
}

#[test]
fn identical_copies_at_the_end_still_leave_a_gap() {
    let day = Day::today();
    let first = day.put(0, PROCESS_NS, 0, &[]);
    let original = day.put(1, PROCESS_NS, 1, &[(RecvText, BTC_BBO)]);
    let alternate = original.replace(
        &day.hour_ms(1).to_string(),
        &(day.hour_ms(1) + 1).to_string(),
    );
    fs::copy(
        day.source_dir().join(original),
        day.source_dir().join(alternate),
    )
    .expect("copy fixture");
    let report = accepted(1, &first)
        + &withheld(2, "duplicate_sequence")
        + &withheld(3, "duplicate_sequence");
    let manifest = sealed(&day, &mut day.with_quality(), &report);
    assert_eq!(manifest["files"].as_array().expect("files").len(), 1);
    let missing = gap(&manifest, "missing_sequence");
    assert_eq!(missing["file_seq"], "1");
    assert_eq!(missing["last_file_seq"], "1");
    assert_eq!(missing["end_wall_ns"], Value::Null);
}

fn only_withheld(day: &Day, command: &mut std::process::Command, reason: &str) -> Value {
    let manifest = sealed(day, command, &withheld(1, reason));
    assert_eq!(manifest["files"], json!([]));
    assert_eq!(manifest["coins_covered"], json!({}));
    assert!(gap(&manifest, "withheld").is_object());
    manifest
}

#[test]
fn checksum_clean_eof_and_complete_record_are_all_required() {
    let raw = encode(
        MIDNIGHT_MS,
        PROCESS_NS,
        header(PROCESS_NS, 0),
        &[(RecvText, BTC_BBO)],
    );
    let good = compress(&raw, true);
    let mut bad_checksum = good.clone();
    *bad_checksum.last_mut().expect("checksum") ^= 1;
    let mut second_frame = good.clone();
    second_frame.extend(compress(&[], true));
    let mut trailing_junk = good.clone();
    trailing_junk.push(0xff);
    let mut partial_header = raw.clone();
    partial_header.extend([0, 0, 0]);
    let mut unknown_kind = raw.clone();
    let first_len = HEADER_LEN + header(PROCESS_NS, 0).to_string().len();
    unknown_kind[first_len + 4] = 0xff;
    for (label, bytes, reason) in [
        ("bad checksum", bad_checksum, "invalid_zstd"),
        ("cut zstd", good[..good.len() - 1].to_vec(), "invalid_zstd"),
        ("no checksum", compress(&raw, false), "invalid_zstd"),
        ("second frame", second_frame, "invalid_zstd"),
        ("ending junk", trailing_junk, "invalid_zstd"),
        (
            "cut record header",
            compress(&partial_header, true),
            "invalid_record",
        ),
        (
            "cut final payload",
            compress(&raw[..raw.len() - 1], true),
            "invalid_record",
        ),
        (
            "unknown kind",
            compress(&unknown_kind, true),
            "invalid_record",
        ),
    ] {
        let day = Day::today();
        let path = day.source_dir().join(day.name(0));
        fs::write(&path, &bytes).expect("stage invalid fixture");
        only_withheld(&day, &mut day.with_quality(), reason);
        assert_eq!(fs::read(path).expect("staged bytes"), bytes, "{label}");
    }
}

#[test]
fn partial_truncated_foreign_name_wrong_date_and_directory_are_withheld() {
    let day = Day::today();
    let original = day.name(0);
    for (bad_name, reason, known_hour) in [
        (original.clone() + ".part", "partial recording", true),
        (
            original.replace(".hlrec.zst", ".truncated.hlrec.zst"),
            "truncated recording",
            true,
        ),
        (
            original.replace("20261126", "20261125"),
            "invalid filename",
            false,
        ),
        (
            "synthetic-private-id\naccepted\t99".into(),
            "invalid filename",
            false,
        ),
    ] {
        let case = Day::today();
        let raw = encode(MIDNIGHT_MS, PROCESS_NS, header(PROCESS_NS, 0), &[]);
        fs::write(case.source_dir().join(bad_name), compress(&raw, true))
            .expect("stage misnamed file");
        let manifest = only_withheld(&case, &mut case.with_quality(), reason);
        let gap = gap(&manifest, "withheld");
        if known_hour {
            assert_eq!(gap["start_wall_ns"], PROCESS_NS.to_string());
            assert_eq!(
                gap["end_wall_ns"],
                (PROCESS_NS + HOUR_MS * 1_000_000).to_string()
            );
        } else {
            assert_eq!(gap["start_wall_ns"], Value::Null);
            assert_eq!(gap["end_wall_ns"], Value::Null);
        }
        assert_eq!(gap["reason"], reason);
        assert!(!manifest.to_string().contains("synthetic-private-id"));
    }
    fs::create_dir(day.source_dir().join(&original)).expect("stage nested directory");
    fs::write(day.source_dir().join(&original).join("nested"), [0]).expect("nested entry");
    only_withheld(&day, &mut day.with_quality(), "not a regular file");
}

#[test]
fn misplaced_opening_hour_is_withheld() {
    let day = Day::today();
    let name = day.put(4, PROCESS_NS, 0, &[]);
    fs::rename(
        day.source_dir().join(name),
        day.source_dir().join(day.name(1)),
    )
    .expect("mislabel fixture hour");
    only_withheld(&day, &mut day.with_quality(), "wrong opening hour");
}

#[cfg(unix)]
#[test]
fn symlink_to_a_valid_recording_is_withheld() {
    let day = Day::today();
    let name = day.put(0, PROCESS_NS, 0, &[]);
    let link = day.source_dir().join(name);
    let target = day.root.join("synthetic-target");
    fs::rename(&link, &target).expect("move real fixture outside staging");
    std::os::unix::fs::symlink(&target, link).expect("create staged symlink");
    only_withheld(&day, &mut day.with_quality(), "not a regular file");
}

#[test]
fn header_requires_public_source_url_and_numbered_chain() {
    for (field, value, reason) in [
        ("source", json!("synthetic-other-source"), "wrong_source"),
        (
            "url",
            json!("wss://fixture.invalid/synthetic-private-id"),
            "wrong_url",
        ),
        ("file_seq", Value::Null, "invalid_file_open"),
        ("process_start_wall_ns", Value::Null, "invalid_file_open"),
        ("format", json!(2), "invalid_file_open"),
        ("event", json!("connecting"), "invalid_file_open"),
    ] {
        let day = Day::today();
        let mut open = header(PROCESS_NS, 0);
        if value.is_null() {
            open.as_object_mut().expect("header object").remove(field);
        } else {
            open[field] = value;
        }
        let raw = encode(MIDNIGHT_MS, PROCESS_NS, open, &[(RecvText, BTC_BBO)]);
        fs::write(day.source_dir().join(day.name(0)), compress(&raw, true))
            .expect("stage bad header");
        let manifest = only_withheld(&day, &mut day.with_quality(), reason);
        assert!(!manifest.to_string().contains("synthetic-other-source"));
        assert!(!manifest.to_string().contains("synthetic-private-id"));
    }
}

#[test]
fn rejected_sends_denied_metadata_and_unknown_events_never_escape() {
    const REJECTED: &str = r#"{"method":"synthetic-private-send"}"#;
    const DENIED: &str = "synthetic-private-id";
    for (kind, payload, reason) in [
        (SentText, REJECTED, "rejected_send"),
        (
            Meta,
            r#"{"event":"disconnected","reason":"lost synthetic-private-id here"}"#,
            "denied_meta",
        ),
        (
            Meta,
            r#"{"event":"disconnected","reason":"lost synthetic-\u0070rivate-id here"}"#,
            "denied_meta",
        ),
        (
            Meta,
            r#"{"event":"unknown-synthetic-event"}"#,
            "unknown_meta",
        ),
        (
            Meta,
            r#"{"event":"unknown-synthetic-event","event":"connected"}"#,
            "invalid_meta",
        ),
        (
            Meta,
            r#"{"event":"connected","other":{"synthetic-\u0070rivate-id":1}}"#,
            "denied_meta",
        ),
    ] {
        let day = Day::today();
        day.put(0, PROCESS_NS, 0, &[(RecvText, BTC_BBO), (kind, payload)]);
        let deny_file = day.root.join("deny-list");
        fs::write(
            &deny_file,
            format!("\r\n{DENIED}\r\nunused-private-fixture\r\n"),
        )
        .expect("write private synthetic deny list");
        let mut command = day.with_quality();
        command.arg("--deny-list").arg(deny_file);
        let manifest = only_withheld(&day, &mut command, reason);
        let json = manifest.to_string();
        for sensitive in [
            REJECTED,
            "synthetic-private-send",
            DENIED,
            "unused-private-fixture",
            "unknown-synthetic-event",
        ] {
            assert!(!json.contains(sensitive));
        }
    }
}

#[test]
fn unavailable_or_unfinished_quality_leaves_the_day_unsealed() {
    let day = Day::today();
    let name = day.put(0, PROCESS_NS, 0, &[]);
    let output = run(&mut day.command());
    assert_ne!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout).expect("UTF-8 report"),
        accepted(1, &name)
    );
    assert!(
        String::from_utf8(output.stderr)
            .expect("UTF-8 diagnostic")
            .contains("day not ready")
    );
    assert!(!day.manifest().exists());

    let mut missing_complete = quality(day.date);
    missing_complete
        .as_object_mut()
        .expect("quality object")
        .remove("complete");
    let mut interrupted = quality(day.date);
    interrupted["complete"] = false.into();
    let mut no_checks = quality(day.date);
    no_checks["checks"] = json!([]);
    for invalid in [
        missing_complete.to_string(),
        interrupted.to_string(),
        no_checks.to_string(),
        quality("2026-11-25").to_string(),
        String::from(r#"{"incomplete":"synthetic-private-detail""#),
    ] {
        fs::write(day.quality_file(), invalid).expect("write unfinished result");
        let output = run(&mut day.with_quality());
        assert_ne!(output.status.code(), Some(0));
        let all = [output.stdout, output.stderr].concat();
        assert!(
            !std::str::from_utf8(&all)
                .expect("UTF-8 output")
                .contains("synthetic-private-detail")
        );
        assert!(!day.manifest().exists());
    }
}

#[test]
fn empty_day_can_report_zero_coverage() {
    let day = Day::today();
    let mut coverage = quality(day.date);
    coverage["checks"][0]["compared"] = 0.into();
    fs::write(day.quality_file(), coverage.to_string()).expect("write zero coverage");
    let manifest = sealed(&day, &mut day.with_quality(), "");
    assert_eq!(manifest["files"], json!([]));
    assert_eq!(manifest["coins_covered"], json!({}));
    assert_eq!(manifest["quality"]["checks"][0]["compared"], 0);
}

#[test]
fn denied_quality_and_foreign_previous_path_cannot_enter_output() {
    const MARKER: &str = "synthetic-private-project";
    let day = Day::today();
    let previous = Day::yesterday();
    let anchor = previous.hour_ms(23) * 1_000_000;
    let previous_name = previous.put(23, anchor, 0, &[]);
    let mut changed = sealed(
        &previous,
        &mut previous.with_quality(),
        &accepted(1, &previous_name),
    );
    changed["files"][0]["path"] = format!("../{MARKER}/{previous_name}").into();
    fs::write(previous.manifest(), changed.to_string()).expect("alter synthetic previous link");
    day.put(0, anchor, 1, &[]);
    let mut command = day.with_quality();
    command.arg("--previous-manifest").arg(previous.manifest());
    unsealed(&day, &mut command, &[MARKER]);

    let mut denied = quality(day.date);
    denied["checks"][0]["subject"] = MARKER.into();
    fs::write(day.quality_file(), denied.to_string()).expect("set denied quality subject");
    let deny_list = day.root.join("private-deny-list");
    fs::write(&deny_list, MARKER).expect("set deny list");
    let mut command = day.with_quality();
    command.arg("--deny-list").arg(deny_list);
    unsealed(&day, &mut command, &[MARKER]);
}

fn unsealed(day: &Day, command: &mut std::process::Command, sensitive: &[&str]) -> String {
    let result = run(command);
    assert_ne!(result.status.code(), Some(0));
    assert!(!day.manifest().exists());
    let stdout = String::from_utf8(result.stdout).expect("UTF-8 report");
    let stderr = String::from_utf8(result.stderr).expect("UTF-8 diagnostic");
    for word in sensitive {
        assert!(!stdout.contains(word));
        assert!(!stderr.contains(word));
    }
    stdout
}

#[test]
fn retry_accepts_the_same_manifest_and_preserves_it_after_conflicting_input() {
    let day = Day::today();
    let name = day.put(0, PROCESS_NS, 0, &[]);
    let report = accepted(1, &name);
    let mut new_quality = quality(day.date);
    new_quality["checks"][1]["status"] = "pass".into();
    new_quality["checks"][1]["compared"] = 1.into();
    fs::write(day.quality_file(), new_quality.to_string()).expect("write completed pass");
    sealed(&day, &mut day.with_quality(), &report);
    let first = fs::read(day.manifest()).expect("first manifest");
    sealed(&day, &mut day.with_quality(), &report);
    new_quality["loader_revision"] = "changed".into();
    fs::write(day.quality_file(), new_quality.to_string()).expect("change quality input");
    let result = run(&mut day.with_quality());
    assert_ne!(result.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(result.stdout).expect("UTF-8 report"),
        report
    );
    assert_eq!(fs::read(day.manifest()).expect("protected manifest"), first);
}

#[test]
fn dates_and_delistings_belong_to_the_requested_utc_day() {
    let day = Day::today();
    let name = day.put(0, PROCESS_NS, 0, &[]);
    let mut earlier = day.expiring("2026-11-25");
    earlier.arg("--quality-results").arg(day.quality_file());
    assert!(unsealed(&day, &mut earlier, &[]).is_empty());
    for delisting in [
        "BTC=2026-11-25T23:59:59Z",
        "BTC=2026-11-26T24:00:00Z",
        "synthetic-private-coin=2026-11-26T12:00:00Z",
    ] {
        let mut command = day.with_quality();
        command.args(["--coverage-end", delisting]);
        assert!(unsealed(&day, &mut command, &["synthetic-private-coin"]).is_empty());
    }
    let mut same_day = day.expiring(day.date);
    same_day.arg("--quality-results").arg(day.quality_file());
    let manifest = sealed(&day, &mut same_day, &accepted(1, &name));
    assert_eq!(manifest["expires"], day.date);
}

#[test]
fn final_outputs_screen_rejected_sends_and_denied_control_words() {
    for (rejected, denied, subject) in [
        (
            "synthetic-rejected-subject",
            None,
            "synthetic-rejected-subject",
        ),
        ("unapproved", Some("withheld"), "BTC"),
        ("unapproved", Some("schema_version"), "BTC"),
    ] {
        let day = Day::today();
        day.put(0, PROCESS_NS, 0, &[(SentText, rejected)]);
        let mut results = quality(day.date);
        results["checks"][0]["subject"] = subject.into();
        fs::write(day.quality_file(), results.to_string())
            .expect("stage independent quality input");
        let mut command = day.with_quality();
        if let Some(denied) = denied {
            let list = day.root.join("deny-list");
            fs::write(&list, denied).expect("private control word");
            command.arg("--deny-list").arg(list);
        }
        let mut sensitive = vec![rejected];
        sensitive.extend(denied);
        let stdout = unsealed(&day, &mut command, &sensitive);
        assert!(stdout.is_empty() || stdout == withheld(1, "rejected_send"));
    }
}

#[test]
fn second_rejected_send_after_unknown_meta_stays_private_across_inputs() {
    const FIRST: &str = "synthetic-first-send";
    const SECOND: &str = "synthetic-second-send";
    let day = Day::today();
    day.put(
        0,
        PROCESS_NS,
        0,
        &[
            (Meta, r#"{"event":"synthetic-unknown-meta"}"#),
            (SentText, FIRST),
            (SentText, SECOND),
        ],
    );
    let mut results = quality(day.date);
    results["checks"][0]["subject"] = SECOND.into();
    fs::write(day.quality_file(), results.to_string()).expect("set colliding quality label");
    let stdout = unsealed(
        &day,
        &mut day.with_quality(),
        &[FIRST, SECOND, "synthetic-unknown-meta"],
    );
    assert_eq!(stdout, withheld(1, "unknown_meta"));
}
