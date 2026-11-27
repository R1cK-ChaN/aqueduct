//! A simulated venue behind the [`OrderGateway`] interface.
//!
//! The simulator uses the receipt-order timing rule in RFC 0008 D2, with
//! post-only orders, strict trade-through matching and constant latencies.
//! Queue position, taker orders, request limits and funding remain outside
//! this model. RFC 0008 D1 bases its timing assumption on public-feed
//! observations (HL-124, HL-125, HL-128 and HL-130 in `docs/venues/hyperliquid.md`).
//!
//! # Rules in this version
//!
//! - Only [`TimeInForce::Alo`] orders without `reduce_only` are supported.
//!   Other order types are rejected as [`RejectReason::Unsupported`].
//! - An order must have a positive price and size and be worth at least
//!   [`SimConfig::min_order_notional`] (HL-27 in `docs/venues/hyperliquid.md`).
//! - An action has modeled effect time `A = submitted_at + order_latency`.
//!   The first received BBO or trade with `ts_exch > A` proves it. Before
//!   applying that event, apply all proved actions in effect-time order,
//!   then submission and batch order. Stamp their outcomes with `A`.
//! - When a rule book is supplied, placements and modifications require
//!   coverage at `A` before any ordinary rejection. After the notional
//!   check, an off-grid price or over-precise size gives
//!   [`RejectReason::InvalidOrder`] (HL-23, HL-24, HL-25 and HL-26 in
//!   `docs/venues/hyperliquid.md`). Missing coverage stops the run.
//!   Cancels need no lookup.
//! - Post-only validation uses the newest already received best prices by
//!   exchange time, with receipt order breaking ties. The proving event has
//!   not updated the prices yet. A crossing order is rejected as
//!   [`RejectReason::WouldCross`]. An unknown opposite quote passes this
//!   check (HL-15 in `docs/venues/hyperliquid.md`).
//! - A trade with exchange time `E` can fill a resting version only when
//!   its placement or last successful modify has `A < E`. An opposite-side
//!   trade must print strictly through its price. Spend the trade size once,
//!   best price then oldest order first, at our resting prices. This keeps
//!   the existing matching approximation and stamps fills with `E`
//!   (HL-6 in `docs/venues/hyperliquid.md`).
//! - Successful cancels and modifies save each superseded version's remaining
//!   quantity over `A < E <= S`, where `S` is the superseding effect time.
//!   Later prints spend a separate copy of their size across eligible saved
//!   versions in the same matching order. [`LatePrintMisses`] counts those
//!   allocations without producing a fill or report. Saved quantities remain
//!   until consumed, including across gaps and file boundaries.
//! - A report determined by input received at `R` is due at
//!   `max(ts_exch + report_latency, R, previous_delivery_for_same_cloid)`.
//!   Reports are delivered by due time, then creation order.
//! - BBOs and L2 snapshots update best prices only when their exchange time
//!   is at least the stored quote's. Snapshots supply no action evidence.
//!   Feed gaps and L2 diffs change no simulator state.
//!
//! # Driving the simulator
//!
//! Feed [`SimGateway::on_market_event`] in receipt order, with nondecreasing
//! `ts_local`. Qualifying evidence applies to every pending instrument.
//! Exchange times may go backwards. Process all inputs at a local timestamp
//! before polling reports at that timestamp, then call the strategy's timers
//! as specified by RFC 0008 D3. A zero-delay action produced by a market
//! callback needs a later qualifying input to prove it.
//!
//! [`SimGateway::next_wakeup`] advertises only determined reports.
//! [`OrderGateway::poll_events`] releases due reports without proving actions,
//! so pending actions can remain indefinitely during silence. The driver
//! stops at its inclusive cutoff and reads the remaining work through
//! [`SimGateway::pending_actions`] and [`SimGateway::undelivered_reports`].

mod error;

pub use error::SimError;

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use hl_gateway::OrderGateway;
use hl_rules::{RuleBook, RuleVersion};
use hl_types::{
    CancelReason, ClientOrderId, FeeRate, Fill, InstrumentId, Liquidity, MarketEvent,
    MarketEventKind, OrderAction, OrderEvent, OrderEventKind, OrderId, OrderSpec, Px, Qty,
    RejectReason, Side, TimeInForce, Timestamp, Trade, TradeId, Usd,
};

/// Settings for [`SimGateway`]. Latencies and fees are chosen by the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimConfig {
    /// Modeled delay from submission to effect time. Later data must prove it.
    pub order_latency: Duration,
    /// Minimum delay from an effect to its report, also floored by receipt time.
    pub report_latency: Duration,
    /// Fee charged on the notional of every maker fill.
    pub maker_fee: FeeRate,
    /// Smallest accepted order value (HL-27 in `docs/venues/hyperliquid.md`).
    pub min_order_notional: Usd,
}

/// Session identifier for the receipt-ordered, strict trade-through model.
pub const MODEL_NAME: &str = "strict_trade_through";

impl SimConfig {
    /// Exact session parameters in output order, including the fee in basis points.
    pub fn parameters(&self) -> Vec<(String, String)> {
        // One raw fee-rate unit is 0.0001 bps.
        let raw = self.maker_fee.as_e8();
        let sign = if raw < 0 { "-" } else { "" };
        let magnitude = raw.unsigned_abs();
        let bps = format!("{sign}{}.{:04}", magnitude / 10_000, magnitude % 10_000);
        vec![
            (
                "order_latency_ns".into(),
                self.order_latency.as_nanos().to_string(),
            ),
            (
                "report_latency_ns".into(),
                self.report_latency.as_nanos().to_string(),
            ),
            (
                "maker_fee_bps".into(),
                bps.trim_end_matches('0').trim_end_matches('.').to_owned(),
            ),
            (
                "min_order_notional_usd".into(),
                self.min_order_notional.to_string(),
            ),
        ]
    }

    /// Rebuilds this model's configuration, rejecting incomplete or unrepresentable settings.
    pub fn from_parameters(pairs: &[(String, String)]) -> Result<Self, String> {
        if pairs.len() != 4 {
            return Err("expected exactly order_latency_ns, report_latency_ns, maker_fee_bps and min_order_notional_usd".into());
        }
        let value = |key: &str| {
            pairs
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.as_str())
                .ok_or_else(|| format!("missing {key}"))
        };
        let latency = |key: &str| -> Result<Duration, String> {
            let text = value(key)?;
            if !text.bytes().all(|b| b.is_ascii_digit()) {
                return Err(format!("{key}: expected unsigned nanoseconds"));
            }
            text.parse::<u64>()
                .map(Duration::from_nanos)
                .map_err(|e| format!("{key}: {e}"))
        };
        let order_latency = latency("order_latency_ns")?;
        let report_latency = latency("report_latency_ns")?;
        if (order_latency + report_latency).as_nanos() > i64::MAX as u128 {
            return Err("latencies exceed the timestamp range".into());
        }
        let min_order_notional = value("min_order_notional_usd")?
            .parse::<Usd>()
            .map_err(|e| format!("min_order_notional_usd: {e}"))?;
        if min_order_notional.is_negative() {
            return Err("min_order_notional_usd must be nonnegative".into());
        }

        // Shift the decimal point four places before parsing in FeeRate's native units.
        let bps = value("maker_fee_bps")?;
        let (sign, unsigned) = bps.strip_prefix('-').map_or(("", bps), |n| ("-", n));
        let (whole, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
        if whole.is_empty() || !whole.bytes().all(|b| b.is_ascii_digit()) || unsigned.ends_with('.')
        {
            return Err("maker_fee_bps: expected a decimal number of basis points".into());
        }
        let padded = format!("{whole:0>5}");
        let (units, shifted) = padded.split_at(padded.len() - 4);
        let maker_fee = format!("{sign}{units}.{shifted}{fraction}")
            .parse::<FeeRate>()
            .map_err(|e| format!("maker_fee_bps: {e}"))?;
        Ok(Self {
            order_latency,
            report_latency,
            maker_fee,
            min_order_notional,
        })
    }
}

/// Counterfactual allocations to superseded versions under RFC 0008 D2.
/// These quantities produce no fill, report or ledger entry.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LatePrintMisses {
    /// Number of trade inputs with a positive missed allocation.
    pub trades: usize,
    /// Total missed quantity in the selected coin's fixed-point units.
    pub qty: Qty,
}

#[derive(Debug, Clone, Copy)]
struct RestingOrder {
    cloid: ClientOrderId,
    /// Increasing acceptance identity also preserves the original priority.
    oid: OrderId,
    instrument: InstrumentId,
    side: Side,
    px: Px,
    open_qty: Qty,
    /// Placement or last successful modify's modeled effect time.
    effective_at: Timestamp,
}

impl RestingOrder {
    fn crossed_by(self, instrument: InstrumentId, trade: &Trade) -> bool {
        self.instrument == instrument
            && match self.side {
                Side::Buy => trade.aggressor == Side::Sell && trade.px < self.px,
                Side::Sell => trade.aggressor == Side::Buy && trade.px > self.px,
            }
    }

    fn matching_priority(self) -> (Px, OrderId) {
        let price = match self.side {
            Side::Buy => -self.px,
            Side::Sell => self.px,
        };
        (price, self.oid)
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct BestPrices {
    bid: Option<Px>,
    ask: Option<Px>,
    ts_exch: Timestamp,
}

/// The simulated venue, driven by received evidence instead of wall time.
#[derive(Debug)]
pub struct SimGateway {
    config: SimConfig,
    rules: Option<RuleBook>,
    /// Unproved actions, keyed by effect time and submission/batch sequence.
    inbound: BTreeMap<(Timestamp, u64), OrderAction>,
    /// Undelivered reports, keyed by delivery time and creation sequence.
    outbound: BTreeMap<(Timestamp, u64), OrderEvent>,
    last_report_delivery: BTreeMap<ClientOrderId, Timestamp>,
    /// Resting orders retain their original acceptance priority after modify.
    resting: Vec<RestingOrder>,
    /// Index by superseding time so a trade examines only intervals with S >= E.
    superseded: BTreeMap<(Timestamp, OrderId), RestingOrder>,
    late_print_misses: LatePrintMisses,
    seen_cloids: BTreeSet<ClientOrderId>,
    best: BTreeMap<InstrumentId, BestPrices>,
    next_action_sequence: u64,
    next_report_sequence: u64,
    next_oid: u64,
    next_tid: u64,
}

impl SimGateway {
    /// Creates a venue with no orders, market data or dated rules.
    pub fn new(config: SimConfig) -> Self {
        Self {
            config,
            rules: None,
            inbound: BTreeMap::new(),
            outbound: BTreeMap::new(),
            last_report_delivery: BTreeMap::new(),
            resting: Vec::new(),
            superseded: BTreeMap::new(),
            late_print_misses: LatePrintMisses::default(),
            seen_cloids: BTreeSet::new(),
            best: BTreeMap::new(),
            next_action_sequence: 0,
            next_report_sequence: 0,
            next_oid: 1,
            next_tid: 1,
        }
    }

    /// Creates an empty venue that checks placements and modifications against `rules`.
    pub fn with_rules(config: SimConfig, rules: RuleBook) -> Self {
        Self {
            rules: Some(rules),
            ..Self::new(config)
        }
    }

    /// Processes one input in receipt order. A BBO or trade first proves
    /// pending actions with strictly earlier effect times, then is applied.
    /// `ts_local` must never decrease. `ts_exch` may decrease.
    ///
    /// On a rule lookup error, earlier actions and their reports are retained.
    /// The failing action has been removed without a report. The remaining
    /// actions stay queued and the proving market event has not been applied.
    pub fn on_market_event(&mut self, event: &MarketEvent) -> Result<(), SimError> {
        if matches!(
            &event.kind,
            MarketEventKind::Bbo(_) | MarketEventKind::Trade(_)
        ) {
            self.prove_actions(event.ts_exch, event.ts_local)?;
        }
        match &event.kind {
            MarketEventKind::L2Snapshot(snapshot) => {
                self.update_best(
                    event.instrument,
                    BestPrices {
                        bid: snapshot.bids.first().map(|level| level.px),
                        ask: snapshot.asks.first().map(|level| level.px),
                        ts_exch: event.ts_exch,
                    },
                );
            }
            MarketEventKind::Bbo(bbo) => {
                self.update_best(
                    event.instrument,
                    BestPrices {
                        bid: bbo.bid.map(|level| level.px),
                        ask: bbo.ask.map(|level| level.px),
                        ts_exch: event.ts_exch,
                    },
                );
            }
            MarketEventKind::Trade(trade) => {
                self.match_trade(event.instrument, trade, event.ts_exch, event.ts_local);
                self.count_late_print_misses(event.instrument, trade, event.ts_exch);
            }
            MarketEventKind::L2Diff(_) | MarketEventKind::FeedGap => {}
        }
        Ok(())
    }

    /// Earliest determined report deadline. Pending actions are not wake-ups.
    pub fn next_wakeup(&self) -> Option<Timestamp> {
        self.outbound.first_key_value().map(|((due, _), _)| *due)
    }

    /// Submitted actions still waiting for a qualifying input to prove them.
    pub fn pending_actions(&self) -> usize {
        self.inbound.len()
    }

    /// Determined reports not yet returned by [`OrderGateway::poll_events`].
    pub fn undelivered_reports(&self) -> usize {
        self.outbound.len()
    }

    /// Returns the cumulative counter for allocations to superseded versions.
    pub fn late_print_misses(&self) -> LatePrintMisses {
        self.late_print_misses
    }

    fn update_best(&mut self, instrument: InstrumentId, prices: BestPrices) {
        if self
            .best
            .get(&instrument)
            .is_none_or(|stored| prices.ts_exch >= stored.ts_exch)
        {
            self.best.insert(instrument, prices);
        }
    }

    fn prove_actions(
        &mut self,
        exchange_time: Timestamp,
        receipt_time: Timestamp,
    ) -> Result<(), SimError> {
        while let Some((&(effective_at, _), _)) = self.inbound.first_key_value()
            && effective_at < exchange_time
        {
            let (_, action) = self.inbound.pop_first().expect("pending action exists");
            self.apply(action, effective_at, receipt_time)?;
        }
        Ok(())
    }

    fn apply(
        &mut self,
        action: OrderAction,
        effective_at: Timestamp,
        receipt_time: Timestamp,
    ) -> Result<(), SimError> {
        match action {
            OrderAction::Place { cloid, order } => {
                let version = self.rules_at(order.instrument, effective_at)?;
                let kind = match self.place(cloid, &order, effective_at, version) {
                    Ok(oid) => OrderEventKind::Accepted { oid },
                    Err(reason) => OrderEventKind::Rejected { reason },
                };
                self.report(order.instrument, cloid, kind, effective_at, receipt_time);
            }
            OrderAction::Modify {
                instrument,
                cloid,
                px,
                qty,
            } => {
                let index = self.position_of(cloid);
                let rule_instrument = index.map_or(instrument, |i| self.resting[i].instrument);
                let version = self.rules_at(rule_instrument, effective_at)?;
                let kind = match self.modify(index, px, qty, effective_at, version) {
                    Ok(()) => OrderEventKind::Modified { px, qty },
                    Err(reason) => OrderEventKind::ModifyRejected { reason },
                };
                self.report(instrument, cloid, kind, effective_at, receipt_time);
            }
            OrderAction::Cancel { instrument, cloid } => {
                let kind = match self.position_of(cloid) {
                    Some(index) => {
                        let order = self.resting.remove(index);
                        self.save_superseded(order, effective_at);
                        OrderEventKind::Canceled {
                            reason: CancelReason::Requested,
                        }
                    }
                    None => OrderEventKind::CancelRejected {
                        reason: RejectReason::UnknownOrder,
                    },
                };
                self.report(instrument, cloid, kind, effective_at, receipt_time);
            }
            OrderAction::CancelAll { instrument } => {
                let (canceled, kept): (Vec<_>, Vec<_>) =
                    self.resting.iter().copied().partition(|order| {
                        instrument.is_none_or(|wanted| wanted == order.instrument)
                    });
                self.resting = kept;
                for order in canceled {
                    self.save_superseded(order, effective_at);
                    let kind = OrderEventKind::Canceled {
                        reason: CancelReason::Requested,
                    };
                    self.report(
                        order.instrument,
                        order.cloid,
                        kind,
                        effective_at,
                        receipt_time,
                    );
                }
            }
        }
        Ok(())
    }

    fn rules_at(
        &self,
        instrument: InstrumentId,
        effective_at: Timestamp,
    ) -> Result<Option<RuleVersion>, SimError> {
        let Some(rules) = &self.rules else {
            return Ok(None);
        };
        let version = rules
            .at(instrument, effective_at)
            .map_err(|source| SimError {
                instrument,
                effective_at,
                source,
            })?;
        Ok(Some(*version))
    }

    fn place(
        &mut self,
        cloid: ClientOrderId,
        order: &OrderSpec,
        effective_at: Timestamp,
        version: Option<RuleVersion>,
    ) -> Result<OrderId, RejectReason> {
        if !self.seen_cloids.insert(cloid) {
            return Err(RejectReason::DuplicateClientOrderId);
        }
        if order.tif != TimeInForce::Alo || order.reduce_only {
            return Err(RejectReason::Unsupported);
        }
        self.check_post_only(order.instrument, order.side, order.px, order.qty, version)?;
        let oid = OrderId(self.next_oid);
        self.next_oid += 1;
        self.resting.push(RestingOrder {
            cloid,
            oid,
            instrument: order.instrument,
            side: order.side,
            px: order.px,
            open_qty: order.qty,
            effective_at,
        });
        Ok(oid)
    }

    fn modify(
        &mut self,
        index: Option<usize>,
        px: Px,
        qty: Qty,
        effective_at: Timestamp,
        version: Option<RuleVersion>,
    ) -> Result<(), RejectReason> {
        let index = index.ok_or(RejectReason::UnknownOrder)?;
        let old = self.resting[index];
        self.check_post_only(old.instrument, old.side, px, qty, version)?;
        self.save_superseded(old, effective_at);
        let order = &mut self.resting[index];
        order.px = px;
        order.open_qty = qty;
        order.effective_at = effective_at;
        Ok(())
    }

    fn save_superseded(&mut self, order: RestingOrder, superseded_at: Timestamp) {
        // Repeated modifications at the same effect time have an empty
        // A < E <= S interval. Skipping them preserves the prior nonempty
        // version's unique (S, oid) key and its remaining quantity.
        if order.effective_at < superseded_at {
            self.superseded.insert((superseded_at, order.oid), order);
        }
    }

    /// The checks a post-only order must pass to rest at `px` for `qty`.
    fn check_post_only(
        &self,
        instrument: InstrumentId,
        side: Side,
        px: Px,
        qty: Qty,
        version: Option<RuleVersion>,
    ) -> Result<(), RejectReason> {
        if !px.is_positive() || !qty.is_positive() {
            return Err(RejectReason::InvalidOrder);
        }
        let notional = px.notional(qty).ok_or(RejectReason::InvalidOrder)?;
        if notional < self.config.min_order_notional {
            return Err(RejectReason::BelowMinNotional);
        }
        if let Some(version) = version {
            version
                .check_price(px)
                .map_err(|_| RejectReason::InvalidOrder)?;
            version
                .check_size(qty)
                .map_err(|_| RejectReason::InvalidOrder)?;
        }
        let best = self.best.get(&instrument).copied().unwrap_or_default();
        let would_cross = match side {
            Side::Buy => best.ask.is_some_and(|ask| px >= ask),
            Side::Sell => best.bid.is_some_and(|bid| px <= bid),
        };
        if would_cross {
            Err(RejectReason::WouldCross)
        } else {
            Ok(())
        }
    }

    fn match_trade(
        &mut self,
        instrument: InstrumentId,
        trade: &Trade,
        exchange_time: Timestamp,
        receipt_time: Timestamp,
    ) {
        let mut hit: Vec<RestingOrder> = self
            .resting
            .iter()
            .filter(|order| {
                order.effective_at < exchange_time && order.crossed_by(instrument, trade)
            })
            .copied()
            .collect();
        hit.sort_by_key(|order| order.matching_priority());

        let mut remaining = trade.qty;
        for order in hit {
            if !remaining.is_positive() {
                break;
            }
            let qty = order.open_qty.min(remaining);
            remaining -= qty;
            let notional = order
                .px
                .notional(qty)
                .expect("order value was checked when the order rested");
            let fill = Fill {
                tid: TradeId(self.next_tid),
                oid: order.oid,
                side: order.side,
                px: order.px,
                qty,
                liquidity: Liquidity::Maker,
                fee: self.config.maker_fee.fee_on(notional),
            };
            self.next_tid += 1;

            if let Some(index) = self.position_of(order.cloid) {
                if qty == order.open_qty {
                    self.resting.remove(index);
                } else {
                    self.resting[index].open_qty -= qty;
                }
            }
            self.report(
                instrument,
                order.cloid,
                OrderEventKind::Filled(fill),
                exchange_time,
                receipt_time,
            );
        }
    }

    fn count_late_print_misses(
        &mut self,
        instrument: InstrumentId,
        trade: &Trade,
        exchange_time: Timestamp,
    ) {
        let mut hit: Vec<_> = self
            .superseded
            .range((exchange_time, OrderId(0))..)
            .filter(|(_, order)| {
                order.effective_at < exchange_time && order.crossed_by(instrument, trade)
            })
            .map(|(&key, &order)| (key, order))
            .collect();
        hit.sort_by_key(|(_, order)| order.matching_priority());

        // This separate budget is the counterfactual allocation required by
        // RFC 0008 D2. It never reduces the trade available for actual fills.
        let mut remaining = trade.qty;
        let mut allocated = Qty::ZERO;
        for (key, order) in hit {
            if !remaining.is_positive() {
                break;
            }
            let qty = order.open_qty.min(remaining);
            remaining -= qty;
            allocated += qty;
            if qty == order.open_qty {
                self.superseded.remove(&key);
            } else {
                self.superseded
                    .get_mut(&key)
                    .expect("saved version exists")
                    .open_qty -= qty;
            }
        }
        if allocated.is_positive() {
            self.late_print_misses.trades += 1;
            self.late_print_misses.qty += allocated;
        }
    }

    fn position_of(&self, cloid: ClientOrderId) -> Option<usize> {
        self.resting.iter().position(|order| order.cloid == cloid)
    }

    fn report(
        &mut self,
        instrument: InstrumentId,
        cloid: ClientOrderId,
        kind: OrderEventKind,
        exchange_time: Timestamp,
        receipt_time: Timestamp,
    ) {
        let due = (exchange_time + self.config.report_latency).max(receipt_time);
        let due = self
            .last_report_delivery
            .get(&cloid)
            .map_or(due, |&previous| due.max(previous));
        self.last_report_delivery.insert(cloid, due);
        self.outbound.insert(
            (due, self.next_report_sequence),
            OrderEvent {
                instrument,
                cloid,
                ts_exch: exchange_time,
                ts_local: due,
                kind,
            },
        );
        self.next_report_sequence += 1;
    }
}

impl OrderGateway for SimGateway {
    fn submit(&mut self, now: Timestamp, actions: &[OrderAction]) {
        let effective_at = now + self.config.order_latency;
        for &action in actions {
            self.inbound
                .insert((effective_at, self.next_action_sequence), action);
            self.next_action_sequence += 1;
        }
    }

    fn poll_events(&mut self, now: Timestamp, out: &mut Vec<OrderEvent>) {
        while let Some((&(due, _), _)) = self.outbound.first_key_value()
            && due <= now
        {
            let (_, event) = self.outbound.pop_first().expect("due report exists");
            out.push(event);
        }
    }
}

#[cfg(test)]
mod tests;
