# BloX v2 — Engine Design

Outcome of the grilling session that picked Rust over Go (D5) and a trader's
engine over a venue's (D1).

> **This is the target design, not a status report.** The `blox-server` box
> below — execution client, risk gate, intent log, Postgres, WS/HTTP — is
> mostly unbuilt. What actually exists today: `docs/IMPLEMENTATION.md` §1
> (layout) and §7 (omission table), or the README's Status section.

Implementation specs:
- `docs/CORE.md` — types, both book structures, matching, aggregation, invariants, tests
- `docs/CONDITIONAL_ORDERS.md` — stops, stop-limits, TP/SL, OCO (core addition)
- `docs/ADAPTERS.md` — the adapter contract; live providers, replay, sim, games (also design, see its banner)

Build record:
- `docs/IMPLEMENTATION.md` — what exists, and the four things building it corrected
- `docs/BENCHMARKS.md` — measured performance
- `docs/LEVEL_BOOK.md` — why level sides are a sorted `Vec` and not a bitmap

**Status:** D1–D12, D15–D17, D20–D22, D24–D25 are implemented in `blox-core`
and `blox-server`. D13 (intent log, reconciliation), D18–D19 (execution and
routing), D23 (idempotency) and D26 (risk gate) are production concerns and
deliberately unbuilt — see `docs/IMPLEMENTATION.md` §7.

## What it is

A spot-FX trading engine. Connects to N liquidity providers, each supplying M
instruments. Maintains a live order book per `(provider, instrument)`, merges
them into an aggregate view, routes orders to the best provider, and tracks the
resulting positions.

Not a venue. We are a **customer of markets**, not a market. The engine holds
positions; it does not pair other people's trades.

## Shape in one sentence

A pure, deterministic, I/O-free Rust library that any language can link, plus a
thin Rust server that owns the network and exposes it over WebSocket + HTTP.

```
┌────────────────────────────────────────────┐
│ blox-core          (pure, no I/O, no clock) │
│   apply(event) -> changes                   │
│   OrderBook | LevelBook | aggregate         │
└────────────────────────────────────────────┘
         ▲                        ▲
         │ links directly         │ links directly
┌────────────────┐   ┌──────────────────────────────────┐
│ Python / Node  │   │ blox-server         (tokio)      │
│ games,         │   │  ├─ provider adapters (in)       │
│ backtests,     │   │  ├─ execution client  (out)      │
│ simulations    │   │  ├─ position ledger              │
└────────────────┘   │  ├─ pre-trade risk gate          │
                     │  ├─ intent log (append-only)     │
                     │  └─ WS (data) + HTTP (commands)  │
                     └──────────────────────────────────┘
                              │ async, off hot path
                              ▼
                          PostgreSQL  (history, P&L)
```

---

## Decisions

| # | Decision | Chosen | Rejected |
|---|---|---|---|
| D1 | System role | Trader (customer of markets) | Venue; hybrid venue+hedging |
| D2 | Book granularity | One book per `(provider, instrument)` | One book per instrument with provider tags |
| D3 | Internal liquidity | `provider = "internal"`, uniform code path | Special-cased internal book |
| D4 | Determinism | Pure fold over a sequenced log; non-determinism confined to the sequencer | Concurrent mutable books |
| D5 | Language | **Rust** | Go — does not embed (runtime + GC + cgo cost) |
| D6 | Latency target | Low microseconds; wire is the bottleneck | Sub-10µs HFT (needs colocation) |
| D7 | I/O placement | None in core; server shell owns all sockets | Library owns its own connections |
| D8 | Event delivery | `apply()` **returns** changes | Callbacks / listeners across FFI |
| D9 | Batch API | `apply_batch(&[Event])` from day one | Per-event FFI crossing only |
| D10 | Money | Integer ticks (`i64`) | `f64`; arbitrary-precision decimal |
| D11 | Book types | Two: `OrderBook` (order-level) + `LevelBook` (aggregated depth) | One unified structure |
| D12 | Concurrency | Single-threaded core, `&mut self`; server shards by book | Locks / lock-free in core |
| D13 | Recovery | Local intent log **and** broker query, reconciled; mismatch halts | Broker-only; log-only |
| D14 | Persistence | Books disposable. Only positions + working orders are real. Postgres downstream. | Postgres on the hot path |
| D15 | Symbology | Canonical registry; adapters normalize symbol + scale on ingress | Provider symbols as-is |
| D16 | Canonical scale | Finest tick across providers; coarser providers multiply up | Coarsest tick (lossy division) |
| D17 | Asset scope | Spot FX only | CFDs / futures (contract-spec complexity) |
| D18 | Execution | Sequential sweep with price limit + attempt budget | Fire-once; parallel spray (double-fill) |
| D19 | Routing | Displayed price weighted by rolling reject rate | Pure best-displayed-price |
| D20 | Positions | Netting | Hedging (economic fiction; illegal for US retail) |
| D21 | Unrealized P&L | Derived on read | Stored |
| D22 | Client transport | WS for market data, HTTP for commands | Polling; WS for everything |
| D23 | Idempotency | Required key on order submission | Best-effort retry |
| D24 | Fan-out | Per-subscriber conflation (coalesce, not sample) | Broadcast every update |
| D25 | Slow consumers | Bounded buffer, then disconnect | Unbounded buffering |
| D26 | Risk | Five pre-trade checks in server + manual kill switch | None; checks in core (no clock) |

---

## Core API

No async, no I/O, no clock, no allocation surprises (D7–D9). The exact
signatures and the full `Event`/`Change` vocabularies are `docs/CORE.md` §2–3
— not repeated here, so there's one place they can't drift out of sync.

### Instrument registry

```toml
[EURUSD]
tick_scale = 5          # canonical: 1 tick = 0.00001
lot_scale  = 2

[EURUSD.providers]
lp_a = { symbol = "EURUSD",      price_scale = 5 }
lp_b = { symbol = "EUR/USD",     price_scale = 4 }   # x10 on ingress
lp_c = { symbol = "EURUSD.spot", price_scale = 5 }
```

Rescaling happens in the adapter. The core never sees a provider dialect.

### Position state

Four numbers per instrument. Netting.

```rust
net_qty      : i64   // signed; + long, - short
avg_price    : i64   // canonical ticks, open portion only
realized_pnl : i64
```

Rules:
- Trade **increases** the position → re-average `avg_price`.
- Trade **reduces** the position → realize against `avg_price`, leave it unchanged.
- `net_qty` hits 0 → reset `avg_price`.

Unrealized = `net_qty * (mark - avg_price)`. Never stored.

---

## Server responsibilities

Everything the core refuses to do.

- **Provider adapters** — one per LP. Owns the socket, parses their dialect,
  detects sequence gaps, requests snapshots, maps symbol + rescales price,
  emits canonical events. Broker weirdness is contained here and nowhere else.
- **Sequencer** — stamps `(seq, recv_ts, provider_seq)` on every inbound event.
  The only non-deterministic component, by design.
- **Intent log** — append-only file. One line written *before* any order leaves
  the building. Serves both replay/audit and crash recovery.
- **Execution client** — sequential sweep (D18), reject accounting (D19).
- **Position ledger** — D20/D21.
- **Risk gate** — every order passes, no bypass:

  ```
  max_order_qty          fat finger
  max_position_qty       runaway accumulation
  max_orders_per_sec     loop bug
  price_collar           bad decimal / stale signal
  book_staleness         dead feed
  ```

  Plus a manual kill switch: halts new orders, leaves positions untouched.
  Rejections are logged loudly with a reason — a silent drop is
  indistinguishable from a rejection at the client, and that ambiguity costs
  an afternoon.
- **Reconciliation** — on startup, replay the intent log *and* query each
  provider for positions and open orders. Diff. Equal → run. Different →
  **halt and alert a human.** Never auto-correct.
- **Client API** — WS snapshot-then-delta with per-subscriber conflation;
  HTTP POST for orders with idempotency keys.
- **Postgres writer** — async consumer of the log. Trading continues if
  Postgres is down.

---

## Repo layout — target

```
blox-core/       pure lib, zero deps beyond std
blox-server/     adapters, risk, ledger, API
bindings/py/     PyO3
bindings/node/   napi-rs
```

`bindings/*` don't exist yet (step 10 below). The current `blox-server` is a
synchronous `std`-only TCP line-protocol server, not the tokio/WS/HTTP shell
described above — see `docs/IMPLEMENTATION.md` §1 for the real layout.

Go clients talk to `blox-server` over the socket — cgo makes linking the wrong
choice there.

---

## Build order

Each step working before the next. Steps 1–4 are the whole system in miniature.

1. `blox-core`: `OrderBook`, `LevelBook`, integer ticks, `apply`. Unit tests only.
2. One provider adapter. One instrument. Prices arriving, book populated.
3. Send one order. Handle ack. Handle reject.
4. Position tracking + P&L.
5. Second provider — aggregation and routing earn their keep or don't.
6. Intent log + replay + reconciliation.
7. Risk gate + kill switch. **Before the first live order.**
8. WS/HTTP API.
9. Postgres writer.
10. Python/Node bindings.

---

## Deliberately excluded

- **Venue / matching between our own users.** Different system; would drag in a
  risk engine. `OrderBook` exists in the core, so it stays possible.
- **Hedging positions.** Display concern over a netted engine, if ever needed.
- **Sub-10µs matching.** Requires colocation. The wire is ~10,000x slower than
  the engine; optimising further buys nothing.
- **Kubernetes.** Single box, systemd, reverse proxy. Revisit at multi-region.
- **Parallel order spray.** Double-fill risk, and it gets your flow flagged as
  toxic.

## Open

Not resolved in the session; needed before step 2.

- Which LPs first, and their protocols (FIX 4.4? proprietary WS? REST?).
- Auth / multi-tenancy on `blox-server` — single-user or many?
- Swap / rollover accrual at 5pm NY — in scope for P&L, or ignored?
- Where strategies run: inside `blox-server`, or as external HTTP clients?
- Postgres schema for history.
