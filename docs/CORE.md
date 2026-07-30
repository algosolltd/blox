# blox-core — Implementation Spec

The pure engine. No I/O, no clock, no threads, no randomness, no floats.
Everything here is a function of its inputs.

Read `DESIGN.md` first for *why*. This document is *how*.

**Dependencies: none beyond `std`.** If you find yourself adding one, that's a
signal the thing you're adding belongs in `blox-server`.

---

## 1. Primitive types

```rust
pub type Ticks   = i64;   // price, in canonical ticks (D10/D16)
pub type Lots    = i64;   // quantity, in canonical lots
pub type OrderId = u64;   // caller-assigned, unique per book
pub type OwnerId = u32;   // for self-trade prevention and attribution
pub type Seq     = u64;   // sequencer stamp

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Side { Bid, Ask }

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct InstrumentId(pub u16);

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct ProviderId(pub u16);
```

`InstrumentId` and `ProviderId` are small integers, not strings. String symbols
exist only in the registry and at the API edges. Interning them at the adapter
boundary means the hot path compares two `u16`s instead of two `&str`s, and it
makes every map key `Copy`.

```rust
impl Side {
    #[inline]
    pub fn opposite(self) -> Side {
        match self { Side::Bid => Side::Ask, Side::Ask => Side::Bid }
    }

    /// Does `limit` cross a resting order at `resting`?
    /// Bid crosses when willing to pay at least the ask.
    #[inline]
    pub fn crosses(self, limit: Ticks, resting: Ticks) -> bool {
        match self { Side::Bid => limit >= resting, Side::Ask => limit <= resting }
    }
}
```

### Why no floats, anywhere

Restating D10 because it's the invariant most likely to be violated by accident:
a single `as f64` in a conversion helper reintroduces every problem we designed
around. There is no float in `blox-core`. Not in prices, not in quantities, not
in P&L, not in a convenience `to_decimal()` helper. Rendering to a decimal
string is a *server* concern and it does it with integer division.

---

## 2. Events — the input vocabulary

Two vocabularies (D11), plus one engine-level event.

```rust
#[derive(Clone, Debug)]
pub enum Event {
    // ---- to an OrderBook: we own it, orders have identity ----
    New    { instrument: InstrumentId, id: OrderId, owner: OwnerId,
             side: Side, price: Ticks, qty: Lots, kind: OrderKind },
    Cancel { instrument: InstrumentId, id: OrderId },
    Amend  { instrument: InstrumentId, id: OrderId, qty: Lots },

    // ---- to a LevelBook: the provider owns it, no identity ----
    Snapshot { provider: ProviderId, instrument: InstrumentId,
               bids: Vec<(Ticks, Lots)>, asks: Vec<(Ticks, Lots)> },
    Delta    { provider: ProviderId, instrument: InstrumentId,
               side: Side, price: Ticks, qty: Lots },   // qty == 0 removes
    Clear    { provider: ProviderId },                  // provider disconnected

    // ---- engine-level ----
    Tick { ts_nanos: u64 },   // the only source of time (D4)
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum OrderKind {
    Limit,      // rest the remainder
    Market,     // no price limit; drop the remainder
    Ioc,        // immediate-or-cancel: take what's there, drop the rest
    Fok,        // fill-or-kill: all or nothing, pre-checked
    PostOnly,   // reject entirely if it would cross
}
```

### On `Clear`

`Clear { provider }` drops **every book owned by that provider, across all
instruments**, in one operation. This is the payoff for D2 (one book per
`(provider, instrument)`) and it is the single most important operation in a
multi-provider system: when an LP's connection dies, their liquidity must
vanish atomically. If you had chosen one book per instrument with provider tags,
this would be a filtered scan of every level of every book while matching is
live — and the window where it's half-done is a window where you route to
liquidity that isn't there.

### On `Tick`

The core never calls `Instant::now()`. Time arrives as an event, stamped by the
sequencer in `blox-server`. Everything time-dependent — staleness marking,
expiry — is driven by `Tick`.

This is what makes replay produce *identical* output rather than merely similar
output. Recorded ticks replay at whatever speed you like, and eviction happens
at exactly the same points in the event stream.

---

## 3. Changes — the output vocabulary

`apply` **returns** what changed (D8). It never calls a callback, never holds a
listener list, never knows a consumer exists.

```rust
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    // book mutations
    LevelUpdated { provider: ProviderId, instrument: InstrumentId,
                   side: Side, price: Ticks, qty: Lots },
    BookReplaced { provider: ProviderId, instrument: InstrumentId },
    BookCleared  { provider: ProviderId, instrument: InstrumentId },
    BookStale    { provider: ProviderId, instrument: InstrumentId, age_nanos: u64 },

    // top of book, per instrument, after aggregation
    TopOfBook { instrument: InstrumentId,
                bid: Option<(Ticks, Lots)>, ask: Option<(Ticks, Lots)> },

    // order lifecycle (OrderBook only)
    Accepted  { id: OrderId },
    Rejected  { id: OrderId, reason: RejectReason },
    Filled    { id: OrderId, price: Ticks, qty: Lots, remaining: Lots },
    Cancelled { id: OrderId, remaining: Lots },

    // a match between two orders
    Trade { instrument: InstrumentId, price: Ticks, qty: Lots,
            maker: OrderId, taker: OrderId, aggressor: Side },
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum RejectReason {
    UnknownOrder,
    DuplicateId,
    ZeroQty,
    NegativeQty,
    WouldCross,        // PostOnly
    InsufficientBook,  // Fok
    SelfTrade,
    NoLiquidity,       // Market against an empty book
}
```

### Allocation on the hot path

`apply` returning `Vec<Change>` allocates on every call. At game speeds that is
invisible; at 50k events/sec it is not. So the real signature takes a reusable
buffer, and the `Vec`-returning form is a convenience wrapper:

```rust
impl Engine {
    /// Primary form. Appends to `out`; caller reuses the buffer across calls.
    pub fn apply_into(&mut self, e: &Event, out: &mut Vec<Change>) { .. }

    /// Convenience. Allocates.
    pub fn apply(&mut self, e: &Event) -> Vec<Change> {
        let mut out = Vec::new();
        self.apply_into(e, &mut out);
        out
    }

    /// D9 — one FFI crossing for many events. Essential for backtests.
    pub fn apply_batch(&mut self, events: &[Event], out: &mut Vec<Change>) {
        for e in events { self.apply_into(e, out); }
    }
}
```

`apply_batch` is what makes replaying a year of ticks from Python viable: one
boundary crossing instead of ten million.

---

## 4. `LevelBook` — provider replicas

Aggregated depth. No order identity, because the provider never gave you any.

```rust
pub struct LevelBook {
    pub bids: Vec<(Ticks, Lots)>,   // DESCENDING — best at [0]
    pub asks: Vec<(Ticks, Lots)>,   // ASCENDING  — best at [0]
    pub last_update_ns: u64,        // from the stamped event
    pub stale: bool,
}
```

**Both sides keep the best price at index 0.** Storing bids descending and asks
ascending means every "walk from the best" loop is `for level in book.side(s)`
with no direction branch — the same code path serves both sides. It also makes
`best()` a constant-time `.first()`.

```rust
impl LevelBook {
    #[inline]
    fn side_mut(&mut self, s: Side) -> &mut Vec<(Ticks, Lots)> {
        match s { Side::Bid => &mut self.bids, Side::Ask => &mut self.asks }
    }

    /// Comparator that yields "best first" for the given side.
    #[inline]
    fn find(v: &[(Ticks, Lots)], s: Side, price: Ticks) -> Result<usize, usize> {
        match s {
            Side::Bid => v.binary_search_by(|p| price.cmp(&p.0)),  // reversed
            Side::Ask => v.binary_search_by(|p| p.0.cmp(&price)),
        }
    }

    pub fn apply_delta(&mut self, s: Side, price: Ticks, qty: Lots) -> bool {
        let v = self.side_mut(s);
        match Self::find(v, s, price) {
            Ok(i) if qty == 0 => { v.remove(i); true }
            Ok(i)             => { if v[i].1 == qty { return false } v[i].1 = qty; true }
            Err(_) if qty == 0 => false,          // delete of a level we don't have
            Err(i)             => { v.insert(i, (price, qty)); true }
        }
    }

    pub fn replace(&mut self, bids: Vec<(Ticks, Lots)>, asks: Vec<(Ticks, Lots)>) {
        // Adapters are trusted to deliver sorted, deduplicated input.
        // Debug builds verify it; release builds take their word.
        debug_assert!(bids.windows(2).all(|w| w[0].0 > w[1].0), "bids not descending");
        debug_assert!(asks.windows(2).all(|w| w[0].0 < w[1].0), "asks not ascending");
        self.bids = bids;
        self.asks = asks;
    }

    #[inline] pub fn best_bid(&self) -> Option<(Ticks, Lots)> { self.bids.first().copied() }
    #[inline] pub fn best_ask(&self) -> Option<(Ticks, Lots)> { self.asks.first().copied() }
}
```

`apply_delta` returns whether anything actually changed. Providers resend
identical levels constantly; suppressing no-op updates here means they never
reach a `Change`, never hit the conflation buffer, and never wake a subscriber.
Cheapest possible filter, applied at the earliest possible point.

**Why `Vec` and not `BTreeMap`:** FX depth is shallow — typically 5–20 levels
per side. At that size a contiguous `Vec` beats a `BTreeMap` decisively: the
whole book fits in one or two cache lines, binary search is branch-predictable,
and the `memmove` on insert is a handful of nanoseconds.

```
// ponytail: Vec is right up to a few hundred levels. Past ~1000, the memmove
// on mid-book insert dominates — switch to BTreeMap<Ticks, Lots> then, keeping
// the same find/apply_delta signatures so nothing above notices.
```

---

## 5. `OrderBook` — authoritative books

Real orders, real identity, price-time priority. Used by `provider = internal`
and by every game.

### Structure

The two operations that must be fast are **match** (walk the best level in
arrival order) and **cancel by id** (~90% of orders in any real book are
cancelled, not filled). That points at an arena with an intrusive doubly-linked
list per price level.

```rust
const NIL: u32 = u32::MAX;

pub struct OrderBook {
    bids: VecDeque<Level>,          // DESCENDING — best at [0]
    asks: VecDeque<Level>,          // ASCENDING  — best at [0]
    slots: Vec<Slot>,               // arena
    free: u32,                      // head of the free list
    index: HashMap<OrderId, u32>,   // id -> arena slot
    stp: StpMode,
}

#[derive(Clone, Copy)]
struct Level {
    price: Ticks,
    total: Lots,   // == sum of qty of orders in the list; asserted in debug
    head: u32,     // oldest — fills first
    tail: u32,     // newest
}

#[derive(Clone, Copy)]
struct Slot {
    id:    OrderId,
    owner: OwnerId,
    price: Ticks,   // so cancel can locate its level without a back-pointer
    side:  Side,
    qty:   Lots,
    prev:  u32,
    next:  u32,     // doubles as the free-list link when the slot is free
}
```

**The `price` field on `Slot` is deliberate.** The obvious alternative — storing
a `level: u32` index — is wrong, because inserting or removing a level shifts
every index after it and silently corrupts every order below. Storing the price
and doing a `binary_search` on cancel costs `O(log L)` with `L` around 20, i.e.
four or five comparisons on hot cache lines. That is not a bottleneck, and it
removes a whole category of index-invalidation bug that would otherwise surface
as "an order occasionally cancels the wrong order."

**`VecDeque` rather than `Vec`** for the levels: matching consumes levels from
the front, and `pop_front` is `O(1)` on a deque versus `O(L)` on a `Vec`. Mid-book
insert is `O(L)` either way. `VecDeque::binary_search_by` exists, so the lookup
code is unchanged.

### Arena management

```rust
impl OrderBook {
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
        self.slots[i as usize].next = self.free;
        self.free = i;
    }
}
```

Slots are recycled, so a long-running book reaches a steady state and stops
allocating entirely. That matters for D4 as much as for speed: a book that
doesn't allocate can't produce a latency spike that depends on the allocator's
mood, which is one fewer source of run-to-run variation.

### Unlink — the one function to get right

```rust
impl OrderBook {
    /// Remove slot `i` from its level's list. Returns the level's index,
    /// or None if the level became empty and was removed.
    fn unlink(&mut self, i: u32) -> Option<usize> {
        let (price, side, qty) = {
            let s = &self.slots[i as usize];
            (s.price, s.side, s.qty)
        };
        let levels = match side { Side::Bid => &mut self.bids, Side::Ask => &mut self.asks };
        let li = Self::find_level(levels, side, price).expect("order's level must exist");

        let (prev, next) = { let s = &self.slots[i as usize]; (s.prev, s.next) };
        if prev != NIL { self.slots[prev as usize].next = next; }
        if next != NIL { self.slots[next as usize].prev = prev; }

        let lvl = &mut levels[li];
        if lvl.head == i { lvl.head = next; }
        if lvl.tail == i { lvl.tail = prev; }
        lvl.total -= qty;

        if lvl.head == NIL {
            debug_assert_eq!(lvl.total, 0, "empty level with non-zero total");
            levels.remove(li);
            None
        } else {
            Some(li)
        }
    }
}
```

Every removal path — cancel, full fill, self-trade eviction — routes through
`unlink`. There is exactly one place that knows how to detach an order, so
there is exactly one place that can get the `total` bookkeeping wrong, and one
`debug_assert` guarding it.

### Matching

Price-time priority. The aggressor walks the opposite side from the best price
outward, taking each level's orders in arrival order.

```rust
impl OrderBook {
    /// Take liquidity from the opposite side. Returns the unfilled remainder.
    fn take(&mut self, side: Side, limit: Option<Ticks>, mut qty: Lots,
            taker: OrderId, owner: OwnerId, inst: InstrumentId,
            out: &mut Vec<Change>) -> Lots
    {
        loop {
            if qty == 0 { return 0; }

            let levels = match side.opposite() {
                Side::Bid => &self.bids, Side::Ask => &self.asks
            };
            let Some(lvl) = levels.front() else { return qty };      // book empty

            // A Market order has no limit and crosses everything.
            if let Some(l) = limit {
                if !side.crosses(l, lvl.price) { return qty; }        // no longer crossing
            }
            let price = lvl.price;   // MAKER's price — see below
            let mut head = lvl.head;

            while head != NIL && qty > 0 {
                let m = &self.slots[head as usize];
                let (maker_id, maker_owner, maker_qty) = (m.id, m.owner, m.qty);
                let next = m.next;

                if self.stp != StpMode::None && maker_owner == owner {
                    match self.stp {
                        StpMode::CancelResting => {
                            self.unlink(head);
                            self.index.remove(&maker_id);
                            self.dealloc(head);
                            out.push(Change::Cancelled { id: maker_id, remaining: maker_qty });
                            head = next;
                            continue;
                        }
                        StpMode::CancelIncoming => {
                            out.push(Change::Rejected { id: taker, reason: RejectReason::SelfTrade });
                            return qty;
                        }
                        StpMode::None => unreachable!(),
                    }
                }

                let fill = qty.min(maker_qty);
                qty -= fill;

                out.push(Change::Trade {
                    instrument: inst, price, qty: fill,
                    maker: maker_id, taker: taker_id_of(taker), aggressor: side,
                });
                out.push(Change::Filled {
                    id: maker_id, price, qty: fill, remaining: maker_qty - fill,
                });

                if fill == maker_qty {
                    self.unlink(head);
                    self.index.remove(&maker_id);
                    self.dealloc(head);
                } else {
                    self.slots[head as usize].qty -= fill;
                    // adjust the level total in place
                    let levels = match side.opposite() {
                        Side::Bid => &mut self.bids, Side::Ask => &mut self.asks
                    };
                    levels.front_mut().unwrap().total -= fill;
                }
                head = next;
            }

            // If the level survived with orders remaining, and we still want more,
            // the only reason is qty == 0 — otherwise we'd have drained it.
            let levels = match side.opposite() {
                Side::Bid => &self.bids, Side::Ask => &self.asks
            };
            if levels.front().map_or(false, |l| l.head != NIL) && qty > 0 {
                return qty;   // unreachable in practice; defensive
            }
        }
    }
}
```

**Trade price is the maker's price, always.** The resting order sets the price;
the aggressor accepts it. This is standard across every real venue, and it's
worth stating explicitly because the tempting alternative — comparing the two
orders' timestamps to decide whose price wins — is wrong: it reads the clock to
answer a question the book structure already answers. Anything resting is by
definition the maker; nothing needs to be compared. The timestamp version also
breaks under equal timestamps and under clock skew, and it is exactly the kind
of environmental read that D4 forbids.

### Order entry

```rust
impl OrderBook {
    pub fn new_order(&mut self, inst: InstrumentId, id: OrderId, owner: OwnerId,
                     side: Side, price: Ticks, qty: Lots, kind: OrderKind,
                     out: &mut Vec<Change>)
    {
        if qty <= 0 {
            let r = if qty == 0 { RejectReason::ZeroQty } else { RejectReason::NegativeQty };
            out.push(Change::Rejected { id, reason: r });
            return;
        }
        if self.index.contains_key(&id) {
            out.push(Change::Rejected { id, reason: RejectReason::DuplicateId });
            return;
        }

        // Pre-checks that must happen before any state mutation.
        match kind {
            OrderKind::PostOnly => {
                if self.would_cross(side, price) {
                    out.push(Change::Rejected { id, reason: RejectReason::WouldCross });
                    return;
                }
            }
            OrderKind::Fok => {
                if self.available(side, Some(price)) < qty {
                    out.push(Change::Rejected { id, reason: RejectReason::InsufficientBook });
                    return;
                }
            }
            OrderKind::Market => {
                if self.opposite_empty(side) {
                    out.push(Change::Rejected { id, reason: RejectReason::NoLiquidity });
                    return;
                }
            }
            _ => {}
        }

        out.push(Change::Accepted { id });

        let limit = match kind { OrderKind::Market => None, _ => Some(price) };
        let rest = match kind {
            OrderKind::PostOnly => qty,   // pre-checked as non-crossing; never takes
            _ => self.take(side, limit, qty, id, owner, inst, out),
        };

        match kind {
            OrderKind::Limit | OrderKind::PostOnly if rest > 0 => {
                self.rest_order(id, owner, side, price, rest);
            }
            OrderKind::Ioc | OrderKind::Market if rest > 0 => {
                out.push(Change::Cancelled { id, remaining: rest });
            }
            _ => {}   // fully filled, or Fok (pre-checked, always fully fills)
        }
    }
}
```

The five order kinds are one `match` over a shared `take` walk. `Fok` and
`PostOnly` are each a single pre-check against a read-only helper — that's why
they're included rather than deferred; they cost almost nothing once `take`
exists, and retrofitting them later means revisiting the entry path.

**Pre-checks strictly before mutation.** `Fok` and `PostOnly` must be able to
reject with the book completely untouched. Doing the check inline during the
walk would mean unwinding partial fills, and an unwind path is a rollback
mechanism you'd then have to keep correct forever.

```rust
    /// Read-only. Total quantity available to a taker at `limit` or better.
    fn available(&self, side: Side, limit: Option<Ticks>) -> Lots {
        let levels = match side.opposite() { Side::Bid => &self.bids, Side::Ask => &self.asks };
        levels.iter()
            .take_while(|l| limit.map_or(true, |p| side.crosses(p, l.price)))
            .map(|l| l.total)
            .sum()
    }

    fn would_cross(&self, side: Side, price: Ticks) -> bool {
        let levels = match side.opposite() { Side::Bid => &self.bids, Side::Ask => &self.asks };
        levels.front().map_or(false, |l| side.crosses(price, l.price))
    }
```

Note `available` ignores self-trade prevention, so a `Fok` from an owner with
resting orders on the far side can pass its pre-check and then fail to fill.

```
// ponytail: Fok + StpMode is a known hole. Fine while STP is None (games) or
// while owners don't quote both sides. Fix when a market-maker owner appears:
// make `available` skip levels owned by the taker.
```

### Cancel and amend

```rust
    pub fn cancel(&mut self, id: OrderId, out: &mut Vec<Change>) {
        let Some(&i) = self.index.get(&id) else {
            out.push(Change::Rejected { id, reason: RejectReason::UnknownOrder });
            return;
        };
        let remaining = self.slots[i as usize].qty;
        self.unlink(i);
        self.index.remove(&id);
        self.dealloc(i);
        out.push(Change::Cancelled { id, remaining });
    }

    /// Quantity-only amend. Decrease keeps queue position; increase loses it.
    pub fn amend(&mut self, id: OrderId, qty: Lots, out: &mut Vec<Change>) {
        let Some(&i) = self.index.get(&id) else {
            out.push(Change::Rejected { id, reason: RejectReason::UnknownOrder });
            return;
        };
        if qty <= 0 { return self.cancel(id, out); }

        let s = self.slots[i as usize];
        if qty < s.qty {
            let delta = s.qty - qty;
            self.slots[i as usize].qty = qty;
            let levels = match s.side { Side::Bid => &mut self.bids, Side::Ask => &mut self.asks };
            let li = Self::find_level(levels, s.side, s.price).unwrap();
            levels[li].total -= delta;
        } else if qty > s.qty {
            // Increase = cancel/replace. Moves to the back of the queue.
            self.cancel(id, out);
            self.rest_order(id, s.owner, s.side, s.price, qty);
        }
        out.push(Change::Amended { id, qty });
    }
```

**Decrease keeps priority, increase loses it.** This is the universal exchange
convention and it exists because the alternative is exploitable: if increasing
kept your place, you'd queue 1 lot at the front and inflate it the instant
something interesting appeared, jumping everyone who committed real size. It is
also why an amend that raises size is modelled as cancel/replace rather than an
in-place edit — same code path, no special case.

Price amends are deliberately not supported. A price change is a cancel and a
new order in every meaningful sense, and the caller can express that in two
events without the book needing a third verb.

---

## 6. Aggregation

Merging `k` provider books for one instrument. Read-only, allocation-light,
and it must keep provider attribution — routing (D18/D19) needs to know *who*
is showing the price, not just what the price is.

```rust
pub struct AggLevel {
    pub price: Ticks,
    pub qty: Lots,
    pub sources: SmallVec<[(ProviderId, Lots); 4]>,   // ordered by ProviderId
}

pub struct BookView {
    pub instrument: InstrumentId,
    pub bids: Vec<AggLevel>,
    pub asks: Vec<AggLevel>,
}

impl Engine {
    pub fn aggregate(&self, inst: InstrumentId, depth: usize) -> BookView { .. }
}
```

Algorithm, per side:

1. Collect the top `depth` levels from each non-stale book for `inst`.
2. `k`-way merge by price — best first.
3. Coalesce equal prices, summing `qty` and appending to `sources`.
4. Stop at `depth` merged levels.

With `k` around 5 and `depth` around 10 this is a merge of ~50 pairs of
integers: comfortably sub-microsecond, and no reason to cache it. Caching would
mean invalidation, and invalidation is where this kind of code goes wrong.

Rules that make it correct:

- **Skip stale books entirely** (`stale == true`). This is where the staleness
  flag pays off — a frozen feed leaves the aggregate rather than poisoning it.
- **`sources` is sorted by `ProviderId`, not by size.** Determinism (D4): two
  providers showing the same size must produce the same byte output on every
  run. Sorting by size would tie-break on hash order.
- **The aggregate may legitimately cross.** LP-A's bid can exceed LP-B's ask —
  that's an arbitrage, and it's real information, not a bug. Never "fix" it by
  clamping. A single `OrderBook` must never cross internally; an *aggregate*
  across venues absolutely may, and the moment it does is the moment somebody
  wants to know.

---

## 7. `Engine` — the container

```rust
pub struct Engine {
    level_books: HashMap<(ProviderId, InstrumentId), LevelBook>,
    order_books: HashMap<InstrumentId, OrderBook>,   // provider = internal
    by_provider: HashMap<ProviderId, Vec<InstrumentId>>,   // for O(1) Clear
    max_age_ns: u64,
    now_ns: u64,     // last Tick; never read from the OS
}
```

`by_provider` exists purely so `Clear` doesn't scan every book. It's the index
that makes D2's central operation cheap.

```rust
impl Engine {
    pub fn apply_into(&mut self, e: &Event, out: &mut Vec<Change>) {
        match e {
            Event::Tick { ts_nanos } => {
                self.now_ns = *ts_nanos;
                self.mark_stale(out);
            }
            Event::Clear { provider } => {
                for inst in self.by_provider.remove(provider).unwrap_or_default() {
                    self.level_books.remove(&(*provider, inst));
                    out.push(Change::BookCleared { provider: *provider, instrument: inst });
                    self.emit_top(inst, out);
                }
            }
            Event::Snapshot { provider, instrument, bids, asks } => {
                let b = self.level_books.entry((*provider, *instrument)).or_default();
                b.replace(bids.clone(), asks.clone());
                b.last_update_ns = self.now_ns;
                b.stale = false;
                out.push(Change::BookReplaced { provider: *provider, instrument: *instrument });
                self.emit_top(*instrument, out);
            }
            Event::Delta { provider, instrument, side, price, qty } => {
                let Some(b) = self.level_books.get_mut(&(*provider, *instrument)) else { return };
                if b.apply_delta(*side, *price, *qty) {
                    b.last_update_ns = self.now_ns;
                    b.stale = false;
                    out.push(Change::LevelUpdated {
                        provider: *provider, instrument: *instrument,
                        side: *side, price: *price, qty: *qty,
                    });
                    self.emit_top(*instrument, out);
                }
            }
            Event::New { instrument, id, owner, side, price, qty, kind } => {
                self.order_books.entry(*instrument).or_default()
                    .new_order(*instrument, *id, *owner, *side, *price, *qty, *kind, out);
                self.emit_top(*instrument, out);
            }
            Event::Cancel { instrument, id } => { .. }
            Event::Amend  { instrument, id, qty } => { .. }
        }
    }
}
```

### `emit_top` and the change-suppression rule

`emit_top` recomputes the aggregate top of book and pushes `TopOfBook` **only if
it differs from the last one emitted**. Most book updates happen deep in the
book and change nothing a consumer cares about. Suppressing them here means
they never reach conflation, never cross a socket, and never wake a browser.

This is the single highest-leverage line in the file. In FX, the large majority
of level updates leave the top of book untouched.

### A `Delta` for an unknown book is dropped

Deliberately. It means the adapter sent a delta before a snapshot, which means
it has a sequencing bug. Creating the book from a delta would build a book with
a hole in it that looks valid and prices wrongly forever. Dropping it is
recoverable — the adapter's gap detection sends a snapshot. Debug builds should
assert here; release builds should count it in a metric.

---

## 8. Invariants

Assert these in `debug_assert!` and check them exhaustively in fuzz tests. Every
one has been a real bug in a real order book somewhere.

**Per `OrderBook`:**

1. `level.total == sum(slot.qty for slot in level's list)`
2. `bids` strictly descending; `asks` strictly ascending; no duplicate prices
3. Never internally crossed: `best_bid.price < best_ask.price`
4. `index.len() == count of linked slots`; every `index` entry points at a live slot
5. Every level has `head != NIL` (empty levels are removed, never retained)
6. `slot.prev == NIL` iff it is `level.head`; `slot.next == NIL` iff it is `level.tail`
7. The free list contains no slot reachable from any level

**Per `LevelBook`:**

8. Sorted per side, no duplicate prices, all `qty > 0` (zero means removed)

**Conservation, across an `apply`:**

9. `sum(Trade.qty) == sum(Filled.qty for makers) == taker qty filled`
10. Total quantity is conserved: what left the book equals what was traded

Invariant 3 is the one that catches the largest class of matching bugs. A book
that crosses internally means the `take` loop exited early — and every downstream
number derived from it is wrong from that moment on.

---

## 9. Determinism rules (D4)

Violating any of these silently breaks replay, and replay is how you debug
everything else.

| Forbidden | Instead |
|---|---|
| `Instant::now()`, `SystemTime` | `Event::Tick` |
| Iterating a `HashMap` to build output | sort by key, or iterate a `Vec` |
| `rand` | seeded PRNG in the *caller*, values passed in as event fields |
| `f64` anywhere | integers |
| Threads, `async`, channels inside the core | the server's problem |
| Sorting by a value with possible ties | tie-break on a unique key (`ProviderId`, `OrderId`) |

`HashMap` for **lookup** is fine and encouraged — `index`, `level_books`. What is
forbidden is letting iteration order reach the output. If you need to iterate,
collect and sort, or keep a parallel `Vec`.

**The test that enforces all of this:** run the same event log twice in one
process and byte-compare the `Change` streams; then run it in a second process
and compare again. The second run catches address-dependent hashing that the
first misses, because `HashMap`'s `RandomState` is seeded per process.

---

## 10. Tests

Three layers. All of them are pure functions over data, which is the entire
point of keeping I/O out.

### Unit — matching scenarios

```rust
#[test]
fn maker_price_wins() {
    let mut b = OrderBook::default();
    let mut out = Vec::new();
    b.new_order(I, 1, ALICE, Side::Ask, 100, 10, OrderKind::Limit, &mut out);
    out.clear();
    // Bob bids 105 — crosses. He pays Alice's 100, not his own 105.
    b.new_order(I, 2, BOB, Side::Bid, 105, 10, OrderKind::Limit, &mut out);
    assert!(matches!(out.iter().find(|c| matches!(c, Change::Trade{..})),
        Some(Change::Trade { price: 100, qty: 10, maker: 1, taker: 2, .. })));
}

#[test]
fn time_priority_within_a_level() {
    // two asks at the same price; the earlier one fills first, in full
}

#[test]
fn partial_fill_keeps_queue_position() {
    // maker half-filled stays at the head of its level
}

#[test]
fn cancel_is_exact() {
    // cancel the middle of three orders at one price; the other two are intact
    // and level.total is correct
}

#[test]
fn fok_rejects_without_touching_the_book() {
    // book unchanged, byte for byte, after a rejected Fok
}

#[test]
fn post_only_rejects_a_crossing_order() { .. }

#[test]
fn market_into_empty_book_rejects() { .. }
```

### Property / fuzz

Generate random event streams — new, cancel, amend, in any order, with
adversarial duplicate ids and zero quantities — and assert every invariant from
§8 after each `apply`. This finds the linked-list bugs that hand-written tests
never will, because the failures need a specific *sequence*, not a specific
input.

`cargo-fuzz` with a structured `Arbitrary` impl for `Event` is the cheapest
possible version and it works well.

### Golden replay

Record a real provider session to a file. Replay it into every new build and
byte-compare the `Change` stream against the stored output. This is the
regression test that actually catches matching changes, and it costs nothing
once determinism holds.

Store goldens as the *event log plus a hash of the change stream* rather than
the full change stream — the logs compress well, the outputs don't, and a hash
tells you just as much for a regression check.

---

## 11. Performance notes

Measured numbers, the target-vs-measured comparison, and the "why aggregate is
6x off and staying that way" reasoning live in `docs/BENCHMARKS.md` §1 — not
duplicated here, so there's one table to keep current. Run
`cargo bench -p blox-core` to reproduce it.

What is actually worth doing, in order:

1. **Suppress no-op changes** (`apply_delta` returning `false`, `emit_top`
   deduplicating). This is a 10–100× reduction in downstream work and it is
   three lines.
2. **Reuse the `Change` buffer** (`apply_into`). Removes allocation from the hot
   path entirely.
3. **Intern symbols to `u16` at the boundary.** Already in the design.
4. Everything else.

Do not add SIMD, `#[repr(align(64))]`, or a custom allocator. If you ever
genuinely need those, you're in Profile C and the whole design changes anyway.
