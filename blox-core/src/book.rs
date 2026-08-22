use crate::{Change, Lots, NewOrder, OrderId, OrderKind, RejectReason, Side, Ticks};
use std::collections::{BTreeMap, HashMap, VecDeque};

#[derive(Copy, Clone, Debug)]
struct Resting {
    id: OrderId,
    qty: Lots,
}

#[derive(Copy, Clone, Debug)]
struct Location {
    side: Side,
    price: Ticks,
}

#[derive(Clone, Debug, Default)]
pub struct OrderBook {
    bids: BTreeMap<Ticks, VecDeque<Resting>>,
    asks: BTreeMap<Ticks, VecDeque<Resting>>,
    index: HashMap<OrderId, Location>,
}

impl OrderBook {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn order_count(&self) -> usize {
        self.index.len()
    }
    pub fn best_bid(&self) -> Option<(Ticks, Lots)> {
        self.bids
            .last_key_value()
            .map(|(p, q)| (*p, q.iter().map(|o| o.qty).sum()))
    }
    pub fn best_ask(&self) -> Option<(Ticks, Lots)> {
        self.asks
            .first_key_value()
            .map(|(p, q)| (*p, q.iter().map(|o| o.qty).sum()))
    }
    pub fn depth(&self, side: Side, max: usize) -> Vec<(Ticks, Lots)> {
        let collect = |p: &Ticks, q: &VecDeque<Resting>| (*p, q.iter().map(|o| o.qty).sum());
        match side {
            Side::Bid => self
                .bids
                .iter()
                .rev()
                .take(max)
                .map(|(p, q)| collect(p, q))
                .collect(),
            Side::Ask => self
                .asks
                .iter()
                .take(max)
                .map(|(p, q)| collect(p, q))
                .collect(),
        }
    }

    pub fn submit(&mut self, order: NewOrder) -> Vec<Change> {
        let mut out = Vec::new();
        self.submit_into(order, &mut out);
        out
    }
    pub fn submit_into(&mut self, order: NewOrder, out: &mut Vec<Change>) {
        if order.qty == 0 {
            return out.push(Change::Rejected {
                id: order.id,
                reason: RejectReason::ZeroQty,
            });
        }
        if order.qty < 0 {
            return out.push(Change::Rejected {
                id: order.id,
                reason: RejectReason::NegativeQty,
            });
        }
        if matches!(order.kind, OrderKind::Limit { price } if price <= 0) {
            return out.push(Change::Rejected {
                id: order.id,
                reason: RejectReason::NonPositivePrice,
            });
        }
        if self.index.contains_key(&order.id) {
            return out.push(Change::Rejected {
                id: order.id,
                reason: RejectReason::DuplicateId,
            });
        }
        if self.would_overflow_level(order) {
            return out.push(Change::Rejected {
                id: order.id,
                reason: RejectReason::BookQuantityOverflow,
            });
        }
        if matches!(order.kind, OrderKind::Market) && self.side_empty(order.side.opposite()) {
            return out.push(Change::Rejected {
                id: order.id,
                reason: RejectReason::NoLiquidity,
            });
        }
        out.push(Change::Accepted { id: order.id });
        let limit = match order.kind {
            OrderKind::Market => None,
            OrderKind::Limit { price } => Some(price),
        };
        let remaining = self.take(order.id, order.side, order.qty, limit, out);
        if remaining > 0 {
            match order.kind {
                OrderKind::Market => out.push(Change::Cancelled {
                    id: order.id,
                    remaining,
                }),
                OrderKind::Limit { price } => self.rest(order.id, order.side, price, remaining),
            }
        }
    }

    pub fn cancel(&mut self, id: OrderId) -> Vec<Change> {
        let mut out = Vec::new();
        self.cancel_into(id, &mut out);
        out
    }
    pub fn cancel_into(&mut self, id: OrderId, out: &mut Vec<Change>) {
        let Some(loc) = self.index.remove(&id) else {
            return out.push(Change::Rejected {
                id,
                reason: RejectReason::UnknownOrder,
            });
        };
        let levels = match loc.side {
            Side::Bid => &mut self.bids,
            Side::Ask => &mut self.asks,
        };
        let q = levels.get_mut(&loc.price).expect("indexed price exists");
        let pos = q
            .iter()
            .position(|o| o.id == id)
            .expect("indexed order exists");
        let remaining = q.remove(pos).unwrap().qty;
        if q.is_empty() {
            levels.remove(&loc.price);
        }
        out.push(Change::Cancelled { id, remaining });
    }

    fn side_empty(&self, side: Side) -> bool {
        match side {
            Side::Bid => self.bids.is_empty(),
            Side::Ask => self.asks.is_empty(),
        }
    }
    fn rest(&mut self, id: OrderId, side: Side, price: Ticks, qty: Lots) {
        let levels = match side {
            Side::Bid => &mut self.bids,
            Side::Ask => &mut self.asks,
        };
        levels
            .entry(price)
            .or_default()
            .push_back(Resting { id, qty });
        self.index.insert(id, Location { side, price });
    }
    fn take(
        &mut self,
        taker: OrderId,
        side: Side,
        mut remaining: Lots,
        limit: Option<Ticks>,
        out: &mut Vec<Change>,
    ) -> Lots {
        while remaining > 0 {
            let best = match side {
                Side::Bid => self.asks.first_key_value().map(|(p, _)| *p),
                Side::Ask => self.bids.last_key_value().map(|(p, _)| *p),
            };
            let Some(price) = best else { break };
            if let Some(l) = limit {
                let crosses = match side {
                    Side::Bid => l >= price,
                    Side::Ask => l <= price,
                };
                if !crosses {
                    break;
                }
            }
            let levels = match side {
                Side::Bid => &mut self.asks,
                Side::Ask => &mut self.bids,
            };
            let queue = levels.get_mut(&price).unwrap();
            while remaining > 0 && !queue.is_empty() {
                let maker = queue.front_mut().unwrap();
                let fill = remaining.min(maker.qty);
                remaining -= fill;
                maker.qty -= fill;
                out.push(Change::Trade {
                    price,
                    qty: fill,
                    maker: maker.id,
                    taker,
                    aggressor: side,
                });
                out.push(Change::Filled {
                    id: maker.id,
                    price,
                    qty: fill,
                    remaining: maker.qty,
                });
                out.push(Change::Filled {
                    id: taker,
                    price,
                    qty: fill,
                    remaining,
                });
                if maker.qty == 0 {
                    let id = maker.id;
                    queue.pop_front();
                    self.index.remove(&id);
                }
            }
            if queue.is_empty() {
                levels.remove(&price);
            }
        }
        remaining
    }

    pub fn check(&self) -> Result<(), String> {
        let mut count = 0;
        for (side, levels) in [(Side::Bid, &self.bids), (Side::Ask, &self.asks)] {
            for (price, q) in levels {
                if q.is_empty() {
                    return Err("empty level".into());
                }
                for o in q {
                    if o.qty <= 0 {
                        return Err("non-positive resting qty".into());
                    }
                    if self
                        .index
                        .get(&o.id)
                        .is_none_or(|l| l.side != side || l.price != *price)
                    {
                        return Err("index mismatch".into());
                    }
                    count += 1;
                }
            }
        }
        if count != self.index.len() {
            return Err("index size mismatch".into());
        }
        if let (Some((b, _)), Some((a, _))) = (self.best_bid(), self.best_ask()) {
            if b >= a {
                return Err("crossed book".into());
            }
        }
        Ok(())
    }

    fn would_overflow_level(&self, order: NewOrder) -> bool {
        let OrderKind::Limit { price: limit } = order.kind else {
            return false;
        };
        let opposite = match order.side {
            Side::Bid => &self.asks,
            Side::Ask => &self.bids,
        };
        let fillable: i128 = match order.side {
            Side::Bid => opposite
                .range(..=limit)
                .flat_map(|(_, queue)| queue)
                .map(|resting| i128::from(resting.qty))
                .sum(),
            Side::Ask => opposite
                .range(limit..)
                .flat_map(|(_, queue)| queue)
                .map(|resting| i128::from(resting.qty))
                .sum(),
        };
        let remainder = (i128::from(order.qty) - fillable).max(0);
        if remainder == 0 {
            return false;
        }
        let same_side = match order.side {
            Side::Bid => &self.bids,
            Side::Ask => &self.asks,
        };
        let at_price: i128 = same_side
            .get(&limit)
            .into_iter()
            .flatten()
            .map(|resting| i128::from(resting.qty))
            .sum();
        at_price + remainder > i128::from(i64::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn limit(id: u64, side: Side, price: i64, qty: i64) -> NewOrder {
        NewOrder {
            id,
            side,
            qty,
            kind: OrderKind::Limit { price },
        }
    }
    #[test]
    fn price_time_and_maker_price() {
        let mut b = OrderBook::new();
        b.submit(limit(1, Side::Ask, 100, 5));
        b.submit(limit(2, Side::Ask, 100, 5));
        let out = b.submit(limit(3, Side::Bid, 101, 7));
        let trades: Vec<_> = out
            .iter()
            .filter_map(|c| {
                if let Change::Trade {
                    maker, price, qty, ..
                } = c
                {
                    Some((*maker, *price, *qty))
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(trades, vec![(1, 100, 5), (2, 100, 2)]);
        assert!(b.check().is_ok());
    }
    #[test]
    fn market_never_rests() {
        let mut b = OrderBook::new();
        b.submit(limit(1, Side::Ask, 100, 2));
        let out = b.submit(NewOrder {
            id: 2,
            side: Side::Bid,
            qty: 5,
            kind: OrderKind::Market,
        });
        assert!(matches!(
            out.last(),
            Some(Change::Cancelled {
                id: 2,
                remaining: 3
            })
        ));
        assert_eq!(b.order_count(), 0);
    }

    #[test]
    fn non_positive_limit_price_is_rejected() {
        let mut book = OrderBook::new();
        let changes = book.submit(NewOrder {
            id: 1,
            side: Side::Bid,
            qty: 1,
            kind: OrderKind::Limit { price: 0 },
        });
        assert_eq!(
            changes,
            vec![Change::Rejected {
                id: 1,
                reason: RejectReason::NonPositivePrice,
            }]
        );
    }
    #[test]
    fn cancel_and_reuse_id() {
        let mut b = OrderBook::new();
        b.submit(limit(1, Side::Bid, 99, 2));
        b.cancel(1);
        assert!(matches!(
            b.submit(limit(1, Side::Bid, 98, 1))[0],
            Change::Accepted { id: 1 }
        ));
    }

    #[test]
    fn aggregate_quantity_cannot_overflow() {
        let mut book = OrderBook::new();
        assert!(matches!(
            book.submit(limit(1, Side::Bid, 100, i64::MAX)).as_slice(),
            [Change::Accepted { id: 1 }]
        ));
        assert_eq!(book.best_bid(), Some((100, i64::MAX)));
        assert_eq!(
            book.submit(limit(2, Side::Bid, 100, 1)),
            vec![Change::Rejected {
                id: 2,
                reason: RejectReason::BookQuantityOverflow,
            }]
        );
        assert!(book.check().is_ok());
    }

    #[test]
    fn partial_limit_fill_rests_only_the_remainder() {
        let mut book = OrderBook::new();
        book.submit(limit(1, Side::Ask, 100, 3));
        book.submit(limit(2, Side::Bid, 101, 5));
        assert_eq!(book.best_bid(), Some((101, 2)));
        assert_eq!(book.best_ask(), None);
        assert!(book.check().is_ok());
    }

    #[test]
    fn depth_is_aggregated_best_to_worst_and_honors_limit() {
        let mut book = OrderBook::new();
        book.submit(limit(1, Side::Bid, 99, 2));
        book.submit(limit(2, Side::Bid, 100, 3));
        book.submit(limit(3, Side::Bid, 100, 4));
        assert_eq!(book.depth(Side::Bid, 1), vec![(100, 7)]);
        assert!(book.depth(Side::Bid, 0).is_empty());
        assert!(book.check().is_ok());
    }

    #[test]
    fn invalid_and_unknown_commands_do_not_mutate_the_book() {
        let mut book = OrderBook::new();
        for order in [
            limit(1, Side::Bid, 100, 0),
            limit(2, Side::Bid, 100, -1),
            limit(3, Side::Ask, 0, 1),
        ] {
            assert!(matches!(
                book.submit(order).as_slice(),
                [Change::Rejected { .. }]
            ));
        }
        assert!(matches!(
            book.cancel(9).as_slice(),
            [Change::Rejected {
                reason: RejectReason::UnknownOrder,
                ..
            }]
        ));
        assert_eq!(book.order_count(), 0);
        assert!(book.check().is_ok());
    }

    #[test]
    fn deterministic_command_stream_preserves_invariants() {
        fn run() -> (OrderBook, Vec<Change>) {
            let mut book = OrderBook::new();
            let mut changes = Vec::new();
            let mut state = 0x9e37_79b9_7f4a_7c15_u64;
            for id in 1..=2_000 {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                let side = if state & 1 == 0 { Side::Bid } else { Side::Ask };
                let price = 90 + ((state >> 8) % 21) as i64;
                let qty = 1 + ((state >> 16) % 20) as i64;
                book.submit_into(limit(id, side, price, qty), &mut changes);
                if id % 7 == 0 {
                    book.cancel_into(id - 3, &mut changes);
                }
                assert!(book.check().is_ok());
            }
            (book, changes)
        }
        let (first_book, first_changes) = run();
        let (second_book, second_changes) = run();
        assert_eq!(first_changes, second_changes);
        assert_eq!(
            first_book.depth(Side::Bid, usize::MAX),
            second_book.depth(Side::Bid, usize::MAX)
        );
        assert_eq!(
            first_book.depth(Side::Ask, usize::MAX),
            second_book.depth(Side::Ask, usize::MAX)
        );
    }
}
