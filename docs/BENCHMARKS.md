# Benchmarks

Measured, not estimated. Reproduce with `cargo bench -p blox-core` and
`cd blox-sim && ./blox-sim`.

**Machine:** AMD Ryzen AI 9 HX PRO 370 (24 threads), Linux 7.0.10, rustc 1.96.0
release (`lto = true`, `codegen-units = 1`), Go 1.26.3. Loopback TCP.

The headline: **the engine is never the bottleneck, and it is not close.**
Everything below is in service of showing that precisely enough to stop
optimising it.

---

## 1. The engine alone

Single thread, no I/O. Best of 5 rounds; the minimum is the stablest estimate
when noise is additive.

| Operation | ns/op | ops/sec |
|---|---:|---:|
| `Delta`, existing level | 43 | 23.1M |
| `top_of_book`, 5 providers | 52 | 19.2M |
| `Cancel` | 74 | 13.6M |
| `Delta` with **1000 parked triggers** | 76 | 13.2M |
| `New` limit order, no cross | 86 | 11.7M |
| `Delta`, insert + remove level | 92 | 10.9M |
| Mixed realistic stream | 187 | 5.3M |
| `New` crossing 3 levels | 273 | 3.7M |
| `aggregate(depth=10)`, 5 providers | 1762 | 0.6M |

### Reading it

**Conditional orders are nearly free.** A thousand parked triggers add 33 ns to
a delta, and the cost does not grow with how many are parked — the four-bucket
structure means the check is four integer comparisons regardless. This was the
main thing worth verifying about `docs/CONDITIONAL_ORDERS.md`, because a naive
implementation that scans parked triggers would degrade exactly where it hurts.

**`aggregate` is 34x `top_of_book`, and that is fine.** One heap allocation per
level for its `sources` vector. `aggregate` runs on demand (a `BOOK` query);
`top_of_book` runs on *every event*. Optimising the cold path to tidy up a
table is the reflex this design exists to resist. The upgrade path is recorded
in a `ponytail:` comment at the call site.

**"Mixed realistic stream" (187 ns) is the honest average** — the generated
workload from `testkit`, mixing providers, orders, cancels, triggers, ticks and
disconnects. Single-operation numbers flatter; this one does not.

### First guesses vs measurements

`docs/CORE.md` §11 originally carried estimates written before any code existed.
Kept here because the pattern is instructive:

| Operation | Guessed | Measured | Ratio |
|---|---:|---:|---:|
| `Delta`, existing level | 20 | 43 | 2.2x |
| `Cancel` | 40 | 74 | 1.9x |
| `New` limit, no cross | 50 | 86 | 1.7x |
| `New` crossing 3 levels | 200 | 273 | 1.4x |
| `aggregate(depth=10)` | 300 | 1762 | **5.9x** |

Consistently 1.4–2.2x optimistic on the hot path, and 6x wrong on the one path
with per-item allocation. Nothing here changed a decision, which is itself the
finding: at these magnitudes the guesses were wrong and it did not matter.

---

## 2. Through the socket

`blox-sim` (Go) against `blox-server` over loopback. This is the number a
caller actually experiences.

### Latency — one command at a time

| | p50 | p90 | p99 | p99.9 | max |
|---|---:|---:|---:|---:|---:|
| `PING` (path floor) | 2.4µs | 2.8µs | 3.3µs | 5.2µs | 25µs |
| `TOP` query | 2.4µs | 2.8µs | 3.3µs | 5.9µs | 8.4µs |
| `NEW` order | 2.7µs | 3.9µs | 5.0µs | 13µs | 39.5µs |

`PING` does nothing but traverse Go → socket → sequencer → engine thread →
socket → Go. It is therefore the floor, and the gap to `NEW` (~310 ns) is what
the engine and protocol parse actually cost at this level.

**The socket is ~28x the engine.** A new order costs 86 ns inside the engine
and 2.7µs to ask for over a socket.

### Throughput — pipelined

4 connections × 200,000 events, `QUIET`+`ECHO 0`, `PING` barrier at the end:

```
800,000 events in 684ms
1.17M events/sec
850 ns per event
```

850 ns/event against 86 ns of engine work: **the shell costs ~10x the engine
even when pipelining amortises the syscalls.** Protocol parsing, the mpsc
channel, and the single-writer thread account for the rest.

Note this is a *single-writer* engine (D12) — one thread applying every event.
The 1.17M/s is one core's worth of the whole path, not a parallel number. The
design's answer to scaling is sharding by `(provider, instrument)`, not making
this loop faster.

---

## 3. The ratio that decides everything

```
engine, one order                        86 ns
loopback socket round trip            2,700 ns      31x
broker round trip (20-40ms)      30,000,000 ns   350,000x
```

A real broker round trip is **350,000x** the cost of matching an order.

Making the engine 10x faster moves end-to-end latency by 0.0003%. This is the
entire argument for `DESIGN.md` D6 (Profile B, not HFT), and the reason there
is no SIMD, no cache-line padding, and no custom allocator in this codebase. If
those were ever needed, the design would be wrong in bigger ways first —
you would need colocation, kernel bypass, and pinned cores, and the network
would still dominate.

---

## 4. Under a live market

8 seconds, 13 concurrent agents over 13 connections: 1 liquidity provider, 4
market makers, 6 noise traders, 2 momentum traders.

```
orders submitted     81,038   (10.1k/s)
cancels              10,966
provider snapshots   12,871   (1.6k/s)
trades matched       44,684   (5.6k/s)
volume              284,521 lots
stale cancels         8,953   (quote filled before the cancel landed)
rejects                   0
invariants           OK
```

**Rejects are 0 and that is the point.** The 8,953 stale cancels are market
makers cancelling quotes that filled between deciding to cancel and the cancel
arriving — expected in any live market. They are counted separately precisely
so that `rejects` staying at 0 means something.

**`CHECK` passes after every run.** All ten invariants from `docs/CORE.md` §8
hold after 100k+ events from 13 concurrent connections racing through the
sequencer. That is the property the whole single-writer design exists to
provide.

The rates here are agent-limited, not engine-limited: Go's tickers are coarse
at microsecond intervals, so the agents cannot generate load faster than
~10k orders/sec. Section 2 is the engine's actual ceiling.

---

## 5. What is not measured

Honest gaps, so nobody reads more into these numbers than they support.

- **Loopback only.** A real NIC adds 10–50µs and real jitter. That widens the
  gap between engine and network, so the conclusion holds harder.
- **One instrument, ≤5 providers.** Book counts stay small enough that every
  `HashMap` lookup hits cache. Thousands of instruments would change the
  constant, not the shape.
- **Shallow books.** FX depth is 5–20 levels, which is where `Vec` levels win.
  Past ~64 levels per side that inverts, and a tick-indexed bitmap takes over —
  measured both ways in `docs/LEVEL_BOOK.md`, built in `level_book.rs`, left
  switched off.
- **No margin, positions, or leaderboard.** Those are app-layer and unbuilt.
  A leaderboard in particular is likely to be the real hot path for a
  tournament-style host app, not matching.
- **Single-writer, one core.** No sharding is implemented, so nothing here says
  anything about horizontal scale.
