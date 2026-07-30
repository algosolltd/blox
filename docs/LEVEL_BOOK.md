# Level Book Representation

Why one side of a provider book is a sorted `Vec` and not a tick-indexed
bitmap, decided by measurement rather than by argument.

`blox-core/src/level_book.rs` carries both structures. Only the `Vec` is wired
in. This document is the record of why, and the condition that would flip it.

Related: `docs/CORE.md` §4 (what a `LevelBook` is for), `docs/BENCHMARKS.md`
(engine-wide numbers), `docs/IMPLEMENTATION.md` §7 (the omission table).

---

## 1. The question

A `LevelBook` is a replica of one provider's aggregated depth — prices and
quantities, no order identity. Its whole job is four operations:

| Operation | Called by | How often |
|---|---|---|
| `apply_delta` | every `Event::Delta` from an adapter | hot |
| `top` / `best_bid` / `best_ask` | every event, to emit `Change::TopOfBook` | hot |
| `side()`, walked in order | `Engine::agg_side`, bounded by `depth` | on demand |
| `available` / `vwap_for` | a fill simulator | unbuilt |

The original note in `level_book.rs` said `Vec` is right up to a few hundred
levels and past ~1000 the memmove on a mid-book insert dominates. That was a
guess. This is the measurement.

## 2. The two structures

**Sorted `Vec<(Ticks, Lots)>`, best-first.** Bids descend, asks ascend, so
index 0 is the best price on both sides and every walk is direction-free.
Lookup is a binary search; insert and remove memmove the tail.

**Tick-indexed dense array plus occupancy bitmap.** One `Lots` per tick over a
bounded window, plus a `u64` per 64 ticks recording which are occupied.
`trailing_zeros` finds the next non-empty level. Insert and remove are O(1) —
they write one slot and flip one bit. Ordered iteration is a masked word scan.

The second is adapted from the `PriceLevelArray` in
[bozoslav/order-book](https://github.com/bozoslav/order-book). Two things were
deliberately not ported — see §7.

## 3. Method

Same machine as `docs/BENCHMARKS.md`: AMD Ryzen AI 9 HX PRO 370, Linux 7.0.10,
rustc 1.96.0, release profile (`lto = true`, `codegen-units = 1`).

Every number below is a minimum over repeated rounds, and every comparison is
an A/B of two binaries run alternately in the same session. That matters more
than usual here: this box drifts. Across the sessions in this document the same
untouched `OrderBook` operations moved by up to 4%, and absolute figures ran
~10% slower than the ones recorded in `docs/BENCHMARKS.md`. **±4% is the noise
floor. Nothing inside it is a result.**

Three scopes, narrow to wide, because each one answers a different objection:

1. the structures in isolation, no engine around them;
2. the real `LevelBook` API at four depths;
3. the whole engine, via `cargo bench -p blox-core`.

## 4. The structures in isolation

ns/op. Levels are contiguous ticks — the best case for the bitmap, since a
sparse book only widens its window.

| Operation | depth 10 | depth 100 | depth 1000 | depth 10000 |
|---|---:|---:|---:|---:|
| update existing level — `Vec` | 9.7 | 7.1 | 9.7 | 15.8 |
| update existing level — bitmap | **4.5** | **4.5** | **2.1** | **2.1** |
| remove + insert level — `Vec` | 12.8 | 21.3 | 38.6 | 258.5 |
| remove + insert level — bitmap | **2.1** | **2.1** | **2.2** | **2.2** |
| best of book — `Vec` | 0.4 | 0.2 | 0.2 | 0.2 |
| best of book — bitmap | 0.3 | 0.3 | 0.3 | 0.3 |
| walk top 10 — `Vec` | **1.7** | **1.6** | **1.7** | **1.7** |
| walk top 10 — bitmap | 15.7 | 15.7 | 15.8 | 15.6 |

### Reading it

**The bitmap's write cost is flat and the `Vec`'s is linear.** 2.1 ns whether
the side holds 10 levels or 10,000; the `Vec` goes from 12.8 to 258.5 ns on the
same span. That is the memmove, and it is exactly the effect the original note
predicted.

**The bitmap's read cost is also flat, and it is 9x worse.** ~1.5 ns per level
against ~0.17 ns. A masked word load plus `trailing_zeros` plus a multiply to
recover the price, per level, none of it vectorisable — against a `memcpy` from
contiguous memory. This is the part the original note did not anticipate, and
it is what decides the outcome.

**Best of book is a tie** at ~0.2-0.5 ns either way. Sub-nanosecond, one cached
slot lookup versus one array index. It does not enter the decision.

## 5. The real `LevelBook`

Driving the actual public API, `Vec` build against bitmap build, ns/op.

| Operation | depth 8 | depth 20 | depth 500 | depth 2000 |
|---|---:|---:|---:|---:|
| `apply_delta`, existing level | 3.3 → 4.2 | 5.0 → 6.3 | 8.6 → **2.4** | 10.7 → **2.4** |
| `apply_delta`, remove + insert | 11.9 → 13.2 | 15.8 → 17.2 | 29.7 → **2.7** | 61.3 → **2.6** |
| `available`, half the book | 1.2 → 1.3 | 2.5 → **2.4** | 66.9 → 564.7 | 236.5 → 2238.4 |
| `top` | 0.3 → 0.5 | 0.3 → 0.5 | 0.3 → 0.5 | 0.3 → 0.5 |

### Reading it

**The crossover is real and it is sharp.** At depth 2000 writes get 78-96%
cheaper. At depth 8-20 they get 8-27% *more* expensive, because the promotion
check and the wider dispatch cost more than the three-element memmove they were
meant to avoid.

**`available` at depth is the counterweight**: 236.5 ns to 2238.4 ns, +847%.
Not a surprise given §4 — it is the same 9x per level, amplified by walking a
thousand of them. Any ordered walk that is not bounded by a small `depth` pays
this.

**FX books are 5-20 levels** (`docs/CORE.md` §4, `docs/BENCHMARKS.md` §5). That
is the left half of this table, and the bitmap loses there.

## 6. The whole engine

`cargo bench -p blox-core`, 7 alternating runs of each binary, minimum. Three
configurations were tried.

**(a) Bitmap whenever the span fits** — no depth condition:

| Operation | `Vec` | bitmap | |
|---|---:|---:|---|
| aggregate 5 providers, depth 10 | 2041.9 | 2403.8 | +17.7% |
| delta, insert + remove level | 99.9 | 96.0 | −3.9% |
| delta, existing level | 48.7 | 48.4 | −0.6% |
| mixed workload (testkit) | 210.9 | 220.3 | +4.5% |

**(b) Bitmap only past 64 levels** — the testkit books are shallow, so this
should have been free:

| Operation | `Vec` | gated bitmap | |
|---|---:|---:|---|
| aggregate 5 providers, depth 10 | 2019.3 | 2236.3 | +10.7% |
| delta, existing level | 48.1 | 51.2 | +6.4% |
| delta, insert + remove level | 100.5 | 105.9 | +5.4% |
| mixed workload (testkit) | 202.5 | 213.1 | +5.2% |
| *cancel* (untouched `OrderBook`) | *76.8* | *76.2* | *−0.8%* |
| *new limit order* (untouched) | *93.3* | *93.4* | *+0.1%* |

**(c) Bitmap present but not wired in** — what shipped:

| Operation | `Vec` | dormant | |
|---|---:|---:|---|
| aggregate 5 providers, depth 10 | 1950.6 | 1904.3 | −2.4% |
| delta, existing level | 45.2 | 45.6 | +0.9% |
| delta, insert + remove level | 94.8 | 94.6 | −0.2% |
| mixed workload (testkit) | 200.2 | 196.8 | −1.7% |

### Reading it

**(b) is the surprising one and it is why this ended where it did.** Gating on
depth was supposed to make the bitmap free for shallow books — they never
promote, so they run the same sorted-`Vec` code. They regressed 5-6% anyway.

Two causes, both structural rather than incidental:

- `apply_delta` gained a promotion check on the sparse path.
- `side()` had to return an enum iterator instead of `&[(Ticks, Lots)]`, because
  the dense representation has no contiguous sorted array to hand out. That
  costs `agg_side` its specialised `Vec::extend`, which is the +10.7% on
  `aggregate` — a path that never touches a dense book at all.

Adding a `size_hint` to the iterator recovered nothing (+17.0% against +17.7%,
inside the noise). **Merely offering the choice taxed the path that does not
use it**, and there is no cheap way to hand back a slice from a bitmap.

**(c) confirms the tax is gone.** Every row is inside the ±4% floor, and
`aggregate` is back from +10.7% to −2.4%.

## 7. Decision

**The sorted `Vec` stays live. The bitmap stays in the file, switched off.**

At 5-20 levels the bitmap is slower on every operation that matters, and
wiring it in costs 5-11% on paths that would never use it. The win only exists
past ~64 levels, which this system does not have.

Set against the scale in `docs/BENCHMARKS.md` §3: the socket round trip is
2.7µs and a broker round trip is 20-40ms. The best case here — 59 ns saved on a
`remove + insert` at depth 2000 — is 2% of one socket hop and 0.0002% of one
broker hop. The `Vec` is not the reason anything is slow.

The bitmap is kept because the measurement is the expensive part of this work,
not the code. `Repr::apply_delta` is complete — promotion, rewindowing, sparse
fallback, no-op suppression — so turning it on is a decision rather than a
rewrite.

### Switching it on

Warranted when a side habitually holds **≥64 levels within a few thousand
ticks** — a crypto venue book, or an FX aggregate over many providers deep.
Three edits:

1. `LevelBook` holds `Repr` per side instead of `Vec<(Ticks, Lots)>`.
2. `side()` returns `Repr::iter()`.
3. `Engine::agg_side` drops its `.iter()` on the result.

Then re-run §6. If `aggregate` regresses more than the deltas gain, the answer
has not changed.

### The two defects not ported

The C++ original is a fixed 1001-slot window over `$100.00–$110.01`, chosen at
compile time, and it **clamps** any price outside that range into the nearest
slot. Measured on the upstream code:

```
bid@50  rests, then ask@100  arrives  ->  FILLS AT 100.00  (buyer's limit was 50.00)
ask@500 rests, then bid@110  arrives  ->  FILLS AT 110.00  (seller's limit was 500.00)
```

Both are fixed here, and both are covered by tests:

- **Never clamp.** A price outside the window rewindows the array; if the
  resulting span will not fit, the side falls back to sparse and keeps the real
  price. `a_price_outside_the_window_widens_it_and_is_never_clamped`.
- **Always cap.** The dense array costs memory per *tick*, not per level, and
  `parse_levels` in `blox-server` accepts any `i64` from an adapter. An
  unbounded window is therefore a remote allocation primitive. Capped at 4096
  ticks — 32 KiB of quantities plus 512 B of bitmap per side.
  `extreme_ticks_do_not_allocate_or_overflow` drives `Ticks::MAX` and
  `Ticks::MIN` through it.

### Tests

The dormant code is dead but not unverified — six tests keep it compiling and
correct, driving `Repr` directly:

| Test | Asserts |
|---|---|
| `dense_agrees_with_a_sorted_model_through_promotion_and_churn` | 20,000 seeded deltas from empty against a sorted model, including the change flag that drives `Change` suppression, crossing the promotion boundary mid-run |
| `a_price_outside_the_window_widens_it_and_is_never_clamped` | the clamping defect above, both inside and beyond the cap |
| `extreme_ticks_do_not_allocate_or_overflow` | `Ticks::MAX` / `Ticks::MIN` from an untrusted adapter |
| `depth_and_span_decide_the_representation` | shallow stays sparse, deep-and-narrow goes dense, deep-and-wide stays sparse |
| `draining_a_dense_side_empties_it_cleanly` | no stale cached `best` after the last level goes |
| `Repr::check` (called by the above) | the bitmap agrees with the quantity array, and the cached best is the real lowest occupied slot — neither is observable from outside, which is why they need asserting |

## 8. What is not measured

- **Sparse books.** Every dense measurement used contiguous ticks, the best
  case. A book with gaps widens the window without adding levels, so the read
  cost rises while the write win does not. The conclusion holds harder, but the
  magnitude is unmeasured.
- **Rewindowing under drift.** A mid that walks out of its window rebuilds the
  array. Headroom is 2x the occupied span, so this is rare in the benchmarks
  and never happened under sustained drift, which is not tested.
- **Memory.** 32 KiB per side per `(provider, instrument)` at the cap, against
  a `Vec`'s 16 bytes per level. At 5 providers × 20 levels that is 320 KiB
  against 3.2 KiB — a 100x increase on a rounding error. Not measured because
  nothing here is memory-bound.
- **The 64-level threshold.** Picked from §5 as the point where the curves
  cross, not tuned. If the bitmap is ever switched on, that number deserves its
  own sweep.
- **`available` / `vwap_for` at depth.** They have no callers yet (the fill
  sim is unbuilt), so their +847% is a property of the structure rather than
  a regression anyone can currently observe. It becomes live the moment the
  fill sim is built.
