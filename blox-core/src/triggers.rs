//! Conditional orders — stops, stop-limits, limits, TP/SL, OCO.
//! See `docs/CONDITIONAL_ORDERS.md`.

use std::collections::HashMap;

use crate::event::*;
use crate::types::*;

/// Bounded so a re-insertion bug fails visibly instead of spinning forever.
pub const MAX_CASCADE: u32 = 8;

/// Pending triggers for one instrument.
///
/// Four buckets keyed by `(side, direction of travel)`.
///
/// **Not two buckets.** The obvious design groups only by direction, but buy
/// triggers reference the ask and sell triggers reference the bid, so a
/// direction-only bucket holds two different reference prices. A head that
/// does not fire can then hide a later entry that should — e.g. a sell limit
/// at 100 (bid 99, no fire) sitting in front of a buy stop at 101 (ask 102,
/// fires). Bucketing by side as well makes every bucket homogeneous, so the
/// head check is valid again. Still O(1): four integer comparisons.
#[derive(Clone, Debug, Default)]
pub struct Triggers {
    /// [Bid-down, Bid-up, Ask-down, Ask-up].
    /// "up" buckets ascend (lowest fires first as price rises);
    /// "down" buckets descend (highest fires first as price falls).
    buckets: [Vec<Trigger>; 4],
    ids: HashMap<OrderId, usize>,
}

#[inline]
fn bucket_of(side: Side, up: bool) -> usize {
    ((side as usize) << 1) | (up as usize)
}

#[inline]
fn is_up(bucket: usize) -> bool {
    bucket & 1 == 1
}

#[inline]
fn side_of(bucket: usize) -> Side {
    if bucket >> 1 == 0 {
        Side::Bid
    } else {
        Side::Ask
    }
}

impl Triggers {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn contains(&self, id: OrderId) -> bool {
        self.ids.contains_key(&id)
    }

    /// Would this trigger fire the instant it is placed?
    ///
    /// Rejecting beats firing: a user placing a stop expects it to wait, and
    /// firing immediately turns a mistyped protective order into an instant
    /// market order in the wrong direction.
    pub fn would_fire_now(t: &Trigger, top: &Top) -> bool {
        top.taker_price(t.side)
            .is_some_and(|p| t.satisfied_at(p))
    }

    /// Insert, keeping the bucket sorted and equal prices in placement order.
    // ponytail: Vec + memmove suits hundreds of triggers per instrument, and
    // placement is thousands of times rarer than price updates. Past ~10k,
    // switch to BTreeMap<Ticks, SmallVec<Trigger>> with the same head check.
    pub fn insert(&mut self, t: Trigger) {
        let b = bucket_of(t.side, t.fires_upward());
        let v = &mut self.buckets[b];
        // `<=` / `>=` so equal prices land *after* existing ones: first
        // placed, first fired. Same principle as time priority in OrderBook,
        // and it is what makes a gap that trips twenty stops reproducible.
        let pos = if is_up(b) {
            v.partition_point(|x| x.trigger_price <= t.trigger_price)
        } else {
            v.partition_point(|x| x.trigger_price >= t.trigger_price)
        };
        v.insert(pos, t);
        self.ids.insert(t.id, b);
    }

    pub fn remove(&mut self, id: OrderId) -> Option<Trigger> {
        let b = self.ids.remove(&id)?;
        let v = &mut self.buckets[b];
        let i = v.iter().position(|t| t.id == id)?;
        Some(v.remove(i))
    }

    pub fn get(&self, id: OrderId) -> Option<&Trigger> {
        let b = *self.ids.get(&id)?;
        self.buckets[b].iter().find(|t| t.id == id)
    }

    /// Every trigger attached to a position — for TP/SL cleanup on close.
    pub fn attached_to(&self, pos: PositionId) -> Vec<OrderId> {
        let mut v: Vec<OrderId> = self
            .buckets
            .iter()
            .flatten()
            .filter(|t| t.attached_to == Some(pos))
            .map(|t| t.id)
            .collect();
        v.sort_unstable(); // determinism
        v
    }

    /// Cheap "is there anything to do?" check. Four comparisons regardless of
    /// how many thousands are parked.
    pub fn any_fires(&self, top: &Top) -> bool {
        (0..4).any(|b| self.head_fires(b, top))
    }

    #[inline]
    fn head_fires(&self, b: usize, top: &Top) -> bool {
        let Some(t) = self.buckets[b].first() else {
            return false;
        };
        top.taker_price(side_of(b))
            .is_some_and(|p| t.satisfied_at(p))
    }

    /// Remove and return the next trigger that fires, or `None`.
    ///
    /// Buckets are scanned in fixed index order, so when a gap trips several
    /// at once the output order is the same on every run.
    pub fn pop_fired(&mut self, top: &Top) -> Option<Trigger> {
        for b in 0..4 {
            if self.head_fires(b, top) {
                let t = self.buckets[b].remove(0);
                self.ids.remove(&t.id);
                return Some(t);
            }
        }
        None
    }

    /// Invariant: nothing may remain whose condition is already satisfied.
    /// The single assertion that covers most of what can go wrong here.
    pub fn check(&self, top: &Top) -> Result<(), String> {
        for b in 0..4 {
            let v = &self.buckets[b];
            for w in v.windows(2) {
                let ok = if is_up(b) {
                    w[0].trigger_price <= w[1].trigger_price
                } else {
                    w[0].trigger_price >= w[1].trigger_price
                };
                if !ok {
                    return Err(format!(
                        "bucket {b} not sorted: {} then {}",
                        w[0].trigger_price, w[1].trigger_price
                    ));
                }
            }
            if let Some(t) = v.first() {
                if let Some(p) = top.taker_price(side_of(b)) {
                    if t.satisfied_at(p) {
                        return Err(format!(
                            "trigger {} @ {} should have fired at reference {p}",
                            t.id, t.trigger_price
                        ));
                    }
                }
            }
        }
        let counted: usize = self.buckets.iter().map(|v| v.len()).sum();
        if counted != self.ids.len() {
            return Err(format!(
                "{} triggers in buckets, {} in index",
                counted,
                self.ids.len()
            ));
        }
        Ok(())
    }
}

/// What a fired trigger turns into.
pub fn fired_as(t: &Trigger) -> FiredAs {
    match t.kind {
        TriggerKind::Stop => FiredAs::Market,
        TriggerKind::StopLimit { limit_price } => FiredAs::Limit { price: limit_price },
        // A limit order that has been waiting executes at its own price.
        TriggerKind::Limit => FiredAs::Limit {
            price: t.trigger_price,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn top(bid: Ticks, ask: Ticks) -> Top {
        Top {
            bid: Some((bid, 10)),
            ask: Some((ask, 10)),
        }
    }

    fn stop(id: OrderId, side: Side, price: Ticks) -> Trigger {
        Trigger::new(id, 1, InstrumentId(1), side, price, 10, TriggerKind::Stop)
    }

    fn limit(id: OrderId, side: Side, price: Ticks) -> Trigger {
        Trigger::new(id, 1, InstrumentId(1), side, price, 10, TriggerKind::Limit)
    }

    #[test]
    fn direction_of_travel_is_correct_per_kind() {
        // Buy stop sits above the market, fires as price rises.
        assert!(stop(1, Side::Bid, 110).fires_upward());
        // Sell stop sits below, fires as price falls.
        assert!(!stop(2, Side::Ask, 90).fires_upward());
        // Buy limit sits below, fires as price falls.
        assert!(!limit(3, Side::Bid, 90).fires_upward());
        // Sell limit sits above, fires as price rises.
        assert!(limit(4, Side::Ask, 110).fires_upward());
    }

    #[test]
    fn buy_stop_references_ask_not_mid() {
        let mut t = Triggers::new();
        // bid 10840 / ask 10860 -> mid 10850.
        let market = top(10_840, 10_860);
        // A buy stop at 10850: mid has crossed it, the ask has not... but the
        // ask is 10860 which IS >= 10850, so it fires. Use a stop above the
        // ask to prove mid is not consulted.
        t.insert(stop(1, Side::Bid, 10_855));
        assert!(t.any_fires(&market), "ask 10860 >= 10855 must fire");

        let mut t2 = Triggers::new();
        t2.insert(stop(2, Side::Bid, 10_870));
        assert!(!t2.any_fires(&market), "ask 10860 < 10870 must not fire");
    }

    #[test]
    fn sell_stop_references_bid_not_mid() {
        let mut t = Triggers::new();
        let market = top(10_840, 10_860);
        // Sell stop at 10850: mid (10850) has reached it, but the bid (10840)
        // is already below, so it fires on the bid.
        t.insert(stop(1, Side::Ask, 10_850));
        assert!(t.any_fires(&market));

        // Sell stop at 10830: bid 10840 is still above it. No fire.
        let mut t2 = Triggers::new();
        t2.insert(stop(2, Side::Ask, 10_830));
        assert!(!t2.any_fires(&market));
    }

    #[test]
    fn a_sell_limit_head_cannot_hide_a_firing_buy_stop() {
        // The bug that four buckets exist to prevent.
        let mut t = Triggers::new();
        t.insert(limit(1, Side::Ask, 100)); // fires when bid >= 100
        t.insert(stop(2, Side::Bid, 101)); // fires when ask >= 101

        // bid 99 (sell limit does NOT fire), ask 102 (buy stop DOES).
        let market = top(99, 102);
        let fired = t.pop_fired(&market).expect("buy stop must fire");
        assert_eq!(fired.id, 2);
        assert!(t.pop_fired(&market).is_none());
    }

    #[test]
    fn equal_price_triggers_fire_in_placement_order() {
        let mut t = Triggers::new();
        t.insert(stop(1, Side::Bid, 100));
        t.insert(stop(2, Side::Bid, 100));
        t.insert(stop(3, Side::Bid, 100));
        let market = top(99, 100);
        assert_eq!(t.pop_fired(&market).unwrap().id, 1);
        assert_eq!(t.pop_fired(&market).unwrap().id, 2);
        assert_eq!(t.pop_fired(&market).unwrap().id, 3);
    }

    #[test]
    fn nearest_trigger_fires_first() {
        let mut t = Triggers::new();
        t.insert(stop(1, Side::Bid, 120));
        t.insert(stop(2, Side::Bid, 105));
        t.insert(stop(3, Side::Bid, 110));
        let market = top(99, 125);
        // All three fire; nearest to the market first.
        assert_eq!(t.pop_fired(&market).unwrap().id, 2); // 105
        assert_eq!(t.pop_fired(&market).unwrap().id, 3); // 110
        assert_eq!(t.pop_fired(&market).unwrap().id, 1); // 120
    }

    #[test]
    fn remove_and_lookup() {
        let mut t = Triggers::new();
        t.insert(stop(1, Side::Bid, 100));
        t.insert(stop(2, Side::Ask, 90));
        assert_eq!(t.len(), 2);
        assert!(t.contains(1));
        assert_eq!(t.remove(1).unwrap().id, 1);
        assert!(!t.contains(1));
        assert_eq!(t.len(), 1);
        assert!(t.remove(99).is_none());
    }

    #[test]
    fn attached_lookup_is_sorted() {
        let mut t = Triggers::new();
        let mut a = stop(5, Side::Bid, 100);
        a.attached_to = Some(7);
        let mut b = stop(3, Side::Ask, 90);
        b.attached_to = Some(7);
        let c = stop(4, Side::Ask, 80);
        t.insert(a);
        t.insert(b);
        t.insert(c);
        assert_eq!(t.attached_to(7), vec![3, 5]);
    }

    #[test]
    fn fired_as_maps_kind_to_execution() {
        assert_eq!(fired_as(&stop(1, Side::Bid, 100)), FiredAs::Market);
        assert_eq!(
            fired_as(&limit(2, Side::Bid, 90)),
            FiredAs::Limit { price: 90 }
        );
        let sl = Trigger::new(
            3,
            1,
            InstrumentId(1),
            Side::Ask,
            100,
            10,
            TriggerKind::StopLimit { limit_price: 95 },
        );
        assert_eq!(fired_as(&sl), FiredAs::Limit { price: 95 });
    }
}
