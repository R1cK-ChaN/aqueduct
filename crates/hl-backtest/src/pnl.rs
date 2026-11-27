//! JSON formatting for the typed account summary.

use hl_run::pnl::PnlSummary;
use hl_types::Timestamp;
use serde_json::json;

use crate::summary::JsonObject;

pub(super) fn json(pnl: &PnlSummary) -> JsonObject {
    JsonObject::new([
        ("excluded", json!(["funding"])),
        ("position", json!(pnl.position.to_string())),
        ("mark_px", json!(pnl.mark_px.map(|px| px.to_string()))),
        (
            "mark_ts_exch_ms",
            json!(pnl.mark_ts_exch.map(Timestamp::as_millis)),
        ),
        ("notional", json!(pnl.notional.to_string())),
        ("fees", json!(pnl.fees.to_string())),
        ("gross", json!(pnl.gross.to_string())),
        ("net", json!(pnl.net.to_string())),
        ("max_abs_position", json!(pnl.max_abs_position.to_string())),
        ("min_net", json!(pnl.min_net.to_string())),
        (
            "min_net_ts_exch_ms",
            json!(pnl.min_net_ts_exch.map(Timestamp::as_millis)),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use hl_types::{
        Bbo, ClientOrderId, Fill, InstrumentId, Level, Liquidity, MarketEvent, MarketEventKind,
        OrderEvent, OrderEventKind, OrderId, Qty, SCALE, Side, TradeId,
    };

    use super::*;

    fn summarize(
        instrument: InstrumentId,
        market: &[MarketEvent],
        reports: &[OrderEvent],
    ) -> Result<JsonObject, String> {
        hl_run::pnl::summarize(instrument, market, reports).map(|pnl| json(&pnl))
    }

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
    fn a_flat_round_trip_can_be_valued_without_a_mark() {
        let fills = [
            fill(1, Side::Buy, "12", "1", "0.1"),
            fill(2, Side::Sell, "10", "1", "0.02"),
        ];
        // The open position at 1 ms has no mark. At 2 ms P=0, C=10-12=-2
        // and F=0.1+0.02=0.12. The first defined loss is also the final net.
        let pnl = summarize(INSTRUMENT, &[], &fills).unwrap();
        let pnl: serde_json::Value = serde_json::from_str(&pnl.to_string()).unwrap();
        assert_eq!(pnl["mark_px"], json!(null));
        assert_eq!(pnl["gross"], "-2");
        assert_eq!(pnl["net"], "-2.12");
        assert_eq!(pnl["max_abs_position"], "1");
        assert_eq!(pnl["min_net"], "-2.12");
        assert_eq!(pnl["min_net_ts_exch_ms"], 2);
    }

    #[test]
    fn tied_fills_keep_the_short_peak_and_a_repeated_low_keeps_its_first_time() {
        let fills = [
            fill(20, Side::Sell, "10", "2", "0"),
            fill(20, Side::Buy, "10", "1", "0"),
            fill(60, Side::Buy, "10", "1", "0"),
        ];
        let quotes = [
            quote(10, "9", "11"),
            quote(30, "11", "13"),
            quote(40, "9", "11"),
            quote(50, "11", "13"),
        ];
        // The two fills at 20 ms move P from 0 to -2 to -1, so max |P|=2.
        // C=2*10-10=10. At both 30 and 50 ms, net=10-1*12=-2.
        // Covering the last unit at 10 brings the position and cash to zero.
        let pnl = summarize(INSTRUMENT, &quotes, &fills).unwrap();
        let pnl: serde_json::Value = serde_json::from_str(&pnl.to_string()).unwrap();
        assert_eq!(pnl["max_abs_position"], "2");
        assert_eq!(pnl["min_net"], "-2");
        assert_eq!(pnl["min_net_ts_exch_ms"], 30);
        assert_eq!(pnl["position"], "0");
        assert_eq!(pnl["net"], "0");
    }
}
