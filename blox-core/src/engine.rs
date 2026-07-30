//! The container. See `docs/CORE.md` §6 and §7.

use std::collections::HashMap;

use crate::event::*;
use crate::level_book::LevelBook;
use crate::order_book::OrderBook;
use crate::triggers::{fired_as, Triggers, MAX_CASCADE};
use crate::types::*;

/// One price level of an aggregate view, with provider attribution.
///
/// Routing needs to know *who* is showing the price, not just what it is.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AggLevel {
    pub price: Ticks,
    pub qty: Lots,
    /// Ordered by `ProviderId` — never by size. Two providers showing the same
    /// quantity must produce identical bytes on every run.
    pub sources: Vec<(ProviderId, Lots)>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BookView {
    pub instrument: InstrumentId,
    pub bids: Vec<AggLevel>,
    pub asks: Vec<AggLevel>,
}

#[derive(Clone, Debug)]
pub struct Engine {
    level_books: HashMap<(ProviderId, InstrumentId), LevelBook>,
    order_books: HashMap<InstrumentId, OrderBook>,
    /// Sorted. Makes `Clear` O(instruments-for-that-provider) instead of a
    /// scan of every book — the central operation in a multi-provider system.
    by_provider: HashMap<ProviderId, Vec<InstrumentId>>,
    /// Sorted. Aggregation reads providers in a fixed order.
    by_instrument: HashMap<InstrumentId, Vec<ProviderId>>,
    /// Sorted. Deterministic iteration for staleness marking.
    book_keys: Vec<(ProviderId, InstrumentId)>,
    triggers: HashMap<InstrumentId, Triggers>,
    last_top: HashMap<InstrumentId, Top>,
    max_age_ns: u64,
    now_ns: u64,
    stp: StpMode,
    /// Deltas arriving before a snapshot. A non-zero count means an adapter
    /// has a sequencing bug.
    dropped_deltas: u64,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    pub fn new() -> Self {
        Self {
            level_books: HashMap::new(),
            order_books: HashMap::new(),
            by_provider: HashMap::new(),
            by_instrument: HashMap::new(),
            book_keys: Vec::new(),
            triggers: HashMap::new(),
            last_top: HashMap::new(),
            // 0 disables staleness marking. Adapters set it per provider.
            max_age_ns: 0,
            now_ns: 0,
            stp: StpMode::None,
            dropped_deltas: 0,
        }
    }

    pub fn with_max_age_ns(mut self, ns: u64) -> Self {
        self.max_age_ns = ns;
        self
    }

    pub fn with_stp(mut self, stp: StpMode) -> Self {
        self.stp = stp;
        self
    }

    // ---- accessors --------------------------------------------------------

    pub fn now_ns(&self) -> u64 {
        self.now_ns
    }

    pub fn dropped_deltas(&self) -> u64 {
        self.dropped_deltas
    }

    pub fn book(&self, provider: ProviderId, instrument: InstrumentId) -> Option<&LevelBook> {
        self.level_books.get(&(provider, instrument))
    }

    pub fn order_book(&self, instrument: InstrumentId) -> Option<&OrderBook> {
        self.order_books.get(&instrument)
    }

    pub fn triggers_for(&self, instrument: InstrumentId) -> Option<&Triggers> {
        self.triggers.get(&instrument)
    }

    pub fn book_count(&self) -> usize {
        self.level_books.len()
    }

    /// Every instrument with at least one book, sorted.
    pub fn instruments(&self) -> Vec<InstrumentId> {
        let mut v: Vec<_> = self
            .by_instrument
            .keys()
            .copied()
            .chain(self.order_books.keys().copied())
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    // ---- aggregation ------------------------------------------------------

    /// Best bid/ask across every non-stale book for this instrument.
    ///
    /// Quantities at the same best price are summed across providers.
    pub fn top_of(&self, inst: InstrumentId) -> Top {
        let mut top = Top::default();
        if let Some(ob) = self.order_books.get(&inst) {
            Self::merge_top(&mut top, ob.top());
        }
        if let Some(providers) = self.by_instrument.get(&inst) {
            for &p in providers {
                if let Some(lb) = self.level_books.get(&(p, inst)) {
                    // Skip stale books entirely: a frozen feed leaves the
                    // aggregate rather than poisoning it.
                    if lb.stale {
                        continue;
                    }
                    Self::merge_top(&mut top, lb.top());
                }
            }
        }
        top
    }

    fn merge_top(acc: &mut Top, t: Top) {
        if let Some((p, q)) = t.bid {
            acc.bid = match acc.bid {
                None => Some((p, q)),
                Some((bp, _)) if p > bp => Some((p, q)),
                Some((bp, bq)) if p == bp => Some((bp, bq + q)),
                keep => keep,
            };
        }
        if let Some((p, q)) = t.ask {
            acc.ask = match acc.ask {
                None => Some((p, q)),
                Some((ap, _)) if p < ap => Some((p, q)),
                Some((ap, aq)) if p == ap => Some((ap, aq + q)),
                keep => keep,
            };
        }
    }

    /// Merged view across providers, best first, with attribution.
    ///
    /// The aggregate may legitimately cross — LP-A's bid above LP-B's ask is
    /// an arbitrage, and real information. Never clamp it. (A single
    /// `OrderBook` must never cross internally; an aggregate across venues
    /// absolutely may.)
    pub fn aggregate(&self, inst: InstrumentId, depth: usize) -> BookView {
        BookView {
            instrument: inst,
            bids: self.agg_side(inst, Side::Bid, depth),
            asks: self.agg_side(inst, Side::Ask, depth),
        }
    }

    // ponytail: ~1.7us at 5 providers x depth 10, dominated by one heap
    // allocation per `sources` Vec. That is 30x `top_of`, and fine: this is
    // the on-demand query path, while `top_of` (52ns) is what runs on every
    // event. If aggregation ever moves onto the hot path, inline the first
    // 2-4 sources instead of allocating — do not reach for it before then.
    fn agg_side(&self, inst: InstrumentId, side: Side, depth: usize) -> Vec<AggLevel> {
        let providers = self.by_instrument.get(&inst).map_or(0, |v| v.len());
        let mut raw: Vec<(Ticks, ProviderId, Lots)> =
            Vec::with_capacity((providers + 1) * depth);

        if let Some(ob) = self.order_books.get(&inst) {
            raw.extend(ob.depth(side, depth).into_iter().map(|(p, q)| (p, INTERNAL, q)));
        }
        if let Some(providers) = self.by_instrument.get(&inst) {
            for &pid in providers {
                if let Some(lb) = self.level_books.get(&(pid, inst)) {
                    if lb.stale {
                        continue;
                    }
                    raw.extend(
                        lb.side(side)
                            .iter()
                            .take(depth)
                            .map(|&(p, q)| (p, pid, q)),
                    );
                }
            }
        }

        // Best first; ties broken on ProviderId so output is byte-stable.
        raw.sort_unstable_by(|a, b| match side {
            Side::Bid => b.0.cmp(&a.0).then(a.1.cmp(&b.1)),
            Side::Ask => a.0.cmp(&b.0).then(a.1.cmp(&b.1)),
        });

        let mut out: Vec<AggLevel> = Vec::with_capacity(depth.min(raw.len()));
        for (price, pid, qty) in raw {
            match out.last_mut() {
                Some(l) if l.price == price => {
                    l.qty += qty;
                    l.sources.push((pid, qty));
                }
                _ => {
                    if out.len() == depth {
                        break;
                    }
                    out.push(AggLevel {
                        price,
                        qty,
                        sources: vec![(pid, qty)],
                    });
                }
            }
        }
        out
    }

    // ---- apply ------------------------------------------------------------

    /// Primary form. Appends to `out`; reuse the buffer across calls to keep
    /// allocation off the hot path.
    pub fn apply_into(&mut self, e: &Event, out: &mut Vec<Change>) {
        match e {
            Event::Tick { ts_nanos } => {
                self.now_ns = *ts_nanos;
                let affected = self.mark_stale(out);
                for inst in affected {
                    self.emit_top(inst, out);
                    self.check_triggers(inst, out);
                }
            }

            Event::Clear { provider } => {
                let instruments = self.by_provider.remove(provider).unwrap_or_default();
                for inst in &instruments {
                    self.level_books.remove(&(*provider, *inst));
                    if let Some(v) = self.by_instrument.get_mut(inst) {
                        v.retain(|p| p != provider);
                        if v.is_empty() {
                            self.by_instrument.remove(inst);
                        }
                    }
                    out.push(Change::BookCleared {
                        provider: *provider,
                        instrument: *inst,
                    });
                }
                self.book_keys.retain(|(p, _)| p != provider);
                for inst in instruments {
                    self.emit_top(inst, out);
                    self.check_triggers(inst, out);
                }
            }

            Event::Snapshot {
                provider,
                instrument,
                bids,
                asks,
            } => {
                self.register(*provider, *instrument);
                let now = self.now_ns;
                let b = self
                    .level_books
                    .entry((*provider, *instrument))
                    .or_default();
                b.replace(bids.clone(), asks.clone());
                b.last_update_ns = now;
                b.stale = false;
                out.push(Change::BookReplaced {
                    provider: *provider,
                    instrument: *instrument,
                });
                self.emit_top(*instrument, out);
                self.check_triggers(*instrument, out);
            }

            Event::Delta {
                provider,
                instrument,
                side,
                price,
                qty,
            } => {
                let now = self.now_ns;
                let Some(b) = self.level_books.get_mut(&(*provider, *instrument)) else {
                    // A delta before any snapshot means the adapter has a
                    // sequencing bug. Creating the book here would build one
                    // with a hole in it that looks valid and prices wrongly
                    // forever; dropping it is recoverable, because gap
                    // detection will send a snapshot.
                    self.dropped_deltas += 1;
                    debug_assert!(false, "delta for {provider:?}/{instrument:?} before snapshot");
                    return;
                };
                if b.apply_delta(*side, *price, *qty) {
                    b.last_update_ns = now;
                    b.stale = false;
                    out.push(Change::LevelUpdated {
                        provider: *provider,
                        instrument: *instrument,
                        side: *side,
                        price: *price,
                        qty: *qty,
                    });
                    self.emit_top(*instrument, out);
                    self.check_triggers(*instrument, out);
                }
            }

            Event::New {
                instrument,
                id,
                owner,
                side,
                price,
                qty,
                kind,
            } => {
                let stp = self.stp;
                self.order_books
                    .entry(*instrument)
                    .or_insert_with(|| OrderBook::with_stp(stp))
                    .new_order(*instrument, *id, *owner, *side, *price, *qty, *kind, out);
                self.emit_top(*instrument, out);
                self.check_triggers(*instrument, out);
            }

            Event::Cancel { instrument, id } => {
                if let Some(ob) = self.order_books.get_mut(instrument) {
                    ob.cancel(*id, out);
                } else {
                    out.push(Change::Rejected {
                        id: *id,
                        reason: RejectReason::UnknownOrder,
                    });
                    return;
                }
                self.emit_top(*instrument, out);
                self.check_triggers(*instrument, out);
            }

            Event::Amend {
                instrument,
                id,
                qty,
            } => {
                if let Some(ob) = self.order_books.get_mut(instrument) {
                    ob.amend(*id, *qty, out);
                } else {
                    out.push(Change::Rejected {
                        id: *id,
                        reason: RejectReason::UnknownOrder,
                    });
                    return;
                }
                self.emit_top(*instrument, out);
                self.check_triggers(*instrument, out);
            }

            Event::PlaceTrigger { trigger } => {
                let inst = trigger.instrument;
                if trigger.qty <= 0 {
                    out.push(Change::Rejected {
                        id: trigger.id,
                        reason: if trigger.qty == 0 {
                            RejectReason::ZeroQty
                        } else {
                            RejectReason::NegativeQty
                        },
                    });
                    return;
                }
                let top = self.top_of(inst);
                if Triggers::would_fire_now(trigger, &top) {
                    out.push(Change::Rejected {
                        id: trigger.id,
                        reason: RejectReason::WouldFireImmediately,
                    });
                    return;
                }
                let tr = self.triggers.entry(inst).or_default();
                if tr.contains(trigger.id) {
                    out.push(Change::Rejected {
                        id: trigger.id,
                        reason: RejectReason::DuplicateId,
                    });
                    return;
                }
                tr.insert(*trigger);
                out.push(Change::TriggerPlaced { id: trigger.id });
            }

            Event::CancelTrigger { instrument, id } => {
                let removed = self
                    .triggers
                    .get_mut(instrument)
                    .and_then(|t| t.remove(*id))
                    .is_some();
                if removed {
                    out.push(Change::TriggerCancelled {
                        id: *id,
                        reason: CancelReason::ByUser,
                    });
                } else {
                    out.push(Change::Rejected {
                        id: *id,
                        reason: RejectReason::UnknownOrder,
                    });
                }
            }
        }
    }

    /// Convenience. Allocates — prefer `apply_into` on a hot path.
    pub fn apply(&mut self, e: &Event) -> Vec<Change> {
        let mut out = Vec::new();
        self.apply_into(e, &mut out);
        out
    }

    /// One FFI crossing for many events. Essential for backtests: replaying a
    /// year of ticks from Python must not pay the boundary cost per event.
    pub fn apply_batch(&mut self, events: &[Event], out: &mut Vec<Change>) {
        for e in events {
            self.apply_into(e, out);
        }
    }

    // ---- internals --------------------------------------------------------

    fn register(&mut self, p: ProviderId, i: InstrumentId) {
        let v = self.by_provider.entry(p).or_default();
        if let Err(pos) = v.binary_search(&i) {
            v.insert(pos, i);
        }
        let v = self.by_instrument.entry(i).or_default();
        if let Err(pos) = v.binary_search(&p) {
            v.insert(pos, p);
        }
        if let Err(pos) = self.book_keys.binary_search(&(p, i)) {
            self.book_keys.insert(pos, (p, i));
        }
    }

    /// Mark books that have gone quiet. Returns the affected instruments,
    /// sorted and deduplicated.
    ///
    /// Iterates `book_keys`, which is kept sorted — iterating the `HashMap`
    /// would leak hash order into the output and break replay.
    fn mark_stale(&mut self, out: &mut Vec<Change>) -> Vec<InstrumentId> {
        let mut affected = Vec::new();
        if self.max_age_ns == 0 {
            return affected;
        }
        let now = self.now_ns;
        for &(p, i) in &self.book_keys {
            let Some(b) = self.level_books.get_mut(&(p, i)) else {
                continue;
            };
            if b.stale {
                continue;
            }
            let age = now.saturating_sub(b.last_update_ns);
            if age > self.max_age_ns {
                b.stale = true;
                out.push(Change::BookStale {
                    provider: p,
                    instrument: i,
                    age_nanos: age,
                });
                affected.push(i);
            }
        }
        affected.dedup();
        affected
    }

    /// Emit `TopOfBook` only when it actually differs from the last one.
    ///
    /// The highest-leverage line in the engine: most book updates happen deep
    /// in the book and change nothing a consumer cares about. Suppressing them
    /// here means they never reach conflation, never cross a socket, and never
    /// wake a browser.
    fn emit_top(&mut self, inst: InstrumentId, out: &mut Vec<Change>) {
        let top = self.top_of(inst);
        if self.last_top.get(&inst) == Some(&top) {
            return;
        }
        self.last_top.insert(inst, top);
        out.push(Change::TopOfBook {
            instrument: inst,
            bid: top.bid,
            ask: top.ask,
        });
    }

    fn check_triggers(&mut self, inst: InstrumentId, out: &mut Vec<Change>) {
        if self.triggers.get(&inst).is_none_or(|t| t.is_empty()) {
            return;
        }
        for _ in 0..MAX_CASCADE {
            let top = self.top_of(inst);
            let mut fired_any = false;

            while let Some(t) = self
                .triggers
                .get_mut(&inst)
                .and_then(|tr| tr.pop_fired(&top))
            {
                fired_any = true;

                // Where the market IS, not where the trigger WAS. A gap fills
                // here, never at the stale trigger price.
                let ref_price = top
                    .taker_price(t.side)
                    .expect("a trigger only fires when its reference exists");
                out.push(Change::TriggerFired {
                    id: t.id,
                    ref_price,
                    becomes: fired_as(&t),
                });

                if let Some(sib) = t.oco_with {
                    if let Some(tr) = self.triggers.get_mut(&inst) {
                        if tr.remove(sib).is_some() {
                            out.push(Change::TriggerCancelled {
                                id: sib,
                                reason: CancelReason::Oco,
                            });
                        }
                    }
                }
            }

            if !fired_any {
                return;
            }
            // Against a replica book, firing cannot move the price, so the
            // next pass finds nothing and returns. The loop exists for venue
            // cascades, where a fired stop trades and moves the book.
        }
        debug_assert!(false, "trigger cascade did not settle for {inst:?}");
    }

    /// Cancel every trigger attached to a position — TP/SL cleanup on close.
    /// Called by the app layer, which owns position lifecycle.
    pub fn cancel_attached(
        &mut self,
        inst: InstrumentId,
        pos: PositionId,
        out: &mut Vec<Change>,
    ) {
        let ids = match self.triggers.get(&inst) {
            Some(t) => t.attached_to(pos),
            None => return,
        };
        for id in ids {
            if let Some(t) = self.triggers.get_mut(&inst) {
                if t.remove(id).is_some() {
                    out.push(Change::TriggerCancelled {
                        id,
                        reason: CancelReason::PositionClosed,
                    });
                }
            }
        }
    }

    /// Every invariant in `docs/CORE.md` §8, across every book. Tests and
    /// fuzzers call this after each `apply`.
    pub fn check(&self) -> Result<(), String> {
        for ((p, i), b) in &self.level_books {
            b.check().map_err(|e| format!("LevelBook {p:?}/{i:?}: {e}"))?;
        }
        for (i, ob) in &self.order_books {
            ob.check().map_err(|e| format!("OrderBook {i:?}: {e}"))?;
        }
        for (i, t) in &self.triggers {
            let top = self.top_of(*i);
            t.check(&top).map_err(|e| format!("Triggers {i:?}: {e}"))?;
        }
        // book_keys must mirror level_books exactly.
        if self.book_keys.len() != self.level_books.len() {
            return Err(format!(
                "book_keys has {} entries, level_books has {}",
                self.book_keys.len(),
                self.level_books.len()
            ));
        }
        for w in self.book_keys.windows(2) {
            if w[0] >= w[1] {
                return Err("book_keys not sorted/deduped".into());
            }
        }
        Ok(())
    }
}
