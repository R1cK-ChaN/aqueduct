//! A reference strategy used to exercise the interfaces.

use hl_types::{Bbo, Event, InstrumentId, MarketEventKind, OrderSpec, Px, Qty, Side, TimeInForce};

use crate::actions::Actions;
use crate::host::{Context, Strategy};

/// Settings for [`FixedSpreadQuoter`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixedSpreadConfig {
    /// Instrument to quote.
    pub instrument: InstrumentId,
    /// Distance of each quote from the mid price. Must be positive.
    pub half_spread: Px,
    /// Size of each quote. Must be positive.
    pub qty: Qty,
}

impl FixedSpreadConfig {
    /// Exact session parameters in output order. The session names the instrument separately.
    pub fn parameters(&self) -> Vec<(String, String)> {
        vec![
            (
                "half_spread_quote_per_base".into(),
                self.half_spread.to_string(),
            ),
            ("qty_base".into(), self.qty.to_string()),
        ]
    }

    /// Rebuilds a quoter configuration from its two positive decimal parameters.
    pub fn from_parameters(
        instrument: InstrumentId,
        pairs: &[(String, String)],
    ) -> Result<Self, String> {
        if pairs.len() != 2 {
            return Err("expected exactly half_spread_quote_per_base and qty_base".into());
        }
        let value = |key: &str| {
            pairs
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.as_str())
                .ok_or_else(|| format!("missing {key}"))
        };
        let half_spread = value("half_spread_quote_per_base")?
            .parse::<Px>()
            .map_err(|e| format!("half_spread_quote_per_base: {e}"))?;
        let qty = value("qty_base")?
            .parse::<Qty>()
            .map_err(|e| format!("qty_base: {e}"))?;
        if !half_spread.is_positive() || !qty.is_positive() {
            return Err("half_spread_quote_per_base and qty_base must be positive".into());
        }
        Ok(Self {
            instrument,
            half_spread,
            qty,
        })
    }
}

/// Keeps one post-only bid and one post-only ask at a fixed distance from the
/// mid price of the best bid and ask.
///
/// This exists to prove that a strategy can be wired to different gateways
/// without change. It is not a trading strategy: it ignores inventory, fees,
/// tick size and adverse selection.
///
/// Behaviour:
///
/// - On a two-sided [`Bbo`] for its instrument it places a missing quote, or
///   modifies a resting quote whose price or open size differs from the
///   target. A quote with a request in flight is left alone.
/// - On [`MarketEventKind::FeedGap`] it cancels all its orders on the
///   instrument.
/// - Every other event is ignored.
#[derive(Debug)]
pub struct FixedSpreadQuoter {
    config: FixedSpreadConfig,
}

impl FixedSpreadQuoter {
    /// Creates the strategy.
    ///
    /// # Panics
    ///
    /// Panics when `half_spread` or `qty` is not positive.
    pub fn new(config: FixedSpreadConfig) -> Self {
        assert!(
            config.half_spread.is_positive(),
            "half_spread must be positive"
        );
        assert!(config.qty.is_positive(), "qty must be positive");
        Self { config }
    }

    fn quote(&self, ctx: &Context<'_>, actions: &mut Actions, side: Side, px: Px) {
        let FixedSpreadConfig {
            instrument, qty, ..
        } = self.config;
        let working = ctx
            .orders()
            .for_instrument(instrument)
            .find(|order| order.side == side);
        match working {
            None => {
                actions.place(OrderSpec {
                    instrument,
                    side,
                    px,
                    qty,
                    tif: TimeInForce::Alo,
                    reduce_only: false,
                });
            }
            Some(order) if order.state.is_pending() => {}
            Some(order) if order.px != px || order.open_qty != qty => {
                actions.modify(instrument, order.cloid, px, qty);
            }
            Some(_) => {}
        }
    }
}

impl Strategy for FixedSpreadQuoter {
    fn on_event(&mut self, ctx: &Context<'_>, event: &Event, actions: &mut Actions) {
        let Event::Market(market) = event else {
            return;
        };
        if market.instrument != self.config.instrument {
            return;
        }
        match &market.kind {
            MarketEventKind::Bbo(Bbo {
                bid: Some(bid),
                ask: Some(ask),
            }) => {
                let mid = bid.px.midpoint(ask.px);
                self.quote(ctx, actions, Side::Buy, mid - self.config.half_spread);
                self.quote(ctx, actions, Side::Sell, mid + self.config.half_spread);
            }
            MarketEventKind::FeedGap => actions.cancel_all(Some(self.config.instrument)),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use hl_types::{
        ClientOrderId, Level, MarketEvent, OrderAction, OrderEvent, OrderEventKind, OrderId,
        Timestamp,
    };

    use super::*;
    use crate::host::StrategyHost;

    const MON: InstrumentId = InstrumentId::new(7);
    const OTHER: InstrumentId = InstrumentId::new(8);

    fn px(text: &str) -> Px {
        text.parse().unwrap()
    }

    fn qty(text: &str) -> Qty {
        text.parse().unwrap()
    }

    fn host() -> StrategyHost<FixedSpreadQuoter> {
        let quoter = FixedSpreadQuoter::new(FixedSpreadConfig {
            instrument: MON,
            half_spread: px("0.0002"),
            qty: qty("500"),
        });
        StrategyHost::new(quoter, 1)
    }

    fn level(price: &str) -> Option<Level> {
        Some(Level {
            px: px(price),
            qty: qty("1000"),
            orders: 1,
        })
    }

    fn market(instrument: InstrumentId, ms: i64, kind: MarketEventKind) -> Event {
        Event::Market(MarketEvent {
            instrument,
            ts_exch: Timestamp::from_millis(ms),
            ts_local: Timestamp::from_millis(ms),
            kind,
        })
    }

    fn bbo(ms: i64, bid: &str, ask: &str) -> Event {
        market(
            MON,
            ms,
            MarketEventKind::Bbo(Bbo {
                bid: level(bid),
                ask: level(ask),
            }),
        )
    }

    fn accept(host: &mut StrategyHost<FixedSpreadQuoter>, cloid: ClientOrderId, oid: u64) {
        host.on_event(&Event::Order(OrderEvent {
            instrument: MON,
            cloid,
            ts_exch: Timestamp::from_millis(1),
            ts_local: Timestamp::from_millis(1),
            kind: OrderEventKind::Accepted { oid: OrderId(oid) },
        }));
    }

    /// Feeds the first BBO and accepts both quotes. Returns (bid, ask) ids.
    fn quoting_host() -> (
        StrategyHost<FixedSpreadQuoter>,
        ClientOrderId,
        ClientOrderId,
    ) {
        let mut host = host();
        let placed: Vec<_> = host
            .on_event(&bbo(0, "0.0250", "0.0260"))
            .order_actions()
            .iter()
            .map(|action| match action {
                OrderAction::Place { cloid, .. } => *cloid,
                other => panic!("expected a place, got {other:?}"),
            })
            .collect();
        accept(&mut host, placed[0], 1);
        accept(&mut host, placed[1], 2);
        (host, placed[0], placed[1])
    }

    #[test]
    fn first_bbo_places_a_post_only_quote_on_each_side() {
        let mut host = host();
        let actions = host
            .on_event(&bbo(0, "0.0250", "0.0260"))
            .order_actions()
            .to_vec();

        let [
            OrderAction::Place { order: bid, .. },
            OrderAction::Place { order: ask, .. },
        ] = actions[..]
        else {
            panic!("expected two places, got {actions:?}");
        };
        assert_eq!(
            (bid.side, bid.px, bid.qty),
            (Side::Buy, px("0.0253"), qty("500"))
        );
        assert_eq!(
            (ask.side, ask.px, ask.qty),
            (Side::Sell, px("0.0257"), qty("500"))
        );
        assert_eq!((bid.tif, ask.tif), (TimeInForce::Alo, TimeInForce::Alo));
        assert!(!bid.reduce_only && !ask.reduce_only);
    }

    #[test]
    fn quotes_with_a_request_in_flight_are_left_alone() {
        let mut host = host();
        host.on_event(&bbo(0, "0.0250", "0.0260"));
        assert!(host.on_event(&bbo(1, "0.0300", "0.0310")).is_empty());
    }

    #[test]
    fn resting_quotes_follow_the_mid_price() {
        let (mut host, bid, ask) = quoting_host();
        let actions = host
            .on_event(&bbo(2, "0.0260", "0.0270"))
            .order_actions()
            .to_vec();

        assert_eq!(
            actions,
            [
                OrderAction::Modify {
                    instrument: MON,
                    cloid: bid,
                    px: px("0.0263"),
                    qty: qty("500")
                },
                OrderAction::Modify {
                    instrument: MON,
                    cloid: ask,
                    px: px("0.0267"),
                    qty: qty("500")
                },
            ]
        );
    }

    #[test]
    fn unchanged_mid_price_needs_no_action() {
        let (mut host, _, _) = quoting_host();
        assert!(host.on_event(&bbo(2, "0.0251", "0.0259")).is_empty());
    }

    #[test]
    fn feed_gap_cancels_everything_on_the_instrument() {
        let (mut host, _, _) = quoting_host();
        let actions = host.on_event(&market(MON, 2, MarketEventKind::FeedGap));
        assert_eq!(
            actions.order_actions(),
            [OrderAction::CancelAll {
                instrument: Some(MON)
            }]
        );
    }

    #[test]
    fn other_instruments_and_one_sided_books_are_ignored() {
        let mut host = host();
        let other = market(
            OTHER,
            0,
            MarketEventKind::Bbo(Bbo {
                bid: level("0.0250"),
                ask: level("0.0260"),
            }),
        );
        assert!(host.on_event(&other).is_empty());

        let one_sided = market(
            MON,
            1,
            MarketEventKind::Bbo(Bbo {
                bid: level("0.0250"),
                ask: None,
            }),
        );
        assert!(host.on_event(&one_sided).is_empty());
    }

    #[test]
    #[should_panic(expected = "half_spread must be positive")]
    fn rejects_a_non_positive_half_spread() {
        FixedSpreadQuoter::new(FixedSpreadConfig {
            instrument: MON,
            half_spread: Px::ZERO,
            qty: qty("1"),
        });
    }
}
