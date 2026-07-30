# Conditional Orders — Core Addition

Stops, stop-limits, take-profit, stop-loss, and — in a replica-book world —
limit orders too.

Extends `docs/CORE.md`. Everything there still holds: no I/O, no clock, no
floats, deterministic.

---

## 1. Why this is in the core

A trigger is the question *"has the price crossed a level?"*, and the book is
the only thing that knows the answer. Putting triggers in the app layer means
the app re-derives top-of-book on every tick and races the engine to do it.

It also stays pure: a trigger fires on a **price event**, never on a clock.
Replay reproduces every trigger at the identical sequence number, which is the
property that lets you answer "why did my stop fill there?" after money has
changed hands.

Both projects want it — a trading-pad style app for its resting conditional
orders, a broker-facing system for stops it doesn't want to reveal to the LP.
Shared, no mode flag.

---

## 2. The realization that shapes this

**Against a `LevelBook`, a limit order is also a conditional.**

In an `OrderBook` you own, a limit order *rests* — it joins a queue, other
participants see it, and someone eventually trades against it. Against a
provider replica there is no queue to join and nobody to see it. Your limit
order just sits there waiting for the market to come to you.

So in a replica world, every pending order is the same shape:

```
wait until price satisfies a condition → then execute
```

Buy stop, sell stop, buy limit, sell limit, take-profit, stop-loss — one
mechanism, six configurations. The only real difference is which direction the
price has to move and what the order becomes when it fires.

This matters for any replica-only app: **it never needs `OrderBook`.**
Users trade against provider prices, not against each other. `LevelBook` +
triggers + a fill function is the whole engine. `OrderBook` stays in the core
for true venue use cases (`docs/ADAPTERS.md` §5's game example, or a future
internal matching venue) and that kind of app simply never constructs one.

---

## 3. Vocabulary

```rust
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum TriggerKind {
    /// Fires when price moves AWAY from you. Becomes a market order.
    /// Buy stop sits above the market, sell stop below.
    Stop,
    /// Fires like Stop, becomes a Limit at `limit_price` instead of a market order.
    StopLimit { limit_price: Ticks },
    /// Fires when price moves TOWARD you. Becomes a limit order at `price`.
    /// Buy limit sits below the market, sell limit above.
    Limit,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Trigger {
    pub id: OrderId,
    pub owner: OwnerId,
    pub instrument: InstrumentId,
    pub side: Side,
    pub trigger_price: Ticks,
    pub qty: Lots,
    pub kind: TriggerKind,
    /// Set for TP/SL. Cancelled automatically when the position closes.
    pub attached_to: Option<PositionId>,
    /// Set for TP/SL pairs. Firing one cancels the other.
    pub oco_with: Option<OrderId>,
}
```

New events and changes:

```rust
// Event
PlaceTrigger  { trigger: Trigger },
CancelTrigger { instrument: InstrumentId, id: OrderId },

// Change
TriggerPlaced    { id: OrderId },
TriggerFired     { id: OrderId, ref_price: Ticks, becomes: FiredAs },
TriggerCancelled { id: OrderId, reason: CancelReason },

pub enum FiredAs {
    Market,
    Limit { price: Ticks },
}

pub enum CancelReason {
    ByUser,
    Oco,             // sibling fired
    PositionClosed,  // the position it was attached to is gone
    Rejected,        // would fire immediately — see §5
}
```

**The core fires the trigger; it does not execute it.** `TriggerFired` says
"this became a market order at sequence N." Turning that into a fill is the
app's job (a fill simulator, or a broker execution client). The
core stays out of the business of deciding what a fill costs.

---

## 4. Reference price — get this right or nothing else matters

"The price crossed" begs the question: *which* price. Bid, ask, mid, or last
trade? Pick wrong and every stop fires at a systematically wrong moment, in the
direction that costs the user money.

**The rule brokers use, and the one to copy: a trigger references the price you
would actually transact at.**

| Order | Sits | Fires when | Reference |
|---|---|---|---|
| Buy stop | above market | market rises to it | **ask** ≥ trigger |
| Sell stop | below market | market falls to it | **bid** ≤ trigger |
| Buy limit | below market | market falls to it | **ask** ≤ trigger |
| Sell limit | above market | market rises to it | **bid** ≥ trigger |

Buying always references the ask, selling always references the bid. No
exceptions, and it collapses to one line:

```rust
#[inline]
fn ref_price(side: Side, top: &Top) -> Option<Ticks> {
    match side { Side::Bid => top.ask, Side::Ask => top.bid }
}
```

The tempting alternative — trigger on mid, or on last trade — is wrong for a
specific and expensive reason. On a wide spread, mid can cross your stop level
while the price you'd actually get never does. The user's stop fires, they're
filled well past where they asked to be, and they are right to complain.

**A stop-loss on a long position is a sell**, so it references the bid. Widening
spreads at rollover or on news will trip stops that a mid-based reference would
have left alone. That is correct behaviour and matches every real broker, but
it *will* generate support tickets, so it's worth documenting for users rather
than discovering in an argument.

---

## 5. Immediate-fire rejection

A buy stop placed *below* the current ask would fire instantly. So would a buy
limit placed above it. That's almost always a typo or a decimal error.

```rust
fn would_fire_now(t: &Trigger, top: &Top) -> bool {
    let Some(p) = ref_price(t.side, top) else { return false };
    match (t.kind, t.side) {
        (TriggerKind::Limit, Side::Bid) => p <= t.trigger_price,
        (TriggerKind::Limit, Side::Ask) => p >= t.trigger_price,
        (_,                  Side::Bid) => p >= t.trigger_price,
        (_,                  Side::Ask) => p <= t.trigger_price,
    }
}
```

**Reject rather than fire.** A user placing a stop expects it to wait; firing
immediately turns a mistyped protective order into an instant market order in
the wrong direction. `Rejected` is recoverable, an accidental position is not.

This is the same reasoning as `PostOnly` in `docs/CORE.md` §5, and the same
discipline: check strictly before mutating anything.

---

## 6. Structure

**Correction from implementation.** This section originally specified *two*
lists, one per direction of travel. That is wrong, and the bug is worth
recording because it is not obvious.

Buy triggers reference the ask; sell triggers reference the bid (§4). A
direction-only bucket therefore holds two different reference prices, so its
head is not a valid guard for the rest: a sell limit at 100 (bid 99, does not
fire) can sit in front of a buy stop at 101 (ask 102, *does* fire), and the
head check hides it. The stop never fires.

The fix is to bucket by `(side, direction)`, giving four homogeneous buckets.
The head check is valid again, and it is still O(1) — four integer comparisons
instead of two.

```rust
pub struct Triggers {
    /// [Bid-down, Bid-up, Ask-down, Ask-up].
    /// "up" buckets ascend (lowest fires first as price rises);
    /// "down" buckets descend (highest fires first as price falls).
    buckets: [Vec<Trigger>; 4],
    ids: HashMap<OrderId, usize>,
}

#[inline]
fn bucket_of(side: Side, up: bool) -> usize { ((side as usize) << 1) | (up as usize) }
```

Checking whether anything fires stays two comparisons per bucket, regardless of
how many thousands are parked:

```rust
pub fn any_fires(&self, top: &Top) -> bool {
    (0..4).any(|b| self.head_fires(b, top))
}

#[inline]
fn head_fires(&self, b: usize, top: &Top) -> bool {
    let Some(t) = self.buckets[b].first() else { return false };
    top.taker_price(side_of(b)).is_some_and(|p| t.satisfied_at(p))
}
```

Measured: 1000 parked triggers add 33 ns to a delta, and the cost does not grow
with the number parked (`docs/BENCHMARKS.md`).

Sorted insert is `O(n)` on the memmove, but placement is rare relative to price
updates — the ratio is thousands of ticks per placement — so it's the right
trade.

```
// ponytail: Vec + memmove is right for hundreds of triggers per instrument.
// Past ~10k, switch to BTreeMap<Ticks, SmallVec<Trigger>>, same head-check.
```

Ties at the same trigger price fire in **placement order**. Insert with
`partition_point` so equal prices append after existing ones — first placed,
first fired. Same principle as time priority in `OrderBook`, and it's what makes
the output reproducible when a gap takes out twenty stops at once.

---

## 7. Evaluation, and the cascade problem

Triggers are checked after every change to the top of book, inside `apply_into`.

The subtle part: **a fired trigger can move the price and fire more triggers.**
In a venue that's real cascading. Against a replica book it can't happen —
your orders don't move the provider's prices — but the loop must still be
bounded, because a bug that re-inserts a trigger would otherwise hang the
engine rather than fail visibly.

```rust
const MAX_CASCADE: u32 = 8;

fn check_triggers(&mut self, inst: InstrumentId, out: &mut Vec<Change>) {
    for round in 0..MAX_CASCADE {
        let top = self.top_of_book(inst);
        let mut fired = false;

        // Drain both heads while they satisfy the condition.
        while let Some(t) = self.triggers(inst).pop_if_fired(top) {
            fired = true;
            let becomes = match t.kind {
                TriggerKind::Stop            => FiredAs::Market,
                TriggerKind::StopLimit { limit_price } => FiredAs::Limit { price: limit_price },
                TriggerKind::Limit           => FiredAs::Limit { price: t.trigger_price },
            };
            out.push(Change::TriggerFired {
                id: t.id,
                ref_price: ref_price(t.side, &top).unwrap(),
                becomes,
            });
            if let Some(sib) = t.oco_with {
                self.cancel_trigger(inst, sib, CancelReason::Oco, out);
            }
        }

        if !fired { return; }
        if round == MAX_CASCADE - 1 {
            debug_assert!(false, "trigger cascade did not settle");
            // Release: stop evaluating this tick. Never spin.
        }
    }
}
```

**Drain `up` and `down` in a defined order** — all of `up` first, then `down`, or
strictly interleaved by price. Either is fine; what matters is that it's fixed,
because it decides the output order when a gap trips both sides at once. Pick
one, write it down, and never let it depend on which `HashMap` bucket something
landed in.

---

## 8. Gap semantics — where the money is

The single most important behaviour in this file.

Price gaps. It's Sunday 22:00 CET and EURUSD opens 40 pips below Friday's close.
A user has a sell stop at 1.0850. The market never traded at 1.0850 — it went
from 1.0890 straight to 1.0810.

**They do not get 1.0850. They get 1.0810.**

The trigger price is a *condition*, never a promise of a fill price. Filling at
the trigger price would mean inventing liquidity that didn't exist — you'd be
paying users out of your own pocket for prices nobody could have got.

The core makes this structurally impossible by reporting `ref_price` — the price
that actually caused the trigger — and leaving execution to the app:

```rust
Change::TriggerFired { id, ref_price: 108100, becomes: FiredAs::Market }
//                          ^^^^^^ where the market IS, not where the stop WAS
```

The app fills from the current book (a fill simulator) or sends to the broker
(execution client). Neither has any way to reach the stale trigger price,
because the core never hands it to them.

**Stop-limit gaps the other way and users find this surprising.** A stop-limit
with trigger 1.0850 / limit 1.0845 gaps to 1.0810: the stop fires, the limit
order is placed at 1.0845, and the market is at 1.0810 — so it never fills, and
the user is still holding a losing position. That's exactly what stop-limit
means and exactly why plain stops exist. The engine's job is to be correct here,
not kind.

---

## 9. TP/SL and OCO

Take-profit and stop-loss attach to a position, come in pairs, and must clean up
after themselves. Three rules:

1. **Firing one cancels the other** (`oco_with`). A position closed at
   take-profit must not leave a live stop-loss behind — that would open a *new*
   position in the opposite direction later.
2. **Closing the position cancels both** (`attached_to`). Manual close, stop-out,
   tournament end — same path.
3. **Reducing a position scales them.** Close half of a long, and the attached
   TP/SL quantities halve. Otherwise the remaining stop is oversized and flips
   the user short when it fires.

Rule 3 is the one that's usually discovered in production. The failure looks
like "my stop-loss made me short" and it's genuinely alarming to a user.

Position lifecycle lives in the app layer (D20), so the app emits
`CancelTrigger` on close. The core provides the `attached_to` field and a
lookup; it does not track positions itself.

```
// ponytail: attached_to is a plain field with a helper to find triggers by
// position. No position model in the core — that's D20's four integers, and
// they live in the app. If a second app needs the same cleanup logic, extract
// it then, not now.
```

---

## 10. Determinism

Additions to `docs/CORE.md` §9:

| Rule | Why |
|---|---|
| Triggers at equal price fire in placement order | otherwise a gap produces a different order every run |
| `up` fully drained before `down` (or a fixed interleave) | fixes output order when both sides trip |
| Cascade bounded at `MAX_CASCADE` | a re-insertion bug fails visibly instead of hanging |
| `ref_price` recorded in `TriggerFired` | replay can prove why it fired |
| No clock in trigger evaluation | firing is a price event; time-based expiry is a `Tick` event |

Good-till-date orders expire on `Tick`, never on a wall clock — same mechanism
as staleness in `docs/CORE.md` §3.

---

## 11. Tests

```rust
#[test]
fn buy_stop_references_ask_not_mid() {
    // bid 1.0840 / ask 1.0860, mid = 1.0850. Buy stop at 1.0850.
    // Mid has crossed. Ask has not. It must NOT fire.
}

#[test]
fn gap_fills_at_market_not_at_trigger() {
    // sell stop 1.0850; book jumps 1.0890 -> 1.0810
    // TriggerFired.ref_price == 108100
}

#[test]
fn stop_below_market_is_rejected_not_fired() {
    // buy stop under the ask -> Rejected, book untouched
}

#[test]
fn oco_sibling_cancelled_on_fire() { .. }

#[test]
fn closing_a_position_cancels_both_tp_and_sl() { .. }

#[test]
fn equal_price_triggers_fire_in_placement_order() {
    // three stops at the same price; assert the exact firing order,
    // then re-run in a fresh process and assert it again
}

#[test]
fn one_gap_fires_twenty_stops_deterministically() {
    // byte-compare the Change stream across two runs
}
```

The last two are the ones that catch the failures that only appear in
production, because they need a *sequence* rather than an input. Fold trigger
placement into the `cargo-fuzz` generator from `docs/CORE.md` §10 and assert
after every `apply`: no trigger remains whose condition is already satisfied.
That single invariant covers most of what can go wrong here.
