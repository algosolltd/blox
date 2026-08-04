```
 $$$$$$\  $$\                               $$\   $$\     $$\                     $$\                    
$$  __$$\ $$ |                              \__|  $$ |    $$ |                    \__|                   
$$ /  $$ |$$ | $$$$$$\   $$$$$$\   $$$$$$\  $$\ $$$$$$\   $$$$$$$\  $$$$$$\$$$$\  $$\  $$$$$$$\ $$$$$$\  
$$$$$$$$ |$$ |$$  __$$\ $$  __$$\ $$  __$$\ $$ |\_$$  _|  $$  __$$\ $$  _$$  _$$\ $$ |$$  _____|\____$$\ 
$$  __$$ |$$ |$$ /  $$ |$$ /  $$ |$$ |  \__|$$ |  $$ |    $$ |  $$ |$$ / $$ / $$ |$$ |$$ /      $$$$$$$ |
$$ |  $$ |$$ |$$ |  $$ |$$ |  $$ |$$ |      $$ |  $$ |$$\ $$ |  $$ |$$ | $$ | $$ |$$ |$$ |     $$  __$$ |
$$ |  $$ |$$ |\$$$$$$$ |\$$$$$$  |$$ |      $$ |  \$$$$  |$$ |  $$ |$$ | $$ | $$ |$$ |\$$$$$$$\\$$$$$$$ |
\__|  \__|\__| \____$$ | \______/ \__|      \__|   \____/ \__|  \__|\__| \__| \__|\__| \_______|\_______|
              $$\   $$ |                                                                                 
              \$$$$$$  |                                                                                 
               \______/                                                                                  
 $$$$$$\            $$\             $$\     $$\                                                          
$$  __$$\           $$ |            $$ |    \__|                                                         
$$ /  \__| $$$$$$\  $$ |$$\   $$\ $$$$$$\   $$\  $$$$$$\  $$$$$$$\   $$$$$$$\                            
\$$$$$$\  $$  __$$\ $$ |$$ |  $$ |\_$$  _|  $$ |$$  __$$\ $$  __$$\ $$  _____|                           
 \____$$\ $$ /  $$ |$$ |$$ |  $$ |  $$ |    $$ |$$ /  $$ |$$ |  $$ |\$$$$$$\                             
$$\   $$ |$$ |  $$ |$$ |$$ |  $$ |  $$ |$$\ $$ |$$ |  $$ |$$ |  $$ | \____$$\                            
\$$$$$$  |\$$$$$$  |$$ |\$$$$$$  |  \$$$$  |$$ |\$$$$$$  |$$ |  $$ |$$$$$$$  |                           
 \______/  \______/ \__| \______/    \____/ \__| \______/ \__|  \__|\_______/                            
```

[algorithmicasolutions.com](https://algorithmicasolutions.com)

# blox-core

A pure, deterministic, I/O-free order book and matching engine, in Rust.

No sockets, no clock, no threads, no randomness, no floats. Everything is a
function of its inputs — `apply(event) -> [change]` — which is what makes it
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

## Layout

| Module | Role |
|---|---|
| `types` | primitives — `Ticks`, `Lots`, `Side`, `Instrument` |
| `event` | the two input vocabularies and the output vocabulary |
| `level_book` | provider replicas — aggregated depth, no identity |
| `order_book` | authoritative books — price-time priority |
| `triggers` | conditional orders — stops, TP/SL, OCO |
| `engine` | the container, aggregation, change suppression |
| `decimal` | integer decimal parsing for adapter boundaries |
| `testkit` | seeded workload generation for tests and benchmarks |

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

## The rules that must not be broken

Violating any of these silently breaks replay, and replay is how everything
else gets debugged.

- No `std::time`. Time arrives as `Event::Tick`.
- No `f64`. Prices and quantities are integers.
- No `HashMap` iteration reaching the output. Lookup is fine; ordering is
  not. Sort by key, or keep a parallel sorted `Vec`.
- No `rand`. Seed a generator in the caller and pass values in as events.
- No threads or `async`. The engine is `&mut self` and single-owner.

## Numbers

| | |
|---|---|
| new limit order | 86 ns |
| top of book, 5 providers | 52 ns |
| 1000 parked triggers add | 33 ns |

See `docs/BENCHMARKS.md` for methodology.

## Running it

```bash
cargo test        # unit + determinism + fuzz + replay tests
cargo bench       # micro-benchmarks
```

## Docs

| | |
|---|---|
| `docs/CORE.md` | engine specification |
| `docs/CONDITIONAL_ORDERS.md` | stops, TP/SL, OCO |
| `docs/IMPLEMENTATION.md` | what was built, and what building it corrected |
| `docs/BENCHMARKS.md` | measured performance |
| `docs/LEVEL_BOOK.md` | why level sides are a sorted `Vec` and not a bitmap |

## Status

The engine is built, tested and benchmarked. Not built yet: real provider
adapters (needs a chosen feed), margin and positions, and the app layer.
`docs/IMPLEMENTATION.md` §7 lists every deliberate omission with the condition
that should trigger building it.

## License

MIT — see [LICENSE](../LICENSE). blox-core is free to use, modify, and ship
in personal or commercial projects, with no obligation beyond keeping the
copyright notice in the license text.

blox-core is built and maintained by
[Algorithmica Solutions](https://algorithmicasolutions.com).

> **A small credit is appreciated, never required.** If blox-core ends up
> running under the hood of something you ship — especially something public
> or commercial — a line like *"Powered by blox-core"* in your README, docs,
> or about page helps other people find their way back here.
