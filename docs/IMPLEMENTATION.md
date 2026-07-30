# Implementation Notes

What exists, how to run it, and what the build taught us that the design docs
got wrong.

Design rationale is in `DESIGN.md`; the specs are `docs/CORE.md`,
`docs/CONDITIONAL_ORDERS.md`, `docs/ADAPTERS.md`.
This document is the record of building it.

---

## 1. Layout

```
blox-core/          the engine. Pure, deterministic, zero dependencies.
  src/types.rs        Ticks, Lots, Side, Top, Instrument
  src/event.rs        Event / Change vocabularies
  src/level_book.rs   provider replicas — aggregated depth
  src/order_book.rs   authoritative books — arena + intrusive lists
  src/triggers.rs     conditional orders
  src/engine.rs       container, aggregation, change suppression
  src/decimal.rs      integer decimal parsing for adapter boundaries
  src/testkit.rs      seeded workload generation
  src/bin/replay_hash.rs   cross-process determinism checker
  benches/engine.rs   micro-benchmarks
  tests/              105 tests, see §3

blox-server/        the I/O shell. TCP + line protocol.
  src/main.rs         listener, sequencer channel, single-writer engine thread
  src/protocol.rs     newline-delimited text protocol

blox-sim/           Go: fake market + measurement harness
  agents.go           LP, market makers, noise traders, momentum traders
  client.go           connection wrapper
  main.go             market / throughput / latency modes
  stats.go            percentiles
```

**`blox-core` has no dependencies beyond `std`**, and that is enforced by
having none in `Cargo.toml`. Anything that wants one belongs in the server.

---

## 2. Running it

```bash
cargo test --workspace          # 105 tests, ~3s
cargo test --workspace --release
cargo clippy --workspace --all-targets   # clean
cargo bench -p blox-core        # micro-benchmarks

cargo build --release
cd blox-sim && go build -o blox-sim .

./blox-sim                      # spawns a server, runs all three modes
./blox-sim -mode market -duration 30s
./blox-sim -mode throughput -conns 8 -events 500000
./blox-sim -mode latency -samples 50000
```

The simulator spawns its own server on an ephemeral port unless given `-addr`.

Poking at the protocol by hand:

```
$ ./target/release/blox-server 127.0.0.1:7070
> SUB 1
> NEW 1 1 10 S 10850 100 LIMIT
> NEW 1 2 20 B 10860 150 LIMIT
< EV TRADE 1 10850 100 1 2 B          # maker's price, not the taker's 10860
< EV TOP 1 10860 50 - -               # 50 lots of the taker rested
> CHECK
< OK invariants hold
```

---

## 3. Tests

105 tests across six files. All pass in debug and release; clippy is clean.

| File | n | What it covers |
|---|---|---|
| inline `#[cfg(test)]` | 30 | per-module units — decimal, level book, triggers, types |
| `tests/matching.rs` | 27 | price-time priority, all five order kinds, STP, conservation |
| `tests/providers.rs` | 15 | aggregation, staleness, `Clear`, change suppression |
| `tests/triggers.rs` | 15 | reference prices, gaps, OCO, TP/SL cleanup |
| `tests/determinism.rs` | 6 | in-process reproducibility, batching, prefix replay |
| `tests/replay_process.rs` | 2 | **cross-process** reproducibility |
| `tests/fuzz.rs` | 7 | property tests over random streams, every invariant |
| doctest | 1 | the example in `lib.rs` actually runs |

Three of these earn their place beyond the obvious:

**`Engine::check()` is called after every single `apply` in the fuzz tests.**
It asserts all ten invariants from `docs/CORE.md` §8 — level totals equal the
sum of their lists, no empty levels retained, no cyclic lists, the free list
touches nothing live, books never cross themselves. It caught a real bug within
minutes of being written (§4).

**Cross-process determinism** exists because an in-process rerun cannot catch
hash-order dependence: `HashMap`'s `RandomState` is seeded per process, so two
runs in one process share a seed and agree with each other while both being
wrong. `tests/replay_process.rs` runs the `replay_hash` binary four times per
seed and compares.

**`rejections_never_mutate_the_book`** formats the whole book before and after
3000 guaranteed-rejected orders and compares the strings. A rejected FOK or
PostOnly must leave no trace; anything else means a pre-check ran after a
mutation, which would need an unwind path.

---

## 4. What the build corrected in the design

Four things. Recorded because they were all non-obvious, and three of them were
in documents written with some confidence.

### 4.1 Triggers need four buckets, not two

`docs/CONDITIONAL_ORDERS.md` §6 originally specified two lists, one per
direction of price travel, with an O(1) head check.

That is wrong. Buy triggers reference the *ask* and sell triggers the *bid*, so
a direction-only bucket holds two different reference prices and its head is
not a valid guard. A sell limit at 100 (bid 99, does not fire) sitting in front
of a buy stop at 101 (ask 102, fires) hides it — the stop never fires.

Fixed by bucketing on `(side, direction)`: four homogeneous buckets, still
O(1), four comparisons instead of two. Caught while writing
`a_sell_limit_head_cannot_hide_a_firing_buy_stop`, which is now a regression
test.

### 4.2 `pop_fired` leaked its id index

The first version removed a fired trigger from its bucket but not from the
`ids` map. Ten trigger tests failed instantly with
`0 triggers in buckets, 1 in index`.

Worth noting because of *how* it was caught: not by a test asserting trigger
behaviour, but by `check()` running after every apply. The behavioural
assertions all passed. Only the structural invariant noticed.

### 4.3 The `f64` price trap is far worse than one anecdote

`docs/ADAPTERS.md` warned against `s.parse::<f64>()? * 100_000.0` and used
`1.08501` as the example. On measuring, that particular value survives the
round trip — the doc's example was wrong.

The real number: sweeping every 5-decimal price from 1.00000 to 1.20000, **3445
of 20000 (17%) are off by one tick** through the float path. `1.00002 *
100000.0` is `100001.99999999999`, and `as i64` truncates to `100001`.

That sweep is now a test. A 17% silent error rate is a much stronger argument
than one anecdote, and the anecdote happened to be false.

### 4.4 `rescale`'s guard must survive release builds

It was written as `debug_assert!`. The release test run caught it: the
should-panic test didn't panic, because the assertion had been compiled out.

Promoted to a real `assert!`. The check is two `u32`s and perfectly
predictable, while the failure it guards is silent, systematic price corruption
from one provider — the kind discovered months later as "that LP mysteriously
never wins any routing". Provider data is a trust boundary.

---

## 5. What the server taught us

### 5.1 `QUIET` had to become two flags

The throughput benchmark hung. The cause was the server behaving *correctly*:
clients pipelining 200k orders were still being sent their own `EV ACK` and
`EV FILL` events, weren't reading during the burst, filled the bounded 8192
queue, and were disconnected as slow consumers — `DESIGN.md` D25 working
exactly as designed.

The flaw was `QUIET`'s semantics. It suppressed acks but not lifecycle echoes,
and there was no way to express "submit as fast as possible without hearing
back".

Split into two flags, each with one meaning:

- `QUIET 1` — no `OK` acks
- `ECHO 0` — no order-lifecycle pushes to the submitter

Independent because the needs are independent: a benchmark wants neither, a
trading agent wants its fills but not an `OK` per command. Subscriptions
(`SUB`) remain a separate axis for market data.

### 5.2 The channel is the sequencer

`blox-server` is one reader thread per connection feeding a single `mpsc`
channel, consumed by one thread that owns the engine. That channel **is** the
sequencer from D4 — events race on the wire, the order they emerge is arbitrary
but recorded, and everything downstream is a pure fold over it.

This falls out of the core having no I/O rather than being designed in, which
is the main evidence the seam is in the right place.

---

## 6. What the Go simulator taught us

### 6.1 Counting trades per agent inflates the count

Trade events go to every subscriber. The first version counted them inside each
agent's drain loop, so with two subscribed momentum traders every trade was
counted twice — a bug that presents as a suspiciously fast engine.

Fixed with exactly one `Observer` that subscribes and counts. Agents track only
their own rejects.

### 6.2 A settle barrier must be on the counting connection

Pinging on a *separate* connection proves only that the engine drained its
input, not that a subscriber received the resulting pushes. The observer now
sends its own `PING settle` and its read loop watches for the matching `PONG`.

The first version also deadlocked: the observer goroutine was in the WaitGroup
but blocked on `ReadLine` and never saw the stop signal.

### 6.3 Most "rejects" were benign

The market sim reported ~5000 rejects per 50k orders, which looked alarming.
They were all `UnknownOrder`: market makers cancelling quotes that had already
been filled between deciding to cancel and the cancel arriving. Entirely
expected in a live market.

Now counted separately as `stale cancels`, leaving `rejects` at 0 — so a
non-zero value means something is actually wrong.

---

## 7. Deliberate omissions

Marked in the code with `ponytail:` comments naming the ceiling and the upgrade
path.

| Not built | Why | When to add |
|---|---|---|
| `aggregate` fast path | 1.7µs, but it is the on-demand query path; `top_of` (52ns) is the hot one | if aggregation moves onto the hot path |
| Tick-indexed bitmap levels | built, benchmarked, left switched off — `Vec` wins at FX depth (5–20 levels) and merely offering the choice cost 5–11%. `docs/LEVEL_BOOK.md` | past ~64 levels per side |
| FOK + STP interaction | FOK's pre-check ignores self-trade prevention, so an owner quoting both sides can pass it then under-fill | when a market-maker owner appears |
| Price amends | a price change is a cancel plus a new order; the caller can say that in two events | probably never |
| Intent log, risk gate, conflation | D13/D23–D26 — production concerns, not engine concerns | before the first live order |
| Margin, positions, P&L | app layer by design | when a host app needs them |
| Real provider adapters | need a chosen LP and its protocol | when the feed is chosen |
| Python/Node bindings | the FFI shape is designed (`apply_batch`), not built | when a host app exists |

---

## 8. Next

1. A real data-feed adapter — blocked on choosing a provider. Steps 1–3 of
   `docs/ADAPTERS.md` §6 need no network and can start from a captured sample.
2. Fill simulator with execution delay.
3. Margin, positions, stop-out — the largest remaining gap.
4. Whatever app-specific ranking or lifecycle logic the host app needs.
