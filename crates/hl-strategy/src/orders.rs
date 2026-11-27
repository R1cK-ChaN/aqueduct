//! The table of live orders a strategy can read.

use std::collections::BTreeMap;

use hl_types::{
    ClientOrderId, InstrumentId, Order, OrderAction, OrderEvent, OrderEventKind, OrderState, Qty,
};

/// The strategy's live orders, as known locally.
///
/// The host keeps this table current: an order enters when the strategy
/// places it, changes as reports arrive, and leaves when it is filled,
/// canceled or rejected. Iteration is in cloid order, which is the order in
/// which the orders were placed.
#[derive(Debug, Default)]
pub struct OpenOrders {
    by_cloid: BTreeMap<ClientOrderId, Order>,
}

fn resting_state(order: &Order) -> OrderState {
    if order.filled_qty.is_positive() {
        OrderState::PartiallyFilled
    } else {
        OrderState::Open
    }
}

impl OpenOrders {
    /// Looks up one live order.
    pub fn get(&self, cloid: ClientOrderId) -> Option<&Order> {
        self.by_cloid.get(&cloid)
    }

    /// All live orders, oldest first.
    pub fn iter(&self) -> impl Iterator<Item = &Order> {
        self.by_cloid.values()
    }

    /// Live orders on one instrument, oldest first.
    pub fn for_instrument(&self, instrument: InstrumentId) -> impl Iterator<Item = &Order> {
        self.iter()
            .filter(move |order| order.instrument == instrument)
    }

    /// Number of live orders.
    pub fn len(&self) -> usize {
        self.by_cloid.len()
    }

    /// True when there are no live orders.
    pub fn is_empty(&self) -> bool {
        self.by_cloid.is_empty()
    }

    /// Records that the strategy just requested `action`.
    pub(crate) fn apply_action(&mut self, action: &OrderAction) {
        match *action {
            OrderAction::Place { cloid, order } => {
                self.by_cloid.entry(cloid).or_insert(Order {
                    cloid,
                    oid: None,
                    instrument: order.instrument,
                    side: order.side,
                    px: order.px,
                    open_qty: order.qty,
                    filled_qty: Qty::ZERO,
                    tif: order.tif,
                    reduce_only: order.reduce_only,
                    state: OrderState::PendingNew,
                });
            }
            OrderAction::Modify { cloid, .. } => {
                if let Some(order) = self.by_cloid.get_mut(&cloid) {
                    order.state = OrderState::PendingModify;
                }
            }
            OrderAction::Cancel { cloid, .. } => {
                if let Some(order) = self.by_cloid.get_mut(&cloid) {
                    order.state = OrderState::PendingCancel;
                }
            }
            OrderAction::CancelAll { instrument } => {
                for order in self.by_cloid.values_mut() {
                    if instrument.is_none_or(|wanted| wanted == order.instrument) {
                        order.state = OrderState::PendingCancel;
                    }
                }
            }
        }
    }

    /// Applies a report from the gateway. Reports about orders that are no
    /// longer live are ignored: a cancel can lose the race against a fill.
    pub(crate) fn apply_event(&mut self, event: &OrderEvent) {
        let Some(order) = self.by_cloid.get_mut(&event.cloid) else {
            return;
        };
        let ended = match event.kind {
            OrderEventKind::Accepted { oid } => {
                order.oid = Some(oid);
                if order.state == OrderState::PendingNew {
                    order.state = OrderState::Open;
                }
                false
            }
            OrderEventKind::Rejected { .. } | OrderEventKind::Canceled { .. } => true,
            OrderEventKind::Filled(fill) => {
                order.oid = Some(fill.oid);
                order.open_qty -= fill.qty;
                order.filled_qty += fill.qty;
                if order.state == OrderState::Open {
                    order.state = OrderState::PartiallyFilled;
                }
                !order.open_qty.is_positive()
            }
            OrderEventKind::Modified { px, qty } => {
                order.px = px;
                order.open_qty = qty;
                if order.state == OrderState::PendingModify {
                    order.state = resting_state(order);
                }
                false
            }
            OrderEventKind::ModifyRejected { .. } => {
                if order.state == OrderState::PendingModify {
                    order.state = resting_state(order);
                }
                false
            }
            OrderEventKind::CancelRejected { .. } => {
                if order.state == OrderState::PendingCancel {
                    order.state = resting_state(order);
                }
                false
            }
        };
        if ended {
            self.by_cloid.remove(&event.cloid);
        }
    }
}

#[cfg(test)]
mod tests {
    use hl_types::{
        CancelReason, Fill, Liquidity, OrderId, OrderSpec, Px, RejectReason, Side, TimeInForce,
        Timestamp, TradeId, Usd,
    };

    use super::*;

    const BTC: InstrumentId = InstrumentId::new(1);
    const ETH: InstrumentId = InstrumentId::new(2);

    fn px(text: &str) -> Px {
        text.parse().unwrap()
    }

    fn qty(text: &str) -> Qty {
        text.parse().unwrap()
    }

    fn cloid(seq: u64) -> ClientOrderId {
        ClientOrderId::from_parts(1, BTC, Timestamp::EPOCH, seq)
    }

    fn place(orders: &mut OpenOrders, seq: u64, instrument: InstrumentId) -> ClientOrderId {
        let cloid = cloid(seq);
        orders.apply_action(&OrderAction::Place {
            cloid,
            order: OrderSpec {
                instrument,
                side: Side::Buy,
                px: px("100"),
                qty: qty("2"),
                tif: TimeInForce::Alo,
                reduce_only: false,
            },
        });
        cloid
    }

    fn report(orders: &mut OpenOrders, cloid: ClientOrderId, kind: OrderEventKind) {
        orders.apply_event(&OrderEvent {
            instrument: BTC,
            cloid,
            ts_exch: Timestamp::EPOCH,
            ts_local: Timestamp::EPOCH,
            kind,
        });
    }

    fn fill(size: &str) -> OrderEventKind {
        OrderEventKind::Filled(Fill {
            tid: TradeId(7),
            oid: OrderId(11),
            side: Side::Buy,
            px: px("100"),
            qty: qty(size),
            liquidity: Liquidity::Maker,
            fee: Usd::ZERO,
        })
    }

    fn state(orders: &OpenOrders, cloid: ClientOrderId) -> OrderState {
        orders.get(cloid).unwrap().state
    }

    #[test]
    fn placed_order_is_pending_until_accepted() {
        let mut orders = OpenOrders::default();
        let id = place(&mut orders, 0, BTC);
        assert_eq!(state(&orders, id), OrderState::PendingNew);
        assert_eq!(orders.get(id).unwrap().oid, None);

        report(
            &mut orders,
            id,
            OrderEventKind::Accepted { oid: OrderId(11) },
        );
        assert_eq!(state(&orders, id), OrderState::Open);
        assert_eq!(orders.get(id).unwrap().oid, Some(OrderId(11)));
    }

    #[test]
    fn rejected_order_leaves_the_table() {
        let mut orders = OpenOrders::default();
        let id = place(&mut orders, 0, BTC);
        report(
            &mut orders,
            id,
            OrderEventKind::Rejected {
                reason: RejectReason::WouldCross,
            },
        );
        assert!(orders.is_empty());
    }

    #[test]
    fn fills_reduce_open_quantity_until_the_order_is_done() {
        let mut orders = OpenOrders::default();
        let id = place(&mut orders, 0, BTC);
        report(
            &mut orders,
            id,
            OrderEventKind::Accepted { oid: OrderId(11) },
        );

        report(&mut orders, id, fill("0.5"));
        let order = orders.get(id).unwrap();
        assert_eq!(order.state, OrderState::PartiallyFilled);
        assert_eq!(order.open_qty, qty("1.5"));
        assert_eq!(order.filled_qty, qty("0.5"));

        report(&mut orders, id, fill("1.5"));
        assert!(orders.get(id).is_none());
    }

    #[test]
    fn cancel_is_pending_until_confirmed() {
        let mut orders = OpenOrders::default();
        let id = place(&mut orders, 0, BTC);
        report(
            &mut orders,
            id,
            OrderEventKind::Accepted { oid: OrderId(11) },
        );

        orders.apply_action(&OrderAction::Cancel {
            instrument: BTC,
            cloid: id,
        });
        assert_eq!(state(&orders, id), OrderState::PendingCancel);

        report(
            &mut orders,
            id,
            OrderEventKind::Canceled {
                reason: CancelReason::Requested,
            },
        );
        assert!(orders.is_empty());
    }

    #[test]
    fn rejected_cancel_restores_the_resting_state() {
        let mut orders = OpenOrders::default();
        let id = place(&mut orders, 0, BTC);
        report(
            &mut orders,
            id,
            OrderEventKind::Accepted { oid: OrderId(11) },
        );
        report(&mut orders, id, fill("0.5"));

        orders.apply_action(&OrderAction::Cancel {
            instrument: BTC,
            cloid: id,
        });
        report(
            &mut orders,
            id,
            OrderEventKind::CancelRejected {
                reason: RejectReason::UnknownOrder,
            },
        );
        assert_eq!(state(&orders, id), OrderState::PartiallyFilled);
    }

    #[test]
    fn cancel_sent_before_the_accept_stays_pending() {
        let mut orders = OpenOrders::default();
        let id = place(&mut orders, 0, BTC);
        orders.apply_action(&OrderAction::Cancel {
            instrument: BTC,
            cloid: id,
        });

        report(
            &mut orders,
            id,
            OrderEventKind::Accepted { oid: OrderId(11) },
        );
        assert_eq!(state(&orders, id), OrderState::PendingCancel);
        assert_eq!(orders.get(id).unwrap().oid, Some(OrderId(11)));
    }

    #[test]
    fn fill_while_cancel_is_pending_keeps_the_cancel_pending() {
        let mut orders = OpenOrders::default();
        let id = place(&mut orders, 0, BTC);
        report(
            &mut orders,
            id,
            OrderEventKind::Accepted { oid: OrderId(11) },
        );
        orders.apply_action(&OrderAction::Cancel {
            instrument: BTC,
            cloid: id,
        });

        report(&mut orders, id, fill("0.5"));
        assert_eq!(state(&orders, id), OrderState::PendingCancel);
        assert_eq!(orders.get(id).unwrap().open_qty, qty("1.5"));
    }

    #[test]
    fn modify_updates_price_and_open_quantity_when_confirmed() {
        let mut orders = OpenOrders::default();
        let id = place(&mut orders, 0, BTC);
        report(
            &mut orders,
            id,
            OrderEventKind::Accepted { oid: OrderId(11) },
        );

        orders.apply_action(&OrderAction::Modify {
            instrument: BTC,
            cloid: id,
            px: px("101"),
            qty: qty("3"),
        });
        let order = orders.get(id).unwrap();
        assert_eq!(order.state, OrderState::PendingModify);
        assert_eq!(
            order.px,
            px("100"),
            "price changes only when the venue confirms"
        );

        report(
            &mut orders,
            id,
            OrderEventKind::Modified {
                px: px("101"),
                qty: qty("3"),
            },
        );
        let order = orders.get(id).unwrap();
        assert_eq!(order.state, OrderState::Open);
        assert_eq!(order.px, px("101"));
        assert_eq!(order.open_qty, qty("3"));
    }

    #[test]
    fn rejected_modify_leaves_the_order_unchanged() {
        let mut orders = OpenOrders::default();
        let id = place(&mut orders, 0, BTC);
        report(
            &mut orders,
            id,
            OrderEventKind::Accepted { oid: OrderId(11) },
        );
        orders.apply_action(&OrderAction::Modify {
            instrument: BTC,
            cloid: id,
            px: px("101"),
            qty: qty("3"),
        });

        report(
            &mut orders,
            id,
            OrderEventKind::ModifyRejected {
                reason: RejectReason::WouldCross,
            },
        );
        let order = orders.get(id).unwrap();
        assert_eq!(order.state, OrderState::Open);
        assert_eq!(order.px, px("100"));
        assert_eq!(order.open_qty, qty("2"));
    }

    #[test]
    fn cancel_all_can_be_limited_to_one_instrument() {
        let mut orders = OpenOrders::default();
        let btc = place(&mut orders, 0, BTC);
        let eth = place(&mut orders, 1, ETH);

        orders.apply_action(&OrderAction::CancelAll {
            instrument: Some(ETH),
        });
        assert_eq!(state(&orders, btc), OrderState::PendingNew);
        assert_eq!(state(&orders, eth), OrderState::PendingCancel);

        orders.apply_action(&OrderAction::CancelAll { instrument: None });
        assert_eq!(state(&orders, btc), OrderState::PendingCancel);
    }

    #[test]
    fn reports_about_unknown_orders_are_ignored() {
        let mut orders = OpenOrders::default();
        report(&mut orders, cloid(9), fill("1"));
        report(
            &mut orders,
            cloid(9),
            OrderEventKind::CancelRejected {
                reason: RejectReason::UnknownOrder,
            },
        );
        assert!(orders.is_empty());
    }

    #[test]
    fn iterates_oldest_first_and_filters_by_instrument() {
        let mut orders = OpenOrders::default();
        let second = place(&mut orders, 1, ETH);
        let first = place(&mut orders, 0, BTC);
        let third = place(&mut orders, 2, BTC);

        let all: Vec<_> = orders.iter().map(|order| order.cloid).collect();
        assert_eq!(all, [first, second, third]);
        let btc: Vec<_> = orders
            .for_instrument(BTC)
            .map(|order| order.cloid)
            .collect();
        assert_eq!(btc, [first, third]);
        assert_eq!(orders.len(), 3);
    }
}
