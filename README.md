# blox

A pure, deterministic, I/O-free order book and matching engine in Rust, plus a
thin server that owns the sockets.

No dependencies beyond `std` in the core. No clock, no threads, no randomness,
no floats — everything is a function of its inputs, which is what makes it
linkable from any language, testable without infrastructure, and replayable
after the fact.

```rust
use blox_core::*;

let mut eng = Engine::new();
let mut out = Vec::new();
let inst = InstrumentId(1);

// Someone rests an offer at 1.08510.
eng.apply_into(&Event::New {
    instrument: inst, id: 1, owner: 10,
    side: Side::Ask, price: 108_510, qty: 100, kind: OrderKind::Limit,
}, &mut out);

// Someone lifts it, bidding higher than they need to.
out.clear();
eng.apply_into(&Event::New {
    instrument: inst, id: 2, owner: 20,
    side: Side::Bid, price: 108_520, qty: 150, kind: OrderKind::Limit,
}, &mut out);

// They pay the maker's price — 108_510, not their own limit.
// The unfilled 50 rests as the new best bid.
```

## Shape

```
┌──────────────────────────────────────────────┐
│ blox-core        pure, no I/O, no clock       │
│   apply(event) -> [change]                    │
│   LevelBook · OrderBook · Triggers            │
└──────────────────────────────────────────────┘
        ▲                          ▲
        │ link directly            │ link directly
┌────────────────┐   ┌──────────────────────────────┐
│ Python / Node  │   │ blox-server                   │
│ games,         │   │  sockets, sequencer,          │
│ backtests      │   │  single-writer engine thread  │
└────────────────┘   └──────────────────────────────┘
                           ▲ TCP
        ┌──────────────────┴───────────────────┐
        │ blox-sim  (Go)                       │
        │  fake market + measurement           │
        │ blox-visualize  (Go)                 │
        │  browser dashboard: chart, trades,   │
        │  depth, order book over WebSocket    │
        └──────────────────────────────────────┘
```

## What it does

- **Two book types.** `LevelBook` mirrors a provider's aggregated depth;
  `OrderBook` is an authoritative venue book with price-time priority. One book
  per `(provider, instrument)`, merged on read with provider attribution.
- **Order kinds.** Limit, Market, IOC, FOK, PostOnly, plus self-trade
  prevention.
- **Conditional orders.** Stops, stop-limits, limits, take-profit/stop-loss
  with OCO. A gap fills at the market, never at the trigger price.
- **Integer ticks throughout.** No float ever touches a price — parsing a
  decimal via `f64` is wrong on ~17% of a realistic FX range.
- **Deterministic.** The same event log produces byte-identical output, in this
  process and any other. Verified across processes, because `HashMap`'s seed is
  per-process.

## Running it

```bash
cargo test --workspace        # 105 tests
cargo bench -p blox-core      # micro-benchmarks
cargo build --release

cd blox-sim && go build -o blox-sim .
./blox-sim                    # spawns a server, runs all three modes
```

`blox-sim` drives the engine with a synthetic market — a liquidity provider,
market makers, noise traders and momentum traders — and measures latency,
throughput, and whether the book survives concurrent load.

## Watching it

```bash
cd blox-visualize && go build -o blox-visualize .
./blox-visualize              # spawns server + sim, opens http://127.0.0.1:8080
```

A browser dashboard for the engine: candlestick chart, streaming trades,
depth chart and order book, fed by `blox-server`'s own TCP protocol over
WebSocket. No dependencies beyond the Go stdlib on the backend; the frontend
is a single page with a vendored copy of lightweight-charts.

It is also a trading terminal. The order-entry panel submits real orders
(Limit, Market, IOC, FOK, PostOnly) through the bridge's own engine
connection; the echoed lifecycle events drive open/closed order tables and a
live PnL ledger — the four-number netting position from `DESIGN.md` D20/D21,
tracked in integer ticks and marked to the aggregate mid. The PnL is the web
user's alone: the sim's agents trade on their own connections and never
touch the ledger. Your resting orders appear in the aggregate book; your
fills are flagged in the trade stream.

Useful flags: `-addr` to attach to an already-running server, `-no-sim` to
drive the market yourself (`nc 127.0.0.1 7070`), `-seed`, `-makers`/`-noise`/
`-momentum` to retune the synthetic market, `-listen` to move the UI.

## Numbers

| | |
|---|---|
| new limit order (engine) | 86 ns |
| top of book, 5 providers | 52 ns |
| 1000 parked triggers add | 33 ns |
| socket round trip | 2.7 µs |
| pipelined throughput | 1.17M events/sec |
| broker round trip | 20–40 ms |

The engine is ~350,000x faster than a broker round trip. That ratio is why
there is no SIMD, no cache-line padding, and no custom allocator here — see
`docs/BENCHMARKS.md`.

## Docs

What's implemented, matching this repo:

| | |
|---|---|
| `docs/CORE.md` | engine specification |
| `docs/CONDITIONAL_ORDERS.md` | stops, TP/SL, OCO |
| `docs/IMPLEMENTATION.md` | what was built, and what building it corrected |
| `docs/BENCHMARKS.md` | measured performance |

Design and internals — the target architecture (mostly unbuilt, see Status
below) and engineering records that aren't needed to use the library:

| | |
|---|---|
| `DESIGN.md` | the 26 decisions and why |
| `docs/ADAPTERS.md` | the adapter contract: live feeds, replay, sim, games (spec) |
| `docs/LEVEL_BOOK.md` | why level sides are a sorted `Vec` and not a bitmap |

## Status

The engine is built, tested and benchmarked. Not built yet: real provider
adapters (needs a chosen feed), margin and positions, and the app layer.
`docs/IMPLEMENTATION.md` §7 lists every deliberate omission with the condition
that should trigger building it.

## License

MIT — see [LICENSE](LICENSE). blox is free to use, modify, and ship in
personal or commercial projects, with no obligation beyond keeping the
copyright notice in the license text.

> **A small credit is appreciated, never required.** If blox ends up running
> under the hood of something you ship — especially something public or
> commercial — a line like *"Powered by [blox](https://github.com/algosolltd/blox)"*
> in your README, docs, or about page helps other people find their way back
> here. It costs you nothing and means a lot to whoever's maintaining this.
>
> A badge works just as well, if that's more your style:
>
> ```md
> [![Powered by blox](https://img.shields.io/badge/powered%20by-blox-7b61ff)](https://github.com/algosolltd/blox)
> ```
