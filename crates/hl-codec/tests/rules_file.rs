//! The dated perpetual rules file and run selection from RFC 0009 D7.

use hl_codec::hyperliquid::{Network, RulesFileError, instrument_id, parse_rules_file};
use hl_rules::{RuleError, RuleVersion};
use hl_types::Timestamp;
use serde_json::{Value, json};

const EXAMPLE: &str = r#"{
  "instruments": [
    {
      "network": "testnet",
      "coin": "SYNTH",
      "versions": [
        {"effective_from": "100", "sz_decimals": 2, "asset_id": "17"},
        {"effective_from": "200", "sz_decimals": 3, "asset_id": "21"}
      ]
    }
  ]
}"#;

const INSTRUMENT: &str = "/instruments/0";
const VERSION: &str = "/instruments/0/versions/0";
const FIELDS: [(&str, &str); 7] = [
    ("", "instruments"),
    (INSTRUMENT, "network"),
    (INSTRUMENT, "coin"),
    (INSTRUMENT, "versions"),
    (VERSION, "effective_from"),
    (VERSION, "sz_decimals"),
    (VERSION, "asset_id"),
];

fn example() -> Value {
    serde_json::from_str(EXAMPLE).expect("D7 example is JSON")
}

fn instrument(network: Network, coin: &str) -> Value {
    let mut history = example()["instruments"][0].take();
    history["network"] = json!(network.short_name());
    history["coin"] = json!(coin);
    history
}

fn version(start: i64, decimals: u8, asset: u64) -> RuleVersion {
    RuleVersion::new(Timestamp::from_nanos(start), decimals, asset).expect("valid version")
}

fn check_layout_error(text: &str) -> String {
    let error = parse_rules_file(text).expect_err("invalid file");
    assert!(
        matches!(error, RulesFileError::Json(_)),
        "{error} for {text}"
    );
    error.to_string()
}

#[test]
fn example_selects_both_versions_at_their_inclusive_starts() {
    let file = parse_rules_file(EXAMPLE).expect("D7 example");
    let rules = file
        .select(Network::Testnet, "SYNTH")
        .expect("example pair");
    let id = instrument_id(Network::Testnet, "SYNTH");
    assert_eq!(rules.instrument_id, id);
    let first = version(100, 2, 17);
    let second = version(200, 3, 21);
    assert_eq!(rules.versions, [first, second]);
    let before = Timestamp::from_nanos(99);
    assert_eq!(
        rules.book.at(id, before),
        Err(RuleError::MissingCoverage {
            ts_exch: before,
            first_start: first.effective_from(),
        })
    );
    for (time, expected) in [(100, &first), (199, &first), (200, &second)] {
        assert_eq!(rules.book.at(id, Timestamp::from_nanos(time)), Ok(expected));
    }
}

#[test]
fn same_coin_on_two_networks_has_distinct_histories() {
    let mut mainnet = instrument(Network::Mainnet, "SYNTH");
    mainnet["versions"][0]["asset_id"] = json!("98");
    let testnet = instrument(Network::Testnet, "SYNTH");
    let text = json!({"instruments": [mainnet, testnet]}).to_string();
    let file = parse_rules_file(&text).expect("distinct pairs");
    let time = Timestamp::from_nanos(100);
    let main = file
        .select(Network::Mainnet, "SYNTH")
        .expect("mainnet history");
    let test = file
        .select(Network::Testnet, "SYNTH")
        .expect("testnet history");
    assert_ne!(main.instrument_id, test.instrument_id);
    for (rules, network, asset, other_id) in [
        (&main, Network::Mainnet, 98, test.instrument_id),
        (&test, Network::Testnet, 17, main.instrument_id),
    ] {
        let id = instrument_id(network, "SYNTH");
        assert_eq!(rules.instrument_id, id);
        assert_eq!(rules.versions[0], version(100, 2, asset));
        assert_eq!(rules.book.at(id, time), Ok(&rules.versions[0]));
        assert_eq!(
            rules.book.at(other_id, time),
            Err(RuleError::UnknownInstrument(other_id))
        );
    }
}

#[test]
fn accepts_unordered_fields_and_preserves_the_decoded_coin() {
    let text = r#"{
      "instruments": [{
        "versions": [ { "asset_id" : "17", "effective_from" : "100", "sz_decimals" : 2 } ],
        "coin" : "xy\u007a:NVDA", "network" : "mainnet"
      }, {
        "coin" : " xyz:NvDa\t", "network" : "mainnet",
        "versions" : [{"sz_decimals":3, "asset_id":"21", "effective_from":"100"}]
      }]
    }"#;
    let file = parse_rules_file(text).expect("field order and JSON whitespace");
    for (coin, asset) in [("xyz:NVDA", 17), (" xyz:NvDa\t", 21)] {
        let rules = file.select(Network::Mainnet, coin).expect("exact coin");
        assert_eq!(rules.instrument_id, instrument_id(Network::Mainnet, coin));
        assert_eq!(rules.versions[0].asset_id(), asset);
    }
    for coin in ["NVDA", "xyz:nvda", "xyz:NvDa", r"xyz:A\B"] {
        let error = file
            .select(Network::Mainnet, coin)
            .expect_err("different coin");
        assert!(matches!(error, RulesFileError::MissingPair { .. }));
        assert!(error.to_string().contains(coin));
    }
}

#[test]
fn accepts_integer_boundaries() {
    let mut doc = example();
    doc["instruments"][0]["versions"] = json!([
        {"effective_from": "-9223372036854775808", "sz_decimals": 0, "asset_id": "0"},
        {"effective_from": "-1", "sz_decimals": 6, "asset_id": "18446744073709551615"},
        {"effective_from": "0", "sz_decimals": 6, "asset_id": "0"},
        {"effective_from": "9223372036854775807", "sz_decimals": 0, "asset_id": "1"}
    ]);
    let file = parse_rules_file(&doc.to_string()).expect("inclusive bounds");
    let rules = file
        .select(Network::Testnet, "SYNTH")
        .expect("example pair");
    assert_eq!(
        rules.versions,
        [
            version(i64::MIN, 0, 0),
            version(-1, 6, u64::MAX),
            version(0, 6, 0),
            version(i64::MAX, 0, 1),
        ]
    );
}

#[test]
fn rejects_invalid_json_and_trailing_input() {
    check_layout_error(r#"{"instruments":"#);
    check_layout_error(&format!("{EXAMPLE} {{}}"));
}

#[test]
fn rejects_unknown_fields_at_each_level() {
    for path in ["", INSTRUMENT, VERSION] {
        let mut doc = example();
        doc.pointer_mut(path).expect("object")["unrecognized"] = json!(0);
        assert!(check_layout_error(&doc.to_string()).contains("unrecognized"));
    }
}

#[test]
fn rejects_each_missing_field() {
    for (parent, field) in FIELDS {
        let mut doc = example();
        let object = doc
            .pointer_mut(parent)
            .expect("parent")
            .as_object_mut()
            .expect("object");
        object.remove(field).expect("required field");
        assert!(check_layout_error(&doc.to_string()).contains(field));
    }
}

#[test]
fn rejects_duplicate_keys_in_raw_json_at_each_level() {
    let doc = example();
    for (parent, field) in FIELDS {
        let value = &doc.pointer(parent).expect("field parent")[field];
        let key = format!(r#""{field}":"#);
        // Each occurrence has the same valid value. Keeping either one would accept the file.
        let text = EXAMPLE.replacen(&key, &format!("{key}{value}, {key}"), 1);
        let message = check_layout_error(&text);
        assert!(
            message.contains("duplicate field") && message.contains(field),
            "{message}"
        );
    }
    let text = EXAMPLE.replace(
        r#""coin": "SYNTH""#,
        r#""coin": "SYNTH", "\u0063oin": "SYNTH""#,
    );
    let message = check_layout_error(&text);
    assert!(
        message.contains("duplicate field") && message.contains("coin"),
        "{message}"
    );
}

#[test]
fn rejects_wrong_json_types() {
    for path in ["", INSTRUMENT, VERSION] {
        for wrong in [Value::Null, json!(0), json!("text")] {
            let mut doc = example();
            *doc.pointer_mut(path).expect("object position") = wrong;
            check_layout_error(&doc.to_string());
        }
    }
    for (parent, field) in FIELDS {
        for wrong in [Value::Null, json!({"mainnet": null}), json!(true)] {
            let mut doc = example();
            doc.pointer_mut(parent).expect("field parent")[field] = wrong;
            check_layout_error(&doc.to_string());
        }
    }
    for (parent, field) in [
        (INSTRUMENT, "network"),
        (INSTRUMENT, "coin"),
        (VERSION, "effective_from"),
        (VERSION, "asset_id"),
    ] {
        let mut doc = example();
        doc.pointer_mut(parent).expect("field parent")[field] = json!(2);
        check_layout_error(&doc.to_string());
    }
}

#[test]
fn rejects_positional_arrays_in_place_of_objects() {
    let doc = example();
    check_layout_error(&json!([doc["instruments"]]).to_string());
    let mut doc = example();
    let versions = &doc["instruments"][0]["versions"];
    doc["instruments"][0] = json!(["testnet", "SYNTH", versions]);
    check_layout_error(&doc.to_string());
    let mut doc = example();
    doc["instruments"][0]["versions"][0] = json!(["100", 2, "17"]);
    check_layout_error(&doc.to_string());
}

#[test]
fn rejects_empty_arrays() {
    check_layout_error(r#"{"instruments":[]}"#);
    let mut doc = example();
    doc["instruments"][0]["versions"] = json!([]);
    let error = parse_rules_file(&doc.to_string()).expect_err("empty versions");
    assert!(matches!(
        error,
        RulesFileError::Rule(RuleError::EmptyHistory)
    ));
}

#[test]
fn rejects_unknown_networks_and_blank_coins() {
    for (field, values) in [
        ("network", &["other", "Testnet", "mainnet "][..]),
        ("coin", &["", " ", "\t", " \t ", "\u{2003}"][..]),
    ] {
        for value in values {
            let mut doc = example();
            doc["instruments"][0][field] = json!(value);
            check_layout_error(&doc.to_string());
        }
    }
}

fn check_integer_error(field: &str, input: &str) {
    let mut doc = example();
    doc["instruments"][0]["versions"][0][field] = json!(input);
    let error = parse_rules_file(&doc.to_string()).expect_err("invalid integer string");
    assert!(
        matches!(&error, RulesFileError::Integer { field: name, value }
        if *name == field && value == input),
        "{error} for {field} = {input:?}"
    );
    assert!(error.to_string().contains(field));
}

#[test]
fn rejects_noncanonical_integer_strings() {
    for field in ["asset_id", "effective_from"] {
        for input in [
            "", "01", "-0", "+1", "1.5", "1e0", "1 0", " 1", "1 ", "\t1", "1\n", "１",
        ] {
            check_integer_error(field, input);
        }
    }
    for input in ["-01", "-", "9223372036854775808", "-9223372036854775809"] {
        check_integer_error("effective_from", input);
    }
    for input in ["-1", "18446744073709551616"] {
        check_integer_error("asset_id", input);
    }
}

#[test]
fn size_decimals_requires_a_single_integer_digit_from_zero_through_six() {
    for raw in ["-1", "-0", "2.0", "1e0", r#""2""#, "true", "256"] {
        let text = EXAMPLE.replacen(
            r#""sz_decimals": 2"#,
            &format!(r#""sz_decimals": {raw}"#),
            1,
        );
        check_layout_error(&text);
    }
    let mut doc = example();
    doc["instruments"][0]["versions"][0]["sz_decimals"] = json!(7);
    let error = parse_rules_file(&doc.to_string()).expect_err("unsupported precision");
    assert!(matches!(
        error,
        RulesFileError::Rule(RuleError::UnsupportedSizeDecimals(7))
    ));
    assert!(error.to_string().contains("sz_decimals"));
}

#[test]
fn rejects_equal_and_decreasing_starts_in_the_supplied_order() {
    for next in [100, 99] {
        let mut doc = example();
        doc["instruments"][0]["versions"][1]["effective_from"] = json!(next.to_string());
        let error = parse_rules_file(&doc.to_string()).expect_err("out-of-order history");
        let expected = RuleError::NonIncreasingStart {
            previous: Timestamp::from_nanos(100),
            next: Timestamp::from_nanos(next),
        };
        assert!(matches!(error, RulesFileError::Rule(reason) if reason == expected));
    }
}

#[test]
fn validates_all_histories_before_selection() {
    let first = instrument(Network::Testnet, "SYNTH");
    let mut second = instrument(Network::Testnet, "OTHER");
    second["versions"][1]["asset_id"] = json!("01");
    let text = json!({"instruments": [first, second]}).to_string();
    let error = parse_rules_file(&text).expect_err("invalid last version of the last history");
    assert!(matches!(
        error,
        RulesFileError::Integer {
            field: "asset_id",
            ..
        }
    ));
}

#[test]
fn rejects_a_repeated_pair() {
    let history = instrument(Network::Testnet, "SYNTH");
    let other = instrument(Network::Mainnet, "SYNTH");
    let text = json!({"instruments": [history, other, history]}).to_string();
    let error = parse_rules_file(&text).expect_err("repeated pair");
    assert!(
        matches!(&error, RulesFileError::DuplicatePair { network: Network::Testnet, coin } if coin == "SYNTH")
    );
    assert!(error.to_string().contains("testnet"));
    assert!(error.to_string().contains("SYNTH"));
}

#[test]
fn rejects_colliding_ids_and_selects_only_exact_pairs() {
    let first_coin = "SYNTH-16935";
    let second_coin = "SYNTH-54110";
    let id = instrument_id(Network::Mainnet, first_coin);
    assert_eq!(id, instrument_id(Network::Testnet, second_coin));
    let first = instrument(Network::Mainnet, first_coin);
    let second = instrument(Network::Testnet, second_coin);

    let text = json!({"instruments": [first]}).to_string();
    let file = parse_rules_file(&text).expect("one owner");
    let error = file
        .select(Network::Testnet, second_coin)
        .expect_err("missing pair");
    assert!(
        matches!(&error, RulesFileError::MissingPair { network: Network::Testnet, coin } if coin == second_coin)
    );
    assert!(error.to_string().contains("testnet"));
    assert!(error.to_string().contains(second_coin));

    let text = json!({"instruments": [first, second]}).to_string();
    let error = parse_rules_file(&text).expect_err("two owners of one id");
    assert!(
        matches!(&error, RulesFileError::IdCollision { id: actual, first, second }
        if *actual == id && first.0 == Network::Mainnet && first.1 == first_coin
        && second.0 == Network::Testnet && second.1 == second_coin)
    );
    assert!(error.to_string().contains(first_coin));
    assert!(error.to_string().contains(second_coin));
    assert!(error.to_string().contains(&id.to_string()));
}
