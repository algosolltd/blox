//! Authoritative books — real orders, real identity, price-time priority.
//! See `docs/CORE.md` §5.
//!
//! An arena of slots with an intrusive doubly-linked list per price level. The
//! two operations that must be fast are matching (walk the best level in
//! arrival order) and cancel-by-id — in any real book the large majority of
//! orders are cancelled, not filled.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

use crate::event::*;
use crate::types::*;

const NIL: u32 = u32::MAX;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct Level {
    price: Ticks,
    /// Always equal to the sum of `qty` over the level's list. Asserted.
    total: Lots,
    /// Oldest — fills first.
    head: u32,
    /// Newest.
    tail: u32,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct Slot {
    id: OrderId,
    owner: OwnerId,
    /// Stored so cancel can locate its level without a back-pointer. Storing a
    /// level *index* instead would be corrupted by every level insert/remove.
    price: Ticks,
    side: Side,
    qty: Lots,
    prev: u32,
    /// Doubles as the free-list link when the slot is dead.
    next: u32,
    live: bool,
}

impl Slot {
    const DEAD: Slot = Slot {
        id: 0,
        owner: 0,
        price: 0,
        side: Side::Bid,
        qty: 0,
        prev: NIL,
        next: NIL,
        live: false,
    };
}

enum TakeResult {
    Remaining(Lots),
    /// Self-trade prevention rejected the incoming order.
    Aborted,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct OrderBook {
    /// DESCENDING — best bid at front.
    bids: VecDeque<Level>,
    /// ASCENDING — best ask at front.
    asks: VecDeque<Level>,
    slots: Vec<Slot>,
    free: u32,
    index: BTreeMap<OrderId, u32>,
    pub stp: StpMode,
}

impl OrderBook {
    pub fn new() -> Self {
        Self {
            free: NIL,
            ..Default::default()
        }
    }

    pub fn with_stp(stp: StpMode) -> Self {
        Self {
            free: NIL,
            stp,
            ..Default::default()
        }
    }

    // ---- level access -----------------------------------------------------

    #[inline]
    fn levels(&self, s: Side) -> &VecDeque<Level> {
        match s {
            Side::Bid => &self.bids,
            Side::Ask => &self.asks,
        }
    }

    #[inline]
    fn levels_mut(&mut self, s: Side) -> &mut VecDeque<Level> {
        match s {
            Side::Bid => &mut self.bids,
            Side::Ask => &mut self.asks,
        }
    }

    /// Bids descend so the comparator is reversed; asks ascend so it is not.
    #[inline]
    fn find_level(levels: &VecDeque<Level>, s: Side, price: Ticks) -> Result<usize, usize> {
        match s {
            Side::Bid => levels.binary_search_by(|l| price.cmp(&l.price)),
            Side::Ask => levels.binary_search_by(|l| l.price.cmp(&price)),
        }
    }

    // ---- arena ------------------------------------------------------------

    fn alloc(&mut self, s: Slot) -> u32 {
        if self.free == NIL {
            self.slots.push(s);
            (self.slots.len() - 1) as u32
        } else {
            let i = self.free;
            self.free = self.slots[i as usize].next;
            self.slots[i as usize] = s;
            i
        }
    }

    fn dealloc(&mut self, i: u32) {
        self.slots[i as usize] = Slot {
            next: self.free,
            ..Slot::DEAD
        };
        self.free = i;
    }

    // ---- structural -------------------------------------------------------

    /// Remove slot `i` from its level's list, anywhere in the list.
    ///
    /// Every removal that is *not* at the head routes through here, so there
    /// is exactly one place that can get the `total` bookkeeping wrong.
    fn unlink(&mut self, i: u32) {
        let s = self.slots[i as usize];
        debug_assert!(s.live, "unlinking a dead slot");
        let li = Self::find_level(self.levels(s.side), s.side, s.price)
            .expect("an order's level must exist");

        if s.prev != NIL {
            self.slots[s.prev as usize].next = s.next;
        }
        if s.next != NIL {
            self.slots[s.next as usize].prev = s.prev;
        }

        let levels = self.levels_mut(s.side);
        let l = &mut levels[li];
        if l.head == i {
            l.head = s.next;
        }
        if l.tail == i {
            l.tail = s.prev;
        }
        l.total -= s.qty;

        if l.head == NIL {
            debug_assert_eq!(l.total, 0, "empty level with non-zero total");
            levels.remove(li);
        }
    }

    fn rest_order(&mut self, id: OrderId, owner: OwnerId, side: Side, price: Ticks, qty: Lots) {
        let slot = self.alloc(Slot {
            id,
            owner,
            price,
            side,
            qty,
            prev: NIL,
            next: NIL,
            live: true,
        });

        match Self::find_level(self.levels(side), side, price) {
            Ok(i) => {
                // Join the back of the queue at this price.
                let tail = self.levels(side)[i].tail;
                let l = &mut self.levels_mut(side)[i];
                l.tail = slot;
                l.total += qty;
                self.slots[slot as usize].prev = tail;
                if tail != NIL {
                    self.slots[tail as usize].next = slot;
                }
            }
            Err(i) => {
                self.levels_mut(side).insert(
                    i,
                    Level {
                        price,
                        total: qty,
                        head: slot,
                        tail: slot,
                    },
                );
            }
        }
        self.index.insert(id, slot);
    }

    /// Fix up the front level after consuming from its head.
    fn commit_front(&mut self, side: Side, price: Ticks, consumed: Lots, new_head: u32) {
        let levels = self.levels_mut(side);
        let Some(l) = levels.front_mut() else { return };
        debug_assert_eq!(l.price, price, "front level changed during matching");
        l.total -= consumed;
        l.head = new_head;
        if new_head == NIL {
            debug_assert_eq!(l.total, 0, "drained level with non-zero total");
            levels.pop_front();
        }
    }

    // ---- matching ---------------------------------------------------------

    /// Take liquidity from the opposite side, best price outward, arrival
    /// order within each price.
    ///
    /// Only ever removes from the head of the front level, so it never needs
    /// general `unlink`.
    #[allow(clippy::too_many_arguments)]
    fn take(
        &mut self,
        side: Side,
        limit: Option<Ticks>,
        qty: Lots,
        taker_id: OrderId,
        taker_owner: OwnerId,
        inst: InstrumentId,
        out: &mut Vec<Change>,
    ) -> TakeResult {
        let mut remaining = qty;
        let opp = side.opposite();

        loop {
            if remaining <= 0 {
                break;
            }

            let (lvl_price, mut cur) = {
                let Some(l) = self.levels(opp).front() else { break };
                if let Some(lim) = limit {
                    if !side.crosses(lim, l.price) {
                        break;
                    }
                }
                (l.price, l.head)
            };

            // Quantity leaving this level, whether traded or STP-cancelled.
            let mut consumed: Lots = 0;

            while cur != NIL && remaining > 0 {
                let s = self.slots[cur as usize];
                let next = s.next;

                if self.stp != StpMode::None && s.owner == taker_owner {
                    match self.stp {
                        StpMode::CancelResting => {
                            self.index.remove(&s.id);
                            self.dealloc(cur);
                            consumed += s.qty;
                            out.push(Change::Cancelled {
                                id: s.id,
                                remaining: s.qty,
                            });
                            cur = next;
                            if next != NIL {
                                self.slots[next as usize].prev = NIL;
                            }
                            continue;
                        }
                        StpMode::CancelIncoming => {
                            self.commit_front(opp, lvl_price, consumed, cur);
                            out.push(Change::Rejected {
                                id: taker_id,
                                reason: RejectReason::SelfTrade,
                            });
                            return TakeResult::Aborted;
                        }
                        StpMode::None => unreachable!(),
                    }
                }

                let fill = remaining.min(s.qty);
                remaining -= fill;
                consumed += fill;

                // The MAKER's price, always. The resting order sets the price;
                // the aggressor accepts it.
                out.push(Change::Trade {
                    instrument: inst,
                    price: lvl_price,
                    qty: fill,
                    maker: s.id,
                    taker: taker_id,
                    aggressor: side,
                });
                out.push(Change::Filled {
                    id: s.id,
                    price: lvl_price,
                    qty: fill,
                    remaining: s.qty - fill,
                });
                out.push(Change::Filled {
                    id: taker_id,
                    price: lvl_price,
                    qty: fill,
                    remaining,
                });

                if fill == s.qty {
                    self.index.remove(&s.id);
                    self.dealloc(cur);
                    cur = next;
                    if next != NIL {
                        self.slots[next as usize].prev = NIL;
                    }
                } else {
                    self.slots[cur as usize].qty -= fill;
                }
            }

            self.commit_front(opp, lvl_price, consumed, cur);

            // Level survived with orders left but we stopped: only possible
            // when remaining hit zero. Defensive against an infinite loop.
            if remaining > 0 && cur != NIL {
                break;
            }
        }

        TakeResult::Remaining(remaining)
    }

    // ---- read-only helpers ------------------------------------------------

    #[inline]
    pub fn best_bid(&self) -> Option<(Ticks, Lots)> {
        self.bids.front().map(|l| (l.price, l.total))
    }

    #[inline]
    pub fn best_ask(&self) -> Option<(Ticks, Lots)> {
        self.asks.front().map(|l| (l.price, l.total))
    }

    #[inline]
    pub fn top(&self) -> Top {
        Top {
            bid: self.best_bid(),
            ask: self.best_ask(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.bids.is_empty() && self.asks.is_empty()
    }

    pub fn order_count(&self) -> usize {
        self.index.len()
    }

    /// Depth snapshot, best first, for aggregation and display.
    pub fn depth(&self, side: Side, max: usize) -> Vec<(Ticks, Lots)> {
        self.levels(side)
            .iter()
            .take(max)
            .map(|l| (l.price, l.total))
            .collect()
    }

    /// Total quantity a taker on `side` could get at `limit` or better.
    ///
    /// Ignores self-trade prevention — see the ponytail note on `Fok` below.
    pub fn available(&self, side: Side, limit: Option<Ticks>) -> Lots {
        self.levels(side.opposite())
            .iter()
            .take_while(|l| limit.is_none_or(|p| side.crosses(p, l.price)))
            .map(|l| l.total)
            .sum()
    }

    pub fn would_cross(&self, side: Side, price: Ticks) -> bool {
        self.levels(side.opposite())
            .front()
            .is_some_and(|l| side.crosses(price, l.price))
    }

    // ---- entry points -----------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub fn new_order(
        &mut self,
        inst: InstrumentId,
        id: OrderId,
        owner: OwnerId,
        side: Side,
        price: Ticks,
        qty: Lots,
        kind: OrderKind,
        out: &mut Vec<Change>,
    ) {
        // Validation before anything mutates.
        if qty == 0 {
            out.push(Change::Rejected {
                id,
                reason: RejectReason::ZeroQty,
            });
            return;
        }
        if qty < 0 {
            out.push(Change::Rejected {
                id,
                reason: RejectReason::NegativeQty,
            });
            return;
        }
        if self.index.contains_key(&id) {
            out.push(Change::Rejected {
                id,
                reason: RejectReason::DuplicateId,
            });
            return;
        }

        // Pre-checks strictly before mutation, so a rejection leaves the book
        // byte-identical. Checking inline during the walk would need an unwind
        // path, and an unwind path is a rollback mechanism to keep correct
        // forever.
        let reason = match kind {
            OrderKind::PostOnly if self.would_cross(side, price) => Some(RejectReason::WouldCross),
            OrderKind::Fok if self.available(side, Some(price)) < qty => {
                Some(RejectReason::InsufficientBook)
            }
            OrderKind::Market if self.levels(side.opposite()).is_empty() => {
                Some(RejectReason::NoLiquidity)
            }
            _ => None,
        };
        if let Some(reason) = reason {
            out.push(Change::Rejected { id, reason });
            return;
        }

        out.push(Change::Accepted { id });

        let limit = match kind {
            OrderKind::Market => None,
            _ => Some(price),
        };

        let rest = match kind {
            // Pre-checked as non-crossing, so it never takes.
            OrderKind::PostOnly => qty,
            _ => match self.take(side, limit, qty, id, owner, inst, out) {
                // STP already emitted the rejection.
                TakeResult::Aborted => return,
                TakeResult::Remaining(r) => r,
            },
        };

        if rest > 0 {
            match kind {
                OrderKind::Limit | OrderKind::PostOnly => {
                    self.rest_order(id, owner, side, price, rest)
                }
                OrderKind::Ioc | OrderKind::Market => {
                    out.push(Change::Cancelled { id, remaining: rest })
                }
                OrderKind::Fok => {
                    // ponytail: Fok's pre-check ignores STP, so an owner
                    // quoting both sides can pass it and then under-fill.
                    // Harmless while STP is None (games, sims). Fix when a
                    // market-maker owner appears: make `available` skip levels
                    // owned by the taker.
                    debug_assert!(
                        self.stp != StpMode::None,
                        "Fok passed its pre-check but did not fully fill"
                    );
                    out.push(Change::Cancelled { id, remaining: rest });
                }
            }
        }
    }

    pub fn cancel(&mut self, id: OrderId, out: &mut Vec<Change>) {
        let Some(&i) = self.index.get(&id) else {
            out.push(Change::Rejected {
                id,
                reason: RejectReason::UnknownOrder,
            });
            return;
        };
        let remaining = self.slots[i as usize].qty;
        self.unlink(i);
        self.index.remove(&id);
        self.dealloc(i);
        out.push(Change::Cancelled { id, remaining });
    }

    /// Quantity-only amend.
    ///
    /// Decreasing keeps queue position; increasing loses it. This is the
    /// universal exchange convention and it exists because the alternative is
    /// exploitable — you would queue 1 lot at the front and inflate it the
    /// moment something interesting appeared, jumping everyone who committed
    /// real size.
    pub fn amend(&mut self, id: OrderId, qty: Lots, out: &mut Vec<Change>) {
        let Some(&i) = self.index.get(&id) else {
            out.push(Change::Rejected {
                id,
                reason: RejectReason::UnknownOrder,
            });
            return;
        };
        if qty <= 0 {
            return self.cancel(id, out);
        }

        let s = self.slots[i as usize];
        if qty == s.qty {
            return;
        }

        if qty < s.qty {
            let delta = s.qty - qty;
            self.slots[i as usize].qty = qty;
            let li = Self::find_level(self.levels(s.side), s.side, s.price).unwrap();
            self.levels_mut(s.side)[li].total -= delta;
        } else {
            // Increase is cancel/replace: same path, no special case.
            self.unlink(i);
            self.index.remove(&id);
            self.dealloc(i);
            self.rest_order(id, s.owner, s.side, s.price, qty);
        }
        out.push(Change::Amended { id, qty });
    }

    /// Reduce an order to at most `qty`, never increase it.
    ///
    /// This is the safe boundary for user-facing reduce commands: if a fill
    /// races the command and has already brought the order below the requested
    /// size, the command is acknowledged with the current size and is a no-op.
    pub fn reduce(&mut self, id: OrderId, qty: Lots, out: &mut Vec<Change>) {
        let Some(&i) = self.index.get(&id) else {
            out.push(Change::Rejected {
                id,
                reason: RejectReason::UnknownOrder,
            });
            return;
        };
        let current = self.slots[i as usize].qty;
        if qty >= current {
            out.push(Change::Amended { id, qty: current });
            return;
        }
        self.amend(id, qty, out);
    }

    // ---- invariants -------------------------------------------------------

    /// All seven `OrderBook` invariants from `docs/CORE.md` §8.
    ///
    /// Every one of these has been a real bug in a real order book somewhere.
    pub fn check(&self) -> Result<(), String> {
        let mut linked = 0usize;

        for (name, side) in [("bids", Side::Bid), ("asks", Side::Ask)] {
            let levels = self.levels(side);

            // (2) sorted, no duplicate prices
            for w in levels.iter().zip(levels.iter().skip(1)) {
                let ok = match side {
                    Side::Bid => w.0.price > w.1.price,
                    Side::Ask => w.0.price < w.1.price,
                };
                if !ok {
                    return Err(format!(
                        "{name} not sorted: {} then {}",
                        w.0.price, w.1.price
                    ));
                }
            }

            for (li, l) in levels.iter().enumerate() {
                // (5) no empty levels retained
                if l.head == NIL {
                    return Err(format!("{name}[{li}] @ {} is empty", l.price));
                }

                // (6) endpoints, (1) total conservation, list integrity
                let mut sum = 0;
                let mut cur = l.head;
                let mut prev = NIL;
                let mut guard = 0;
                while cur != NIL {
                    let s = self.slots[cur as usize];
                    if !s.live {
                        return Err(format!("{name}[{li}] links dead slot {cur}"));
                    }
                    if s.price != l.price {
                        return Err(format!(
                            "slot {cur} price {} in level @ {}",
                            s.price, l.price
                        ));
                    }
                    if s.side != side {
                        return Err(format!("slot {cur} on wrong side"));
                    }
                    if s.prev != prev {
                        return Err(format!("slot {cur} prev link broken"));
                    }
                    if s.qty <= 0 {
                        return Err(format!("slot {cur} has non-positive qty {}", s.qty));
                    }
                    match self.index.get(&s.id) {
                        Some(&idx) if idx == cur => {}
                        _ => return Err(format!("slot {cur} (id {}) not indexed", s.id)),
                    }
                    sum += s.qty;
                    linked += 1;
                    prev = cur;
                    cur = s.next;

                    guard += 1;
                    if guard > self.slots.len() + 1 {
                        return Err(format!("{name}[{li}] list is cyclic"));
                    }
                }
                if l.tail != prev {
                    return Err(format!("{name}[{li}] tail is not the last node"));
                }
                if sum != l.total {
                    return Err(format!(
                        "{name}[{li}] @ {}: total {} != sum {}",
                        l.price, l.total, sum
                    ));
                }
            }
        }

        // (3) never internally crossed
        if let (Some((b, _)), Some((a, _))) = (self.best_bid(), self.best_ask()) {
            if b >= a {
                return Err(format!("book crossed: bid {b} >= ask {a}"));
            }
        }

        // (4) index matches the linked set
        if linked != self.index.len() {
            return Err(format!(
                "index has {} entries, {linked} slots linked",
                self.index.len()
            ));
        }

        // (7) the free list touches nothing live
        let mut f = self.free;
        let mut guard = 0;
        while f != NIL {
            if self.slots[f as usize].live {
                return Err(format!("free slot {f} is marked live"));
            }
            f = self.slots[f as usize].next;
            guard += 1;
            if guard > self.slots.len() + 1 {
                return Err("free list is cyclic".into());
            }
        }

        Ok(())
    }
}
