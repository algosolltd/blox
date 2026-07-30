//! Provider replicas — aggregated depth, no order identity.
//! See `docs/CORE.md` §4.

use crate::types::*;

/// One side of a provider's book, always best-first.
///
/// Bids descend, asks ascend, so index 0 is the best price on both sides and
/// every "walk from the best" loop is direction-free.
///
/// `Vec` rather than `BTreeMap` because FX depth is shallow — typically 5-20
/// levels per side, which fits in a cache line or two and makes binary search
/// branch-predictable.
///
// ponytail: Vec is right up to a few hundred levels. Past ~1000, the memmove
// on a mid-book insert dominates — [`Repr`] below is the measured replacement,
// built and benchmarked but deliberately not wired in. See its docs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LevelBook {
    /// DESCENDING — best bid at \[0\].
    pub bids: Vec<(Ticks, Lots)>,
    /// ASCENDING — best ask at \[0\].
    pub asks: Vec<(Ticks, Lots)>,
    /// From the stamped event, never from the OS clock.
    pub last_update_ns: u64,
    pub stale: bool,
}

impl LevelBook {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub fn side(&self, s: Side) -> &[(Ticks, Lots)] {
        match s {
            Side::Bid => &self.bids,
            Side::Ask => &self.asks,
        }
    }

    #[inline]
    fn side_mut(&mut self, s: Side) -> &mut Vec<(Ticks, Lots)> {
        match s {
            Side::Bid => &mut self.bids,
            Side::Ask => &mut self.asks,
        }
    }

    /// Locate `price` in a best-first slice.
    ///
    /// Bids are descending so the comparator is reversed; asks are ascending
    /// so it is not.
    #[inline]
    fn find(v: &[(Ticks, Lots)], s: Side, price: Ticks) -> Result<usize, usize> {
        match s {
            Side::Bid => v.binary_search_by(|probe| price.cmp(&probe.0)),
            Side::Ask => v.binary_search_by(|probe| probe.0.cmp(&price)),
        }
    }

    /// Apply one level update. `qty == 0` removes the level.
    ///
    /// Returns whether anything actually changed. Providers resend identical
    /// levels constantly; suppressing no-ops here means they never reach a
    /// `Change`, never hit a conflation buffer, and never wake a subscriber.
    /// Cheapest filter, earliest point.
    pub fn apply_delta(&mut self, s: Side, price: Ticks, qty: Lots) -> bool {
        Self::sorted_delta(self.side_mut(s), s, price, qty)
    }

    /// The delta itself, against a best-first `Vec`. Split out only so the
    /// dormant [`Repr::Sparse`] shares it rather than growing a second copy.
    #[inline]
    fn sorted_delta(v: &mut Vec<(Ticks, Lots)>, s: Side, price: Ticks, qty: Lots) -> bool {
        match Self::find(v, s, price) {
            Ok(i) if qty <= 0 => {
                v.remove(i);
                true
            }
            Ok(i) => {
                if v[i].1 == qty {
                    return false;
                }
                v[i].1 = qty;
                true
            }
            // Delete of a level we never had: nothing to do.
            Err(_) if qty <= 0 => false,
            Err(i) => {
                v.insert(i, (price, qty));
                true
            }
        }
    }

    /// Wholesale replace. Used on connect and after a sequence gap — the only
    /// safe recovery when the delta stream has a hole in it.
    ///
    /// Adapters are trusted to deliver sorted, deduplicated, positive-quantity
    /// input; debug builds verify it, release builds take their word.
    pub fn replace(&mut self, bids: Vec<(Ticks, Lots)>, asks: Vec<(Ticks, Lots)>) {
        debug_assert!(
            bids.windows(2).all(|w| w[0].0 > w[1].0),
            "bids must be strictly descending"
        );
        debug_assert!(
            asks.windows(2).all(|w| w[0].0 < w[1].0),
            "asks must be strictly ascending"
        );
        debug_assert!(
            bids.iter().chain(asks.iter()).all(|l| l.1 > 0),
            "levels must have positive quantity"
        );
        self.bids = bids;
        self.asks = asks;
    }

    #[inline]
    pub fn best_bid(&self) -> Option<(Ticks, Lots)> {
        self.bids.first().copied()
    }

    #[inline]
    pub fn best_ask(&self) -> Option<(Ticks, Lots)> {
        self.asks.first().copied()
    }

    #[inline]
    pub fn top(&self) -> Top {
        Top {
            bid: self.best_bid(),
            ask: self.best_ask(),
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.bids.is_empty() && self.asks.is_empty()
    }

    /// Total quantity available to a taker on `side` at `limit` or better.
    /// `None` limit means "any price".
    pub fn available(&self, side: Side, limit: Option<Ticks>) -> Lots {
        self.side(side.opposite())
            .iter()
            .take_while(|(p, _)| limit.is_none_or(|l| side.crosses(l, *p)))
            .map(|(_, q)| q)
            .sum()
    }

    /// Volume-weighted price to take `qty` from this book, without consuming
    /// it. Returns `(vwap, filled)`.
    ///
    /// Truncation is rounded *against* the taker so it can never gift a better
    /// price than the book supported.
    pub fn vwap_for(&self, side: Side, qty: Lots, limit: Option<Ticks>) -> Option<(Ticks, Lots)> {
        if qty <= 0 {
            return None;
        }
        let mut remaining = qty;
        let mut cost: i128 = 0;
        for &(price, avail) in self.side(side.opposite()) {
            if let Some(l) = limit {
                if !side.crosses(l, price) {
                    break;
                }
            }
            let take = remaining.min(avail);
            cost += take as i128 * price as i128;
            remaining -= take;
            if remaining == 0 {
                break;
            }
        }
        let filled = qty - remaining;
        if filled == 0 {
            return None;
        }
        let f = filled as i128;
        // Round against the taker: up when buying, down when selling.
        let vwap = match side {
            Side::Bid => (cost + f - 1) / f,
            Side::Ask => cost / f,
        };
        Some((vwap as Ticks, filled))
    }

    /// Invariants from `docs/CORE.md` §8 (item 8). Returns the first breach.
    pub fn check(&self) -> Result<(), String> {
        for (name, v, descending) in [("bids", &self.bids, true), ("asks", &self.asks, false)] {
            for w in v.windows(2) {
                let ok = if descending { w[0].0 > w[1].0 } else { w[0].0 < w[1].0 };
                if !ok {
                    return Err(format!("{name} not sorted: {:?} then {:?}", w[0], w[1]));
                }
            }
            if let Some(bad) = v.iter().find(|l| l.1 <= 0) {
                return Err(format!("{name} has non-positive level {bad:?}"));
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Dormant: the bitmap side. Not used by `LevelBook`.
// ---------------------------------------------------------------------------
//
// Built, tested and benchmarked, then left switched off. Kept because the
// measurement is the expensive part and re-deriving it later would cost more
// than carrying the code.
//
// **Why it is off.** At the depth this engine actually runs — `docs/CORE.md`
// §4 and `docs/BENCHMARKS.md` §5 both say FX books are 5-20 levels — the
// bitmap is a net loss:
//
// | operation                       | depth |    Vec | bitmap |       |
// |---------------------------------|------:|-------:|-------:|-------|
// | delta, existing level           |  2000 |   10.7 |    2.4 |  −78% |
// | delta, remove + insert level    |  2000 |   61.3 |    2.6 |  −96% |
// | available, half the book        |  2000 |  236.5 | 2238.4 | +847% |
// | delta, existing level           |    20 |    5.0 |    6.3 |  +26% |
// | available, half the book        |    20 |    2.5 |    2.4 |   −4% |
//
// Writes go 10-40x faster once a side is deep; ordered walks go ~9x slower at
// every depth, because a masked word scan cannot touch contiguous memory. Deep
// books win, shallow books lose. Wiring it in behind a depth threshold *still*
// cost 5% on `delta` and 11% on `aggregate` in the engine benchmarks, because
// handing `side()` back as an enum iterator costs `agg_side` its specialised
// `Vec::extend`. Hence: off.
//
// **When to switch it on.** A side that habitually holds ≥64 levels within a
// few thousand ticks — a crypto venue book, or an FX aggregate over many
// providers deep. Then: hold `Repr` in `LevelBook` instead of the two `Vec`s,
// change `side()` to return `Repr::iter()`, and update `agg_side` in
// `engine.rs` to drop its `.iter()`. `Repr::apply_delta` already handles
// promotion, rewindowing and the sparse fallback.
//
// Adapted from the `PriceLevelArray` in <https://github.com/bozoslav/order-book>,
// minus its two defects: that one clamps out-of-range prices into the nearest
// slot (which is how a $50 bid fills at $100) and fixes its window at compile
// time.

/// Widest tick span held densely. Beyond this a side falls back to the sorted
/// `Vec`.
///
/// The dense array is indexed by tick, so its memory is proportional to the
/// *span* of the book, not the number of levels in it. Adapter input is
/// untrusted — `parse_levels` accepts any `i64` — so an unbounded window is a
/// remote allocation primitive. 4096 ticks costs 32 KiB of quantities and
/// 512 B of bitmap per side.
const DENSE_MAX_TICKS: usize = 4096;

/// Smallest dense window. Below this the headroom is not worth the reshape
/// churn as the mid drifts.
const DENSE_MIN_TICKS: usize = 256;

/// Levels a side must hold before the bitmap earns its read cost.
///
// ponytail: promote-only, no demotion. A side that grows past this and then
// shrinks stays dense — shallow and slightly slower to walk, never wrong.
// Demoting would mean tracking an occupied count on every set/clear to buy
// back a few ns on a book that already proved it gets deep.
const DENSE_MIN_LEVELS: usize = 64;

/// Sentinel for "no occupied slot". Unreachable as an index because the window
/// is capped at `DENSE_MAX_TICKS`.
const NONE: usize = usize::MAX;

/// One side of a book, in whichever shape suits its current depth and span.
#[allow(dead_code)]
#[derive(Clone, Debug)]
enum Repr {
    /// Best-first, no duplicate prices, all quantities positive. What
    /// `LevelBook` uses today.
    Sparse(Vec<(Ticks, Lots)>),
    Dense(Dense),
}

impl Default for Repr {
    fn default() -> Self {
        Repr::Sparse(Vec::new())
    }
}

/// A quantity per tick over a bounded window, with a bitmap of which ticks are
/// occupied.
///
/// Slot 0 is always the *best* price, on both sides: `step` is +1 for asks and
/// -1 for bids, so walking slots upward walks prices away from the touch. That
/// keeps every read direction-free, exactly as the sorted `Vec` does.
#[allow(dead_code)]
#[derive(Clone, Debug)]
struct Dense {
    /// Price of slot 0.
    base: Ticks,
    /// +1 for asks (ascending), -1 for bids (descending).
    step: Ticks,
    qty: Vec<Lots>,
    bits: Vec<u64>,
    /// Lowest occupied slot, i.e. the best price. `NONE` when empty.
    best: usize,
}

#[allow(dead_code)]
impl Dense {
    /// Slot holding `price`, or `None` if it falls outside the window.
    ///
    /// Checked throughout: a price outside the window must rewindow, never
    /// clamp. Clamping is how you fill a $50 bid at $100.
    #[inline]
    fn slot(&self, price: Ticks) -> Option<usize> {
        let d = price.checked_sub(self.base)?;
        let d = if self.step < 0 { d.checked_neg()? } else { d };
        (d >= 0 && (d as usize) < self.qty.len()).then_some(d as usize)
    }

    #[inline]
    fn price_at(&self, slot: usize) -> Ticks {
        self.base + slot as Ticks * self.step
    }

    /// Lowest occupied slot at or after `from`.
    #[inline]
    fn next_from(&self, from: usize) -> usize {
        if from >= self.qty.len() {
            return NONE;
        }
        let mut w = self.bits[from / 64] & (!0u64 << (from % 64));
        let mut c = from / 64;
        loop {
            if w != 0 {
                return c * 64 + w.trailing_zeros() as usize;
            }
            c += 1;
            if c >= self.bits.len() {
                return NONE;
            }
            w = self.bits[c];
        }
    }

    /// Returns whether anything changed. `qty <= 0` empties the slot.
    #[inline]
    fn set(&mut self, slot: usize, qty: Lots) -> bool {
        let new = qty.max(0);
        if self.qty[slot] == new {
            return false;
        }
        self.qty[slot] = new;
        if new == 0 {
            self.bits[slot / 64] &= !(1u64 << (slot % 64));
            if slot == self.best {
                self.best = self.next_from(slot + 1);
            }
        } else {
            self.bits[slot / 64] |= 1u64 << (slot % 64);
            if slot < self.best {
                self.best = slot;
            }
        }
        true
    }
}

#[allow(dead_code)]
impl Repr {
    /// Build the representation for `levels` (best-first, positive, deduped),
    /// sized to also admit `extra` if given.
    ///
    /// Falls back to `Sparse` whenever the side is too shallow to profit, the
    /// span will not fit the cap, or the tick arithmetic would overflow.
    fn build(side: Side, levels: Vec<(Ticks, Lots)>, extra: Option<Ticks>) -> Repr {
        if levels.len() < DENSE_MIN_LEVELS {
            return Repr::Sparse(levels);
        }
        let prices = levels.iter().map(|l| l.0).chain(extra);
        let (Some(lo), Some(hi)) = (prices.clone().min(), prices.max()) else {
            return Repr::Sparse(levels);
        };

        let Some(span) = hi.checked_sub(lo).and_then(|s| s.checked_add(1)) else {
            return Repr::Sparse(levels);
        };
        if span > DENSE_MAX_TICKS as Ticks {
            return Repr::Sparse(levels);
        }
        let span = span as usize;

        // Headroom on both sides so an ordinary drift in the mid does not
        // rewindow on the next delta.
        let width = (span * 2).clamp(DENSE_MIN_TICKS, DENSE_MAX_TICKS);
        let pad = ((width - span) / 2) as Ticks;

        // Slot 0 is the best price: the high end for bids, the low end for asks.
        let (base, step) = match side {
            Side::Bid => (hi.checked_add(pad), -1),
            Side::Ask => (lo.checked_sub(pad), 1),
        };
        let Some(base) = base else {
            return Repr::Sparse(levels);
        };

        let mut d = Dense {
            base,
            step,
            qty: vec![0; width],
            bits: vec![0; width.div_ceil(64)],
            best: NONE,
        };
        for (price, qty) in levels {
            let slot = d.slot(price).expect("built window must admit its levels");
            d.set(slot, qty);
        }
        Repr::Dense(d)
    }

    /// Same contract as [`LevelBook::apply_delta`], including the suppression
    /// of no-op updates, plus promotion and rewindowing.
    fn apply_delta(&mut self, s: Side, price: Ticks, qty: Lots) -> bool {
        match self {
            Repr::Dense(d) => {
                if let Some(slot) = d.slot(price) {
                    return d.set(slot, qty);
                }
                // Outside the window. A delete has nothing to delete; anything
                // else needs a wider window, or none at all.
                if qty <= 0 {
                    return false;
                }
            }
            Repr::Sparse(v) => {
                let changed = LevelBook::sorted_delta(v, s, price, qty);
                if !changed || !Self::promotable(v) {
                    return changed;
                }
                let levels = std::mem::take(v);
                *self = Repr::build(s, levels, None);
                return true;
            }
        }

        let levels: Vec<(Ticks, Lots)> = self.iter().collect();
        *self = Repr::build(s, levels, Some(price));

        match self {
            Repr::Dense(d) => {
                let slot = d.slot(price).expect("rewindowed span must admit price");
                d.set(slot, qty)
            }
            Repr::Sparse(v) => LevelBook::sorted_delta(v, s, price, qty),
        }
    }

    /// Is this sparse side worth holding densely?
    ///
    /// It is sorted best-first, so its two ends *are* its extremes and the
    /// span is O(1) to read — no scan on the delta path.
    ///
    /// Promotes at half the cap but only demotes past the full cap, so a book
    /// hovering at the boundary does not rebuild itself on alternate deltas.
    fn promotable(v: &[(Ticks, Lots)]) -> bool {
        if v.len() < DENSE_MIN_LEVELS {
            return false;
        }
        let (Some(a), Some(b)) = (v.first(), v.last()) else {
            return false;
        };
        a.0.checked_sub(b.0)
            .is_some_and(|d| d.unsigned_abs() < (DENSE_MAX_TICKS / 2) as u64)
    }

    fn iter(&self) -> ReprIter<'_> {
        match self {
            Repr::Sparse(v) => ReprIter::Sparse(v.iter()),
            Repr::Dense(d) => ReprIter::Dense { d, next: d.best },
        }
    }

    fn best(&self) -> Option<(Ticks, Lots)> {
        match self {
            Repr::Sparse(v) => v.first().copied(),
            Repr::Dense(d) => (d.best != NONE).then(|| (d.price_at(d.best), d.qty[d.best])),
        }
    }

    fn is_empty(&self) -> bool {
        match self {
            Repr::Sparse(v) => v.is_empty(),
            Repr::Dense(d) => d.best == NONE,
        }
    }

    /// `LevelBook::check`'s invariants, plus the two the bitmap adds: it must
    /// agree with the quantity array, and the cached best must be the real
    /// lowest occupied slot. Nothing above can observe a breach of either,
    /// which is exactly why they need asserting.
    fn check(&self, side: Side) -> Result<(), String> {
        let descending = side == Side::Bid;
        let mut prev: Option<(Ticks, Lots)> = None;
        for l in self.iter() {
            if l.1 <= 0 {
                return Err(format!("non-positive level {l:?}"));
            }
            if let Some(p) = prev {
                let ok = if descending { p.0 > l.0 } else { p.0 < l.0 };
                if !ok {
                    return Err(format!("not sorted: {p:?} then {l:?}"));
                }
            }
            prev = Some(l);
        }

        if let Repr::Dense(d) = self {
            for (slot, &q) in d.qty.iter().enumerate() {
                let occupied = d.bits[slot / 64] & (1u64 << (slot % 64)) != 0;
                if occupied != (q > 0) {
                    return Err(format!(
                        "slot {slot} @ {}: bitmap says {occupied}, qty is {q}",
                        d.price_at(slot)
                    ));
                }
            }
            let best = d.next_from(0);
            if best != d.best {
                return Err(format!("best slot is {best}, cached {}", d.best));
            }
        }
        Ok(())
    }
}

#[allow(dead_code)]
enum ReprIter<'a> {
    Sparse(std::slice::Iter<'a, (Ticks, Lots)>),
    Dense { d: &'a Dense, next: usize },
}

impl Iterator for ReprIter<'_> {
    type Item = (Ticks, Lots);

    #[inline]
    fn next(&mut self) -> Option<(Ticks, Lots)> {
        match self {
            ReprIter::Sparse(it) => it.next().copied(),
            ReprIter::Dense { d, next } => {
                if *next == NONE {
                    return None;
                }
                let slot = *next;
                *next = d.next_from(slot + 1);
                Some((d.price_at(slot), d.qty[slot]))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book() -> LevelBook {
        let mut b = LevelBook::new();
        b.replace(
            vec![(100, 10), (99, 20), (98, 30)],
            vec![(101, 15), (102, 25), (103, 35)],
        );
        b
    }

    #[test]
    fn best_is_always_index_zero() {
        let b = book();
        assert_eq!(b.best_bid(), Some((100, 10)));
        assert_eq!(b.best_ask(), Some((101, 15)));
    }

    #[test]
    fn delta_inserts_in_sorted_position() {
        let mut b = book();
        assert!(b.apply_delta(Side::Bid, 99, 50));
        assert_eq!(b.bids, vec![(100, 10), (99, 50), (98, 30)]);

        // A new best bid lands at the front.
        assert!(b.apply_delta(Side::Bid, 105, 7));
        assert_eq!(b.bids[0], (105, 7));

        // A new ask between two existing levels lands between them.
        let mut c = LevelBook::new();
        c.replace(vec![], vec![(101, 15), (103, 35)]);
        assert!(c.apply_delta(Side::Ask, 102, 9));
        assert_eq!(c.asks, vec![(101, 15), (102, 9), (103, 35)]);
        c.check().unwrap();
        b.check().unwrap();
    }

    #[test]
    fn identical_delta_is_suppressed() {
        let mut b = book();
        // Same quantity at an existing level: no change, no Change emitted.
        assert!(!b.apply_delta(Side::Bid, 100, 10));
        // Deleting a level we never had: also no change.
        assert!(!b.apply_delta(Side::Bid, 42, 0));
    }

    #[test]
    fn zero_qty_removes_the_level() {
        let mut b = book();
        assert!(b.apply_delta(Side::Ask, 102, 0));
        assert_eq!(b.asks, vec![(101, 15), (103, 35)]);
        b.check().unwrap();
    }

    #[test]
    fn available_respects_the_limit() {
        let b = book();
        // Buying: walks asks 101,102,103.
        assert_eq!(b.available(Side::Bid, Some(102)), 40); // 15 + 25
        assert_eq!(b.available(Side::Bid, None), 75);
        // Selling: walks bids 100,99,98.
        assert_eq!(b.available(Side::Ask, Some(99)), 30); // 10 + 20
    }

    #[test]
    fn vwap_walks_levels_and_rounds_against_the_taker() {
        let b = book();
        // Buy 20: 15 @ 101 + 5 @ 102 = 1515 + 510 = 2025 / 20 = 101.25 -> 102
        // (rounded up, against the buyer).
        assert_eq!(b.vwap_for(Side::Bid, 20, None), Some((102, 20)));
        // Sell 20: 10 @ 100 + 10 @ 99 = 1990 / 20 = 99.5 -> 99 (rounded down,
        // against the seller).
        assert_eq!(b.vwap_for(Side::Ask, 20, None), Some((99, 20)));
    }

    #[test]
    fn vwap_reports_partial_fills_rather_than_inventing_liquidity() {
        let b = book();
        // Only 75 lots on the ask side; asking for 200 fills 75.
        let (_, filled) = b.vwap_for(Side::Bid, 200, None).unwrap();
        assert_eq!(filled, 75);
        // Limit below the book: nothing fills.
        assert_eq!(b.vwap_for(Side::Bid, 10, Some(100)), None);
    }

    // ---- the dormant bitmap ------------------------------------------------
    //
    // `Repr` is not wired into `LevelBook`. These tests are what keeps it
    // compiling and correct, so switching it on later is a decision rather
    // than a rewrite.

    fn is_dense(r: &Repr) -> bool {
        matches!(r, Repr::Dense(_))
    }

    fn levels(r: &Repr) -> Vec<(Ticks, Lots)> {
        r.iter().collect()
    }

    /// Deep enough to promote, narrow enough to fit the window.
    fn deep(s: Side) -> Repr {
        let n = DENSE_MIN_LEVELS as Ticks + 16;
        let r = Repr::build(
            s,
            (0..n)
                .map(|i| match s {
                    Side::Bid => (100_000 - i, 10 + i),
                    Side::Ask => (100_000 + i, 10 + i),
                })
                .collect(),
            None,
        );
        assert!(is_dense(&r));
        r
    }

    #[test]
    fn depth_and_span_decide_the_representation() {
        // Shallow: the Vec walks contiguous memory faster than a bitmap scan,
        // and FX books live here.
        assert!(!is_dense(&Repr::build(
            Side::Bid,
            vec![(100, 10), (99, 20)],
            None
        )));

        // Deep and narrow: the memmove has real length to move.
        deep(Side::Bid);
        deep(Side::Ask);

        // Deep but wider than the cap: sparse at any depth, because the dense
        // array costs memory per *tick*, not per level.
        let n = DENSE_MIN_LEVELS as Ticks + 16;
        let wide = Repr::build(
            Side::Bid,
            (0..n).map(|i| (1_000_000 - i * 1000, 5)).collect(),
            None,
        );
        assert!(!is_dense(&wide));
        assert_eq!(wide.best(), Some((1_000_000, 5)));
        wide.check(Side::Bid).unwrap();
    }

    /// The defect this design exists to avoid: the C++ original clamps a price
    /// outside its fixed window into the nearest slot, so a bid at $50 rests —
    /// and fills — at $100.
    #[test]
    fn a_price_outside_the_window_widens_it_and_is_never_clamped() {
        let mut r = deep(Side::Bid);
        let worst = levels(&r).last().unwrap().0;

        // Far below the window, but the resulting span still fits the cap:
        // rewindow, keep every level at its real price.
        assert!(r.apply_delta(Side::Bid, worst - 900, 7));
        assert!(is_dense(&r));
        assert_eq!(r.iter().last(), Some((worst - 900, 7)));
        assert_eq!(r.best(), Some((100_000, 10)));
        r.check(Side::Bid).unwrap();

        // Beyond the cap: falls back to sparse, still at the real price.
        assert!(r.apply_delta(Side::Bid, worst - 90_000, 9));
        assert!(!is_dense(&r));
        assert_eq!(r.iter().last(), Some((worst - 90_000, 9)));
        assert_eq!(r.best(), Some((100_000, 10)));
        r.check(Side::Bid).unwrap();
    }

    /// A remote adapter can name any `i64` — `parse_levels` does not bound it.
    /// Neither the window nor the tick arithmetic may blow up on the extremes.
    #[test]
    fn extreme_ticks_do_not_allocate_or_overflow() {
        let mut r = deep(Side::Bid);
        assert!(r.apply_delta(Side::Bid, Ticks::MAX, 1));
        assert!(!is_dense(&r), "span overflows, must not go dense");
        assert!(r.apply_delta(Side::Bid, Ticks::MIN, 1));
        assert_eq!(r.best(), Some((Ticks::MAX, 1)));
        assert_eq!(r.iter().last(), Some((Ticks::MIN, 1)));
        r.check(Side::Bid).unwrap();

        let mut a = deep(Side::Ask);
        assert!(a.apply_delta(Side::Ask, Ticks::MIN, 1));
        assert!(a.apply_delta(Side::Ask, Ticks::MAX, 1));
        assert_eq!(a.best(), Some((Ticks::MIN, 1)));
        assert_eq!(a.iter().last(), Some((Ticks::MAX, 1)));
        a.check(Side::Ask).unwrap();
    }

    /// The bitmap must be indistinguishable from the sorted `Vec` it replaces
    /// — including the change flag, which drives `Change` suppression.
    #[test]
    fn dense_agrees_with_a_sorted_model_through_promotion_and_churn() {
        // Deterministic LCG; the core forbids `rand`.
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut next = move || {
            seed ^= seed >> 12;
            seed ^= seed << 25;
            seed ^= seed >> 27;
            seed.wrapping_mul(0x2545_F491_4F6C_DD1D)
        };

        // Starts sparse and empty, so this covers the promotion crossing too.
        let mut r = Repr::default();
        let mut model: Vec<(Ticks, Lots)> = Vec::new();

        for _ in 0..20_000 {
            let price = 100_000 + (next() % 600) as Ticks;
            let qty = (next() % 4) as Lots * 10; // 0 a quarter of the time
            let changed = r.apply_delta(Side::Bid, price, qty);

            let before = model.clone();
            match model.binary_search_by(|p| price.cmp(&p.0)) {
                Ok(i) if qty <= 0 => {
                    model.remove(i);
                }
                Ok(i) => model[i].1 = qty,
                Err(_) if qty <= 0 => {}
                Err(i) => model.insert(i, (price, qty)),
            }
            assert_eq!(changed, before != model, "change flag disagrees @ {price}");
            assert_eq!(levels(&r), model, "book diverged @ {price}");
        }
        assert!(is_dense(&r), "600 ticks over 64+ levels should promote");
        r.check(Side::Bid).unwrap();
    }

    #[test]
    fn draining_a_dense_side_empties_it_cleanly() {
        let mut r = deep(Side::Bid);
        for (p, _) in levels(&r) {
            assert!(r.apply_delta(Side::Bid, p, 0));
        }
        assert_eq!(r.best(), None);
        assert_eq!(levels(&r), vec![]);
        assert!(r.is_empty());
        r.check(Side::Bid).unwrap();

        // And refills without a stale `best`.
        assert!(r.apply_delta(Side::Bid, 99_997, 4));
        assert_eq!(r.best(), Some((99_997, 4)));
        r.check(Side::Bid).unwrap();
    }
}
