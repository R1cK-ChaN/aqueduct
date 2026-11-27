//! Session parameters must reconstruct each component in its native units.

use std::time::Duration;

use hl_sim::{MODEL_NAME, SimConfig};
use hl_strategy::FixedSpreadConfig;
use hl_types::InstrumentId;

fn pairs<const N: usize>(entries: [(&str, &str); N]) -> Vec<(String, String)> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
}

fn rejects_bad_entries(
    entries: &[(String, String)],
    accepts: impl Fn(&[(String, String)]) -> bool,
) {
    for index in 0..entries.len() {
        let mut missing = entries.to_vec();
        missing.remove(index);
        assert!(!accepts(&missing));
        let mut unknown = entries.to_vec();
        unknown[index].0 = "unrecognized".into();
        assert!(!accepts(&unknown));
        let mut malformed = entries.to_vec();
        malformed[index].1 = "0.5e1".into();
        assert!(!accepts(&malformed));
    }
    let mut duplicate = entries.to_vec();
    duplicate.push(entries[0].clone());
    assert!(!accepts(&duplicate));
}

#[test]
fn fixed_spread_lists_and_rebuilds_every_parameter_except_the_instrument() {
    let instrument = InstrumentId::new(7);
    let config = FixedSpreadConfig {
        instrument,
        half_spread: "0.015".parse().unwrap(),
        qty: "0.17".parse().unwrap(),
    };
    let expected = pairs([
        ("half_spread_quote_per_base", "0.015"),
        ("qty_base", "0.17"),
    ]);
    assert_eq!(config.parameters(), expected);
    let rebuilt = FixedSpreadConfig::from_parameters(instrument, &expected).unwrap();
    assert_eq!(rebuilt, config);
    assert_eq!(rebuilt.parameters(), expected);
    rejects_bad_entries(&expected, |entries| {
        FixedSpreadConfig::from_parameters(instrument, entries).is_ok()
    });
    for (index, value) in [(0, "-2"), (1, "0")] {
        let mut bad = expected.clone();
        bad[index].1 = value.into();
        assert!(FixedSpreadConfig::from_parameters(instrument, &bad).is_err());
    }
}

#[test]
fn simulator_lists_exact_nanoseconds_and_signed_basis_points() {
    assert_eq!(MODEL_NAME, "strict_trade_through");
    let config = SimConfig {
        order_latency: Duration::from_nanos(400_000_003),
        report_latency: Duration::ZERO,
        maker_fee: "-0.00000001".parse().unwrap(),
        min_order_notional: "10".parse().unwrap(),
    };
    let expected = pairs([
        ("order_latency_ns", "400000003"),
        ("report_latency_ns", "0"),
        ("maker_fee_bps", "-0.0001"),
        ("min_order_notional_usd", "10"),
    ]);
    assert_eq!(config.parameters(), expected);
    let rebuilt = SimConfig::from_parameters(&expected).unwrap();
    assert_eq!(rebuilt, config);
    assert_eq!(rebuilt.parameters(), expected);
    rejects_bad_entries(&expected, |entries| {
        SimConfig::from_parameters(entries).is_ok()
    });
    for (index, value) in [
        (0, "-1"),
        (1, "9223372036854775807"),
        (2, "0.00001"),
        (2, "10001"),
        (3, "-1"),
    ] {
        let mut bad = expected.clone();
        bad[index].1 = value.into();
        assert!(SimConfig::from_parameters(&bad).is_err());
    }
}
