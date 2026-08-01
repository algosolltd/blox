> **Superseded — kept for the reasoning, not as instructions.** This plan has
> been carried out; see `README.md` for what actually shipped. Where the two
> disagree, the README is right. Notably: the library takes a container per
> panel rather than pre-built element refs, LightweightCharts is injected or
> imported on demand rather than loaded from a CDN, and the ring buffers,
> `__bench__/` and msgpack below were skipped as unjustified at this
> throughput. Paths to the tradearena repo in this document are also stale.

# Handoff: blox-visualize → modular library

## The ask (in the user's words)

> **Ultra modular system.**
> **Blazingly fast.**
> **Ultra efficient.**
> **Lowest memory footprint.**
> **Needs to be published as an npm package.**

Two repos, two responsibilities:

- **`blox-visualize/`** (this repo) → becomes a **library only**. Drop-in for any webpage. Imports the trading panel as a whole *or* individual components.
- **`blox-server/`** → becomes **minimum code that sends data to the charts**. No UI, no rendering, no static serving of front-end code. Just a WebSocket relay.

---

## What's already been discussed and decided

### Architecture (decisions we converged on)

1. **Drop-in library, two import styles.**
   - Whole panel: `import { TradingPanel, BLOX_PANELS } from "blox-visualize"`
   - Individual: `import { MarketChart, OrderBook } from "blox-visualize"`
   - Both must work, both must tree-shake correctly.

2. **Components stay vanilla DOM-mutating classes.** Same shape they have today (constructor takes a container, exposes `update()`, `setOpen()`, `setClosed()`). This is what makes them framework-agnostic and what keeps drag/resize at 60fps (no React reconciliation per pointermove). Already validated by the existing TypeScript port at `web/dashboard/app/trading/_lib/blox-viz.ts` in the tradearena repo.

3. **LightweightCharts stays a peer dependency.** It's 4× the size of everything else combined — bundling it makes "import the lib" cost ~120KB extra for everyone. Either auto-load it from CDN on first chart use, or require consumers to `<script>` it themselves. Auto-load is the recommended path.

4. **One FeedAdapter, not nine.** Every panel re-parsing the same WebSocket message is the worst perf trap. The FeedAdapter parses once and fans out to subscribers. Each panel subscribes to the channels it needs.

5. **Zero side effects in module bodies.** No top-level DOM mutations, no auto-registration. This is what makes tree-shaking work. The `package.json` declares `sideEffects: ["**/*.css"]` so CSS is the only side effect.

6. **The feed protocol is a typed message schema.** `ServerMessage` and `ClientMessage` discriminated unions over a WebSocket. JSON in v1, optional msgpack/protobuf later if profiling demands it.

### Performance constraints (the hard ones)

Two perf dimensions matter. Bundle size is not one of them.

**Runtime tick performance:**
- 10K ticks/sec/panel should be trivial on a modern laptop
- 9 panels at 100Hz = 900 events/sec, still trivial because the hot path is zero-allocation
- Pipeline: `WebSocket msg → JSON.parse once → typed object → fan out → primitive args to panels → rAF-batched canvas/DOM updates`

**Memory in browser:**
- Trading session = hours of continuous use. Memory must be flat over time, not growing.
- Every panel that holds history declares its cap in the constructor (the existing `CANDLES_MAX = 4000` in `MarketChart` is the model)
- Per-tick allocations are the killer — primitive args + ring buffers = no GC pressure
- Every `addEventListener` / `requestAnimationFrame` / `setTimeout` needs an explicit cleanup in `destroy()`

### The package shape (proposed)

```
blox-visualize/
├── package.json           # exports map, sideEffects, peerDeps
├── tsconfig.json
├── src/
│   ├── index.ts           # barrel: re-exports everything
│   ├── feed.ts            # FeedAdapter interface + synthetic impl
│   ├── format.ts          # fmtQty, fmtTime, createPriceFormat
│   ├── theme.ts           # CSS custom property defaults
│   ├── chart.ts           # MarketChart
│   ├── orderbook.ts       # OrderBookPanel
│   ├── depth.ts           # DepthChart
│   ├── trades.ts          # TradesPanel
│   ├── entry.ts           # OrderEntryForm
│   ├── orders.ts          # AccountOrdersPanel
│   ├── pnl.ts             # PnlPanel
│   ├── positions.ts       # PositionsPanel
│   ├── leaderboard.ts     # LeaderboardPanel
│   ├── panel.ts           # TradingPanel wrapper (all 9 together)
│   └── style.css          # single stylesheet, ~10KB
└── dist/                  # build output
```

`package.json` exports map (tree-shaking test cases below):
```json
{
  "exports": {
    ".": "./dist/index.js",
    "./chart": "./dist/chart.js",
    "./orderbook": "./dist/orderbook.js",
    "./depth": "./dist/depth.js",
    "./trades": "./dist/trades.js",
    "./entry": "./dist/entry.js",
    "./orders": "./dist/orders.js",
    "./pnl": "./dist/pnl.js",
    "./positions": "./dist/positions.js",
    "./leaderboard": "./dist/leaderboard.js",
    "./feed": "./dist/feed.js",
    "./format": "./dist/format.js",
    "./panel": "./dist/panel.js",
    "./style.css": "./dist/style.css"
  },
  "sideEffects": ["**/*.css"]
}
```

Bundle sizes (target):
| Import | Gzipped |
|---|---|
| `MarketChart` only | ~1KB |
| Single panel + format | ~1.5KB |
| All 9 panels | ~10KB |
| Everything + panel wrapper | ~12KB |
| LightweightCharts (peer dep) | ~45KB |

Per-panel memory budget (8-hour session, flat):
| Panel | Budget |
|---|---|
| MarketChart | ~3MB |
| OrderBookPanel | ~500KB |
| DepthChart | ~200KB |
| TradesPanel | ~300KB |
| OrderEntryForm | ~50KB |
| AccountOrdersPanel | ~200KB |
| PnlPanel | ~100KB |
| PositionsPanel | ~100KB |
| LeaderboardPanel | ~200KB |
| **Total all 9** | **~4.5MB** |

---

## The performance patterns (must-haves, copy these)

### 1. Primitive args, no allocations in the hot path
```ts
// ❌ object per tick = GC pressure at 100Hz over 8h
addTrade(tick: Tick): void

// ✅ primitive args = zero allocation per call
addTrade(price: number, qty: number, ts: number): void
```

### 2. Ring buffers, not arrays, for history
```ts
// ✅ pre-allocated, fixed memory, oldest-first eviction
class MarketChart {
  private readonly times = new Float64Array(4000);
  private readonly prices = new Float64Array(4000);
  private head = 0; private size = 0;

  addTrade(price: number, qty: number, ts: number) {
    const i = (this.head + this.size) % this.times.length;
    if (this.size < this.times.length) this.size++;
    else this.head = (this.head + 1) % this.times.length;
    this.times[i] = ts;
    this.prices[i] = price;
    this.scheduleDraw();
  }
}
```

### 3. rAF batching for redraws
```ts
private drawScheduled = false;
private scheduleDraw() {
  if (this.drawScheduled) return;
  this.drawScheduled = true;
  requestAnimationFrame(() => {
    this.drawScheduled = false;
    this.drawNow();  // drains all queued ticks in one frame
  });
}
```

### 4. Explicit `destroy()` on every panel
```ts
class MarketChart {
  destroy() {
    this.feedUnsubscribe?.();
    if (this.drawScheduled) cancelAnimationFrame(this.drawRaf);
    this.chart.remove();           // LightweightCharts cleanup
    this.wsObservers.forEach(unsub => unsub());
    this.wsObservers = [];
    // …any other listeners
  }
}
```

The consumer calls `panel.destroy()` on unmount. Forgetting this is the #1 memory leak source.

### 5. Single FeedAdapter, fan-out, parse once
```ts
class FeedAdapter {
  private ws: WebSocket;
  private subscribers = new Set<FeedListener>();

  connect() {
    this.ws = new WebSocket(URL);
    this.ws.onmessage = (e) => {
      const msg = JSON.parse(e.data);  // ONCE
      this.subscribers.forEach(s => s.onMessage(msg));
    };
  }

  subscribe(l: FeedListener): () => void {
    this.subscribers.add(l);
    return () => this.subscribers.delete(l);
  }
}
```

### 6. Backoff state machine on WebSocket reconnect
```ts
private reconnectAttempts = 0;
private reconnectTimer: number | null = null;

private scheduleReconnect() {
  if (this.destroyed) return;
  const delay = Math.min(60000, 1000 * 2 ** this.reconnectAttempts);
  this.reconnectAttempts++;
  this.reconnectTimer = setTimeout(() => this.connect(), delay);
}

destroy() {
  this.destroyed = true;
  if (this.reconnectTimer) clearTimeout(this.reconnectTimer);
  this.ws?.close();
  this.subscribers.clear();
}
```

---

## What still needs deciding (open questions)

The next agent should resolve these before writing code:

### Q1 — LightweightCharts auto-load or require pre-load?

- **Auto-load** (`<script>` injected from CDN on first chart mount, ~100ms cold-cache cost): "import and forget" wins, but adds CDN dependency and the auto-load logic.
- **Pre-load required** (consumer must `<script src="lightweight-charts">` themselves): simpler library, but every consumer makes the same mistake once.

**Recommendation: auto-load.** The CDN cost is amortized after first use, the DX win is large, and the alternative is one more thing consumers will get wrong.

If auto-load: build config needs the CDN URL as a configurable option (so consumers behind firewalls can swap it).

### Q2 — Feed adapter granularity

- **Granular**: `subscribe({ onTrade, onDepth, onAccount }): () => void` — each panel subscribes to what it needs.
- **Single**: `subscribe(onMessage): () => void` — every panel gets every message and routes internally.

**Recommendation: granular.** Less per-panel work for messages it doesn't care about. The cost is a slightly more verbose subscription call.

### Q3 — What ships in v1?

- **All 9 panels** + format + feed + panel wrapper.
- **Trading critical path only** (chart + book + entry + trades + positions) in v1, the rest in v1.1.

**Recommendation: critical path first.** Lets us validate the perf characteristics (especially memory) on the panels that get the most tick traffic before committing to the API contract for all 9.

### Q4 — What about the existing Go server?

- **Keep as standalone demo**: the Go server keeps its HTML/JS, blox-visualize becomes a separate codebase, both evolve independently.
- **Replace**: the Go server becomes just a WebSocket relay + thin HTML shell. The front-end JS comes from the npm bundle. The existing `static/components/*.js` files are deprecated in favor of the library.

**Recommendation: replace.** Two codebases doing the same thing will diverge. Pick one.

If replace: the existing `static/components/*.js` files (`market-chart.js`, `order-book.js`, etc.) become the *reference implementation* — port them to TypeScript with the patterns above, then build the npm package from those.

---

## Pointers to the source

- **`/home/jarno/hack/blox/blox-visualize/static/components/`** — the 9 vanilla JS panels + format + app glue. This is what to port.
- **`/home/jarno/hack/blox/blox-visualize/static/style.css`** — 589 lines of CSS that the panels depend on. Single stylesheet, ~10KB minified.
- **`/home/jarno/hack/algtradearena/web/dashboard/app/trading/_lib/blox-viz.ts`** — the existing TypeScript port (934 lines). Validates the conversion is mechanical. Look at this for how each class was wrapped.
- **`/home/jarno/hack/algtradearena/web/dashboard/app/trading/`** — the trading portal that uses the panels as widget bodies. Proves the panel → lib boundary works.
- **`/home/jarno/hack/blox/blox-server/`** — the Go server. If we go with Q4=replace, this becomes a WebSocket relay. If we keep it standalone, leave alone.

---

## Concrete next steps

For the agent picking this up:

1. **Resolve Q1-Q4** above. They're sequential: Q1 affects build config, Q2 affects every panel's subscription call, Q3 scopes the work, Q4 affects repo strategy.

2. **Write the v0.1 spec first.** Before any code, write:
   - The exact `package.json` shape (exports, sideEffects, peerDeps)
   - The `FeedAdapter` interface (TypeScript types only)
   - The `MarketChart` full API including `destroy()` and the memory cap
   - The message schema (ServerMessage / ClientMessage unions)
   - The CSS custom property defaults for theming

   This is the contract everything else hangs off of. Once it's written, the port is mechanical.

3. **Validate the perf claims with a benchmark.** Before declaring victory, build a `__bench__/` that:
   - Pushes 100K ticks through a `MarketChart` and measures time + memory before/after
   - Runs for 30 simulated minutes and asserts memory is flat (no growth after warmup)
   - Measures the "import one panel" vs "import all panels" bundle sizes with esbuild or Rollup

   Numbers in this document are estimates, not measurements. The benchmark is what turns "fast" into "verified fast."

4. **Keep the existing TS port (`blox-viz.ts`) as the reference.** Don't rewrite from scratch — port the TS to use the patterns above (ring buffers, primitive args, rAF batching, explicit `destroy()`), and that's the library.

---

## Constraints recap (for the next agent's pre-flight check)

Before shipping any code, the design must satisfy all five:

- [ ] **Ultra modular**: every panel is a separate `package.json` export, tree-shakable, importable individually or together, no top-level side effects.
- [ ] **Blazingly fast**: rAF-batched redraws, primitive args in hot path, single parse per message, target 10K ticks/sec/panel.
- [ ] **Ultra efficient**: zero allocations per tick in steady state, bounded buffers everywhere, no per-frame DOM reads.
- [ ] **Lowest memory footprint**: per-panel caps declared in constructor, `destroy()` tears everything down, session-stable heap (flat over 8h, ~4.5MB for all 9 panels).
- [ ] **npm package**: buildable with esbuild/Rollup, ESM-only, exports map, TypeScript types, peer dep on LightweightCharts (auto-loaded).

If any box can't be checked, the design isn't done yet.
