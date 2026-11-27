use super::*;
use hl_types::{Liquidity, OrderId, TradeId};

const INSTRUMENT: InstrumentId = InstrumentId::new(7);

fn px(value: &str) -> Px {
    value.parse().expect("test price")
}

fn qty(value: &str) -> Qty {
    value.parse().expect("test quantity")
}

fn usd(value: &str) -> Usd {
    value.parse().expect("test amount")
}

fn fill(side: Side, price: &str, size: &str, fee: &str) -> Fill {
    Fill {
        tid: TradeId(11),
        oid: OrderId(13),
        side,
        px: px(price),
        qty: qty(size),
        liquidity: Liquidity::Maker,
        fee: usd(fee),
    }
}

fn assert_account(
    ledger: &Ledger,
    position: &str,
    cash: &str,
    fees: &str,
    notional: &str,
    gross: &str,
    net: &str,
) {
    assert_eq!(ledger.position(), qty(position));
    assert_eq!(ledger.cash(), usd(cash));
    assert_eq!(ledger.fees(), usd(fees));
    assert_eq!(ledger.notional(), usd(notional));
    assert_eq!(
        ledger.profit().expect("defined exact profit"),
        Profit {
            gross: usd(gross),
            net: usd(net),
        }
    );
}

#[test]
fn long_add_partial_close_flip_and_flat_with_positive_fees() {
    let mut ledger = Ledger::new(INSTRUMENT);
    assert_eq!(ledger.instrument(), INSTRUMENT);
    assert_eq!(ledger.mark(), None);
    assert_account(&ledger, "0", "0", "0", "0", "0", "0");
    ledger.apply_mark(px("105")).expect("initial mark");
    assert_eq!(ledger.mark(), Some(px("105")));

    // Buy 2 at 100. P=2, C=-200, F=0.2, N=200.
    // G=-200+2*105=10; net=10-0.2=9.8.
    ledger
        .apply_fill(INSTRUMENT, &fill(Side::Buy, "100", "2", "0.2"))
        .expect("open long");
    assert_account(&ledger, "2", "-200", "0.2", "200", "10", "9.8");

    // Buy 1 at 110. P=2+1=3, C=-200-110=-310, F=0.2+0.1=0.3,
    // N=200+110=310. G=-310+3*105=5; net=5-0.3=4.7.
    ledger
        .apply_fill(INSTRUMENT, &fill(Side::Buy, "110", "1", "0.1"))
        .expect("add long");
    assert_account(&ledger, "3", "-310", "0.3", "310", "5", "4.7");

    // Sell 1 at 120. P=3-1=2, C=-310+120=-190, F=0.3+0.1=0.4,
    // N=310+120=430. G=-190+2*105=20; net=20-0.4=19.6.
    ledger
        .apply_fill(INSTRUMENT, &fill(Side::Sell, "120", "1", "0.1"))
        .expect("partial close");
    assert_account(&ledger, "2", "-190", "0.4", "430", "20", "19.6");

    // Sell 3 at 90. P=2-3=-1, C=-190+270=80, F=0.4+0.3=0.7,
    // N=430+270=700. G=80-1*105=-25; net=-25-0.7=-25.7.
    ledger
        .apply_fill(INSTRUMENT, &fill(Side::Sell, "90", "3", "0.3"))
        .expect("flip to short");
    assert_account(&ledger, "-1", "80", "0.7", "700", "-25", "-25.7");

    // Buy 1 at 70. P=-1+1=0, C=80-70=10, F=0.7+0.1=0.8,
    // N=700+70=770. G=10+0*105=10; net=10-0.8=9.2.
    ledger
        .apply_fill(INSTRUMENT, &fill(Side::Buy, "70", "1", "0.1"))
        .expect("close to flat");
    assert_account(&ledger, "0", "10", "0.8", "770", "10", "9.2");

    // Flat G=10+0*M=10 and net=9.2 for both M=1 and M=999.
    for mark in ["1", "999"] {
        ledger.apply_mark(px(mark)).expect("flat mark");
        assert_account(&ledger, "0", "10", "0.8", "770", "10", "9.2");
    }
}

#[test]
fn long_add_partial_close_flip_and_flat_with_rebates() {
    let mut ledger = Ledger::new(INSTRUMENT);
    ledger.apply_mark(px("105")).expect("initial mark");

    // Buy 2 at 100. P=2, C=-200, F=-0.02, N=200.
    // G=-200+2*105=10; net=10-(-0.02)=10.02.
    ledger
        .apply_fill(INSTRUMENT, &fill(Side::Buy, "100", "2", "-0.02"))
        .expect("open long");
    assert_account(&ledger, "2", "-200", "-0.02", "200", "10", "10.02");

    // Buy 1 at 110. P=2+1=3, C=-200-110=-310, F=-0.02-0.01=-0.03,
    // N=200+110=310. G=-310+3*105=5; net=5-(-0.03)=5.03.
    ledger
        .apply_fill(INSTRUMENT, &fill(Side::Buy, "110", "1", "-0.01"))
        .expect("add long");
    assert_account(&ledger, "3", "-310", "-0.03", "310", "5", "5.03");

    // Sell 1 at 120. P=3-1=2, C=-310+120=-190, F=-0.03-0.01=-0.04,
    // N=310+120=430. G=-190+2*105=20; net=20-(-0.04)=20.04.
    ledger
        .apply_fill(INSTRUMENT, &fill(Side::Sell, "120", "1", "-0.01"))
        .expect("partial close");
    assert_account(&ledger, "2", "-190", "-0.04", "430", "20", "20.04");

    // Sell 3 at 90. P=2-3=-1, C=-190+270=80, F=-0.04-0.03=-0.07,
    // N=430+270=700. G=80-105=-25; net=-25-(-0.07)=-24.93.
    ledger
        .apply_fill(INSTRUMENT, &fill(Side::Sell, "90", "3", "-0.03"))
        .expect("flip to short");
    assert_account(&ledger, "-1", "80", "-0.07", "700", "-25", "-24.93");

    // Buy 1 at 70. P=-1+1=0, C=80-70=10, F=-0.07-0.01=-0.08,
    // N=700+70=770. G=10; net=10-(-0.08)=10.08.
    ledger
        .apply_fill(INSTRUMENT, &fill(Side::Buy, "70", "1", "-0.01"))
        .expect("close to flat");
    assert_account(&ledger, "0", "10", "-0.08", "770", "10", "10.08");

    // Flat G=10+0*M=10 and net=10.08 for both M=1 and M=999.
    for mark in ["1", "999"] {
        ledger.apply_mark(px(mark)).expect("flat mark");
        assert_account(&ledger, "0", "10", "-0.08", "770", "10", "10.08");
    }
}

#[test]
fn fractional_fills_and_new_mark_preserve_every_digit() {
    let mut ledger = Ledger::new(INSTRUMENT);
    ledger.apply_mark(px("100.15")).expect("initial mark");

    // Buy 0.00001 at 100.1 gives N=0.001001 and C=-0.001001.
    // M*P=100.15*0.00001=0.0010015, so G=0.0000005.
    // F=0.00000001 and net=0.0000005-0.00000001=0.00000049.
    ledger
        .apply_fill(
            INSTRUMENT,
            &fill(Side::Buy, "100.1", "0.00001", "0.00000001"),
        )
        .expect("first fractional fill");
    assert_account(
        &ledger,
        "0.00001",
        "-0.001001",
        "0.00000001",
        "0.001001",
        "0.0000005",
        "0.00000049",
    );

    // Buy 0.00002 at 100.2 adds 0.002004. P=0.00003,
    // C=-0.003005, N=0.003005, F=0.00000001+0.00000002=0.00000003.
    // M*P=100.15*0.00003=0.0030045, G=-0.0000005,
    // net=-0.0000005-0.00000003=-0.00000053.
    ledger
        .apply_fill(
            INSTRUMENT,
            &fill(Side::Buy, "100.2", "0.00002", "0.00000002"),
        )
        .expect("second fractional fill");
    assert_account(
        &ledger,
        "0.00003",
        "-0.003005",
        "0.00000003",
        "0.003005",
        "-0.0000005",
        "-0.00000053",
    );

    // At M=100.25, M*P=0.0030075, G=0.0030075-0.003005=0.0000025,
    // net=0.0000025-0.00000003=0.00000247. Other balances do not change.
    ledger.apply_mark(px("100.25")).expect("new mark");
    assert_account(
        &ledger,
        "0.00003",
        "-0.003005",
        "0.00000003",
        "0.003005",
        "0.0000025",
        "0.00000247",
    );
}

#[test]
fn unmarked_position_needs_a_mark_but_flat_profit_does_not() {
    let mut ledger = Ledger::new(INSTRUMENT);
    ledger
        .apply_fill(INSTRUMENT, &fill(Side::Buy, "100", "2", "0.2"))
        .expect("fill before first mark");
    // P=2, C=-200, F=0.2, N=200. With no M, P*M is undefined.
    assert_eq!(ledger.position(), qty("2"));
    assert_eq!(ledger.cash(), usd("-200"));
    assert_eq!(ledger.fees(), usd("0.2"));
    assert_eq!(ledger.notional(), usd("200"));
    let before = ledger;
    let error = ledger.profit().expect_err("missing mark");
    assert_eq!(error, LedgerError::MissingMark { position: qty("2") });
    assert_eq!(
        error.to_string(),
        "profit for position 2 requires a mark price"
    );
    assert_eq!(ledger, before);

    // Sell 2 at 110. P=0, C=-200+220=20, F=0.2+0.2=0.4,
    // N=200+220=420. G=20 and net=20-0.4=19.6 without a mark.
    ledger
        .apply_fill(INSTRUMENT, &fill(Side::Sell, "110", "2", "0.2"))
        .expect("close before first mark");
    assert_account(&ledger, "0", "20", "0.4", "420", "20", "19.6");
    assert_eq!(ledger.mark(), None);
}

#[test]
fn rejects_wrong_instrument_without_changing_state() {
    let mut ledger = Ledger::new(INSTRUMENT);
    ledger.apply_mark(px("100")).expect("initial mark");
    let before = ledger;
    let error = ledger
        .apply_fill(InstrumentId::new(8), &fill(Side::Buy, "100", "1", "0.1"))
        .expect_err("different instrument");
    assert_eq!(
        error,
        LedgerError::WrongInstrument {
            expected: INSTRUMENT,
            actual: InstrumentId::new(8),
        }
    );
    assert_eq!(
        error.to_string(),
        "fill instrument 8 does not match ledger instrument 7"
    );
    assert_eq!(ledger, before);
}

#[test]
fn rejects_inexact_fill_notional_without_rounding_or_mutation() {
    let mut ledger = Ledger::new(INSTRUMENT);
    let before = ledger;
    // 0.00000001*0.5=0.000000005, halfway between adjacent e8 amounts.
    let error = ledger
        .apply_fill(INSTRUMENT, &fill(Side::Buy, "0.00000001", "0.5", "0.1"))
        .expect_err("inexact fill product");
    assert_eq!(
        error,
        LedgerError::InexactProduct {
            operation: "fill notional",
            px: px("0.00000001"),
            qty: qty("0.5"),
        }
    );
    assert_eq!(
        error.to_string(),
        "inexact product for fill notional, price 0.00000001 times size 0.5"
    );
    assert_eq!(ledger, before);
}

#[test]
fn rejects_inexact_long_and_short_marks_without_changing_state() {
    for (side, position) in [(Side::Buy, "0.5"), (Side::Sell, "-0.5")] {
        let mut ledger = Ledger::new(INSTRUMENT);
        ledger.apply_mark(px("2")).expect("initial mark");
        ledger
            .apply_fill(INSTRUMENT, &fill(side, "2", "0.5", "0"))
            .expect("exact fill");
        let before = ledger;
        // Fill value 2*0.5=1 is exact. New position value is
        // 0.00000001*(+/-0.5)=+/-0.000000005, which is inexact.
        assert_eq!(
            ledger.apply_mark(px("0.00000001")),
            Err(LedgerError::InexactProduct {
                operation: "marked position value",
                px: px("0.00000001"),
                qty: qty(position),
            })
        );
        assert_eq!(ledger, before);
    }
}

#[test]
fn rejects_a_fill_whose_new_marked_position_is_inexact() {
    let mut ledger = Ledger::new(INSTRUMENT);
    ledger.apply_mark(px("0.00000001")).expect("flat mark");
    let before = ledger;
    // N=2*0.5=1 is exact, but M*P=0.00000001*0.5=0.000000005.
    assert_eq!(
        ledger.apply_fill(INSTRUMENT, &fill(Side::Buy, "2", "0.5", "0.1")),
        Err(LedgerError::InexactProduct {
            operation: "marked position value",
            px: px("0.00000001"),
            qty: qty("0.5"),
        })
    );
    assert_eq!(ledger, before);
}

#[test]
fn rejects_fill_product_overflow_without_changing_state() {
    let mut ledger = Ledger::new(INSTRUMENT);
    let before = ledger;
    // 90,000,000,000*2=180,000,000,000 exceeds the e8 i64 maximum
    // 92,233,720,368.54775807 despite both operands fitting.
    let error = ledger
        .apply_fill(INSTRUMENT, &fill(Side::Buy, "90000000000", "2", "0"))
        .expect_err("overflowing product");
    assert_eq!(
        error,
        LedgerError::Overflow {
            operation: "fill notional"
        }
    );
    assert_eq!(error.to_string(), "fixed-point overflow for fill notional");
    assert_eq!(ledger, before);
}

#[test]
fn rejects_position_overflow_without_changing_state() {
    let mut ledger = Ledger::new(INSTRUMENT);
    // P=90,000,000,000, C=-900, N=900 at a price of 0.00000001.
    ledger
        .apply_fill(
            INSTRUMENT,
            &fill(Side::Buy, "0.00000001", "90000000000", "0"),
        )
        .expect("large exact position");
    let before = ledger;
    // Added notional is 30, but P=93,000,000,000 would exceed i64 e8.
    assert_eq!(
        ledger.apply_fill(
            INSTRUMENT,
            &fill(Side::Buy, "0.00000001", "3000000000", "0")
        ),
        Err(LedgerError::Overflow {
            operation: "position"
        })
    );
    assert_eq!(ledger, before);
}

#[test]
fn rejects_cash_overflow_without_changing_state() {
    let mut ledger = Ledger::new(INSTRUMENT);
    ledger
        .apply_fill(INSTRUMENT, &fill(Side::Buy, "90000000000", "1", "0"))
        .expect("large exact cash debit");
    let before = ledger;
    // C=-90,000,000,000-3,000,000,000=-93,000,000,000 exceeds
    // the magnitude of the negative e8 i64 bound.
    assert_eq!(
        ledger.apply_fill(INSTRUMENT, &fill(Side::Buy, "3000000000", "1", "0")),
        Err(LedgerError::Overflow {
            operation: "trading cash"
        })
    );
    assert_eq!(ledger, before);
}

#[test]
fn rejects_fee_overflow_without_changing_state() {
    let mut ledger = Ledger::new(INSTRUMENT);
    ledger
        .apply_fill(
            INSTRUMENT,
            &fill(Side::Buy, "1", "1", "92233720368.54775807"),
        )
        .expect("maximum total fee before first mark");
    let before = ledger;
    // F=i64::MAX e8 + 0.00000001 is one raw unit beyond the bound.
    assert_eq!(
        ledger.apply_fill(INSTRUMENT, &fill(Side::Buy, "1", "1", "0.00000001")),
        Err(LedgerError::Overflow { operation: "fees" })
    );
    assert_eq!(ledger, before);
}

#[test]
fn rejects_accumulated_notional_overflow_without_changing_state() {
    let mut ledger = Ledger::new(INSTRUMENT);
    ledger
        .apply_fill(INSTRUMENT, &fill(Side::Buy, "90000000000", "1", "0"))
        .expect("large exact notional");
    let before = ledger;
    // Selling 1 at 3 billion gives P=0 and C=-87 billion, both in range.
    // N=90 billion+3 billion=93 billion is outside the e8 i64 range.
    assert_eq!(
        ledger.apply_fill(INSTRUMENT, &fill(Side::Sell, "3000000000", "1", "0")),
        Err(LedgerError::Overflow {
            operation: "traded notional"
        })
    );
    assert_eq!(ledger, before);
}

#[test]
fn rejects_marked_value_overflow_from_either_update() {
    let mut ledger = Ledger::new(INSTRUMENT);
    ledger.apply_mark(px("90000000000")).expect("flat mark");
    ledger
        .apply_fill(INSTRUMENT, &fill(Side::Buy, "1", "1", "0"))
        .expect("P=1 has an exact representable value");
    let before = ledger;
    // Adding 1 gives P=2 and M*P=180 billion, outside e8 i64.
    assert_eq!(
        ledger.apply_fill(INSTRUMENT, &fill(Side::Buy, "1", "1", "0")),
        Err(LedgerError::Overflow {
            operation: "marked position value"
        })
    );
    assert_eq!(ledger, before);

    ledger.apply_mark(px("1")).expect("smaller mark");
    ledger
        .apply_fill(INSTRUMENT, &fill(Side::Buy, "1", "1", "0"))
        .expect("P=2 with mark 1");
    let before = ledger;
    // Changing M to 90 billion produces the same overflowing M*P.
    assert_eq!(
        ledger.apply_mark(px("90000000000")),
        Err(LedgerError::Overflow {
            operation: "marked position value"
        })
    );
    assert_eq!(ledger, before);
}

#[test]
fn rejects_gross_overflow_without_changing_mark() {
    let mut ledger = Ledger::new(INSTRUMENT);
    ledger
        .apply_fill(INSTRUMENT, &fill(Side::Buy, "1", "3", "0"))
        .expect("buy three");
    ledger
        .apply_fill(INSTRUMENT, &fill(Side::Sell, "90000000000", "1", "0"))
        .expect("sell one");
    let before = ledger;
    // P=2, C=90 billion-3, N=90 billion+3 all fit.
    // At M=2 billion, G=90 billion-3+4 billion=93,999,999,997 overflows.
    assert_eq!(
        ledger.apply_mark(px("2000000000")),
        Err(LedgerError::Overflow {
            operation: "gross profit"
        })
    );
    assert_eq!(ledger, before);
}

#[test]
fn rejects_net_overflow_without_changing_state() {
    let mut ledger = Ledger::new(INSTRUMENT);
    ledger.apply_mark(px("1")).expect("initial mark");
    let before = ledger;
    // Buy 1 at 1 with M=1 gives G=-1+1=0. With F=i64::MIN e8,
    // net=0-(-92233720368.54775808) is one e8 unit above i64::MAX.
    assert_eq!(
        ledger.apply_fill(
            INSTRUMENT,
            &fill(Side::Buy, "1", "1", "-92233720368.54775808")
        ),
        Err(LedgerError::Overflow {
            operation: "net profit"
        })
    );
    assert_eq!(ledger, before);
}
