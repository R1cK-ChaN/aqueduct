//! Mark one instrument in exchange-time order and cross-check the final account.

use hl_ledger::{Ledger, LedgerError};
use hl_types::{
    Fill, InstrumentId, MarketEvent, MarketEventKind, OrderEvent, OrderEventKind, Px, Qty, SCALE,
    Side, Timestamp, Usd,
};

/// Final account and path extrema for one instrument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PnlSummary {
    /// Trading cash from delivered executions, before fees.
    pub cash: Usd,
    /// Signed final position.
    pub position: Qty,
    /// Last usable midpoint price.
    pub mark_px: Option<Px>,
    /// Exchange time of the last usable midpoint.
    pub mark_ts_exch: Option<Timestamp>,
    /// Total traded notional on both sides.
    pub notional: Usd,
    /// Total reported fees.
    pub fees: Usd,
    /// Final profit before fees.
    pub gross: Usd,
    /// Final profit after fees.
    pub net: Usd,
    /// Largest absolute position after any fill.
    pub max_abs_position: Qty,
    /// Lowest defined net profit, starting from zero.
    pub min_net: Usd,
    /// First exchange time at which the lowest negative net profit occurred.
    pub min_net_ts_exch: Option<Timestamp>,
}

enum Step<'a> {
    Mark { bid: Px, ask: Px },
    Fill(InstrumentId, &'a Fill),
}

/// The exchange time and prices needed to value one usable BBO after a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BboMark {
    time: Timestamp,
    bid: Px,
    ask: Px,
}

impl BboMark {
    /// Retains a two-sided, uncrossed quote for the selected instrument.
    pub fn from_event(instrument: InstrumentId, event: &MarketEvent) -> Option<Self> {
        if event.instrument == instrument
            && let MarketEventKind::Bbo(bbo) = &event.kind
            && let (Some(bid), Some(ask)) = (bbo.bid, bbo.ask)
            && bid.px < ask.px
        {
            Some(Self {
                time: event.ts_exch,
                bid: bid.px,
                ask: ask.px,
            })
        } else {
            None
        }
    }
}

fn timeline<'a>(
    marks: impl Iterator<Item = BboMark>,
    reports: &'a [OrderEvent],
) -> Vec<(Timestamp, Step<'a>)> {
    let mut steps: Vec<_> = marks
        .map(|BboMark { time, bid, ask }| (time, Step::Mark { bid, ask }))
        .collect();
    for report in reports {
        if let OrderEventKind::Filled(fill) = &report.kind {
            steps.push((report.ts_exch, Step::Fill(report.instrument, fill)));
        }
    }
    // All marks were inserted first. A stable sort puts them before tied fills
    // and keeps the input order within each kind.
    steps.sort_by_key(|(time, _)| *time);
    steps
}

fn describe_fill(fill: &Fill, time: Timestamp) -> String {
    format!(
        "fill tid={} oid={} side={:?} px={} qty={} ts_exch_ms={}",
        fill.tid.0,
        fill.oid.0,
        fill.side,
        fill.px,
        fill.qty,
        time.as_millis()
    )
}

/// Mark one instrument in exchange-time order and cross-check the final account.
pub fn summarize(
    instrument: InstrumentId,
    market: &[MarketEvent],
    reports: &[OrderEvent],
) -> Result<PnlSummary, String> {
    let marks = market
        .iter()
        .filter_map(|event| BboMark::from_event(instrument, event));
    account(instrument, marks, reports)
}

/// Values compact marks and delivered reports in the same order as [`summarize`].
/// Collect every mark with [`BboMark::from_event`] for this `instrument`.
pub fn summarize_marks(
    instrument: InstrumentId,
    marks: &[BboMark],
    reports: &[OrderEvent],
) -> Result<PnlSummary, String> {
    account(instrument, marks.iter().copied(), reports)
}

fn account(
    instrument: InstrumentId,
    marks: impl Iterator<Item = BboMark>,
    reports: &[OrderEvent],
) -> Result<PnlSummary, String> {
    let mut ledger = Ledger::new(instrument);
    let mut mark_time = None;
    let mut max_abs_position = Qty::ZERO;
    let mut min_net = Usd::ZERO;
    let mut min_net_time = None;

    for (time, step) in timeline(marks, reports) {
        match step {
            Step::Mark { bid, ask } => {
                // Px::midpoint floors a half unit. Accounting needs the exact mid.
                if !bid.as_e8().abs_diff(ask.as_e8()).is_multiple_of(2) {
                    return Err(format!(
                        "inexact midpoint for bid={bid} ask={ask} ts_exch_ms={}",
                        time.as_millis()
                    ));
                }
                let mid = bid.midpoint(ask);
                ledger.apply_mark(mid).map_err(|error| {
                    format!("mark {mid} at ts_exch_ms={}: {error}", time.as_millis())
                })?;
                mark_time = Some(time);
            }
            Step::Fill(id, fill) => {
                ledger
                    .apply_fill(id, fill)
                    .map_err(|error| format!("{}: {error}", describe_fill(fill, time)))?;
                let magnitude = ledger.position().as_e8().checked_abs().ok_or_else(|| {
                    format!(
                        "{}: fixed-point overflow in absolute position",
                        describe_fill(fill, time)
                    )
                })?;
                max_abs_position = max_abs_position.max(Qty::from_e8(magnitude));
            }
        }
        match ledger.profit() {
            Ok(profit) if profit.net < min_net => {
                min_net = profit.net;
                min_net_time = Some(time);
            }
            Ok(_) | Err(LedgerError::MissingMark { .. }) => {}
            Err(error) => return Err(error.to_string()),
        }
    }

    let profit = ledger.profit().map_err(|error| error.to_string())?;
    // Without a mark, profit() only succeeds when flat. The mark terms then
    // cancel in the independent sum, so zero is a valid reference price.
    verify_net(reports, ledger.mark().unwrap_or(Px::ZERO), profit.net)?;

    Ok(PnlSummary {
        cash: ledger.cash(),
        position: ledger.position(),
        mark_px: ledger.mark(),
        mark_ts_exch: mark_time,
        notional: ledger.notional(),
        fees: ledger.fees(),
        gross: profit.gross,
        net: profit.net,
        max_abs_position,
        min_net,
        min_net_ts_exch: min_net_time,
    })
}

/// Revalue the original fills at one price, independently of the ledger's
/// position, cash, accumulated fees and event ordering.
fn verify_net(reports: &[OrderEvent], mark: Px, ledger_net: Usd) -> Result<(), String> {
    let mut net_e8 = 0_i128;
    let scale = i128::from(SCALE);
    let mark_e8 = i128::from(mark.as_e8());
    for report in reports {
        let OrderEventKind::Filled(fill) = &report.kind else {
            continue;
        };
        let px_e8 = i128::from(fill.px.as_e8());
        let gain_per_unit = match fill.side {
            Side::Buy => mark_e8 - px_e8,
            Side::Sell => px_e8 - mark_e8,
        };
        // The difference of two i64 prices times an i64 quantity fits in i128.
        let product = gain_per_unit * i128::from(fill.qty.as_e8());
        if product % scale != 0 {
            return Err(format!(
                "{}: inexact product in independent net profit at mark {mark}",
                describe_fill(fill, report.ts_exch)
            ));
        }
        let contribution = product / scale - i128::from(fill.fee.as_e8());
        net_e8 = net_e8.checked_add(contribution).ok_or_else(|| {
            format!(
                "{}: overflow in independent net profit",
                describe_fill(fill, report.ts_exch)
            )
        })?;
    }
    let raw =
        i64::try_from(net_e8).map_err(|_| "independent net profit exceeds fixed-point range")?;
    let independent = Usd::from_e8(raw);
    if independent != ledger_net {
        return Err(format!(
            "net profit mismatch: ledger={ledger_net}, independent={independent}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use hl_types::{Bbo, ClientOrderId, Level, Liquidity, OrderId, TradeId};

    use super::*;

    const INSTRUMENT: InstrumentId = InstrumentId::new(1);

    fn fill(ms: i64, side: Side, px: &str, qty: &str, fee: &str) -> OrderEvent {
        OrderEvent {
            instrument: INSTRUMENT,
            cloid: ClientOrderId::from_u128(1),
            ts_exch: Timestamp::from_millis(ms),
            ts_local: Timestamp::from_millis(ms + 200),
            kind: OrderEventKind::Filled(Fill {
                tid: TradeId(1),
                oid: OrderId(2),
                side,
                px: px.parse().unwrap(),
                qty: qty.parse().unwrap(),
                liquidity: Liquidity::Maker,
                fee: fee.parse().unwrap(),
            }),
        }
    }

    fn quote(ms: i64, bid: &str, ask: &str) -> MarketEvent {
        let level = |price: &str| {
            Some(Level {
                px: price.parse().unwrap(),
                qty: Qty::from_e8(SCALE),
                orders: 1,
            })
        };
        MarketEvent {
            instrument: INSTRUMENT,
            ts_exch: Timestamp::from_millis(ms),
            ts_local: Timestamp::from_millis(ms + 200),
            kind: MarketEventKind::Bbo(Bbo {
                bid: level(bid),
                ask: level(ask),
            }),
        }
    }

    #[test]
    fn independent_check_rejects_one_unit_of_difference() {
        let fills = [
            fill(1, Side::Buy, "10", "2", "0.1"),
            fill(2, Side::Sell, "15", "0.5", "-0.02"),
        ];
        // (12 - 10)*2 + (15 - 12)*0.5 - 0.1 + 0.02 = 5.42.
        let mark = "12".parse().unwrap();
        verify_net(&fills, mark, "5.42".parse().unwrap()).unwrap();
        let error = verify_net(&fills, mark, "5.42000001".parse().unwrap()).unwrap_err();
        assert_eq!(
            error,
            "net profit mismatch: ledger=5.42000001, independent=5.42"
        );
    }

    #[test]
    fn an_unrepresentable_midpoint_is_an_error() {
        // (1 + 1.00000001)/2 = 1.000000005, halfway between e8 units.
        let event = quote(42, "1", "1.00000001");
        let mark = BboMark::from_event(INSTRUMENT, &event).unwrap();
        let expected = "inexact midpoint for bid=1 ask=1.00000001 ts_exch_ms=42";
        assert_eq!(summarize(INSTRUMENT, &[event], &[]).unwrap_err(), expected);
        assert_eq!(
            summarize_marks(INSTRUMENT, &[mark], &[]).unwrap_err(),
            expected
        );
    }

    #[test]
    fn compact_marks_keep_only_usable_quotes_for_the_instrument() {
        assert!(std::mem::size_of::<BboMark>() <= 24);
        let mut other = quote(1, "10", "12");
        other.instrument = InstrumentId::new(2);
        let mut one_sided = quote(2, "10", "12");
        if let MarketEventKind::Bbo(bbo) = &mut one_sided.kind {
            bbo.bid = None;
        }
        let events = [
            other,
            one_sided,
            quote(3, "12", "12"),
            quote(4, "13", "12"),
            quote(5, "11", "13"),
        ];
        let marks: Vec<_> = events
            .iter()
            .filter_map(|e| BboMark::from_event(INSTRUMENT, e))
            .collect();
        assert_eq!(marks.len(), 1);
        let pnl = summarize_marks(INSTRUMENT, &marks, &[]).unwrap();
        assert_eq!(pnl.mark_px, Some("12".parse().unwrap()));
        assert_eq!(pnl.mark_ts_exch, Some(Timestamp::from_millis(5)));
        assert_eq!(pnl, summarize(INSTRUMENT, &events, &[]).unwrap());
    }
}
