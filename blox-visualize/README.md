# blox-visualize

Trading panels you can drop on a page, plus the demo server that feeds them.

Two things live here:

- **`src/`** — the `blox-visualize` npm package. Framework-agnostic panels that
  build their own DOM, and one feed adapter that speaks the wire protocol.
- **`*.go` + `static/`** — a demo: a TCP→WebSocket bridge onto `blox-server`,
  serving a page that mounts the package.

## The package

```js
import { FeedAdapter, MarketChart, createPriceFormat } from "blox-visualize";

const fmt  = createPriceFormat(2);
const feed = new FeedAdapter("wss://example/ws");
const chart = new MarketChart(document.querySelector("#chart"), { fmt, title: "BLOX/USD" });

feed.subscribe({
  onHistory: (trades) => chart.seed(trades),   // backfill, oldest first
  onTrade:   (t)      => chart.addTrade(t),
  onOrders:  undefined,
});
feed.connect();
```

Or mount everything at once:

```js
import { TradingPanel, FeedAdapter } from "blox-visualize";
new TradingPanel(host, { feed: new FeedAdapter(url) });
```

Every panel is `new Panel(host, opts)` → `update(data)` → `destroy()`. The host
is one element; the panel fills it. Pass `title` to get a header, or `statusEl`
to send the status line into chrome you already draw.

Panels: `MarketChart` `OrderBook` `Depth` `Trades` `OrderEntry` `Orders` `PnlPanel`.

`Depth` hides levels further than `maxRangePct` from mid (default 1%) and says
so on the canvas when it does — one order resting far out would otherwise
stretch the axis across dead space and flatten the levels that matter. The
window is intersected with the real book, so a tighter book still auto-fits.
Pass `maxRangePct: 0` to draw the whole book.

Subpath imports (`blox-visualize/panels/chart`) pull in one panel and nothing
else — there are no side effects in any module body, so bundlers drop the rest.
`lightweight-charts` is an **optional** peer dep, used only by `MarketChart`: it
is taken from `window.LightweightCharts` if a script tag already loaded it,
otherwise imported on demand.

Import `blox-visualize/style.css` once.

## The wire

`{ type, payload }` frames over one WebSocket. `hello` → `history` → then
`update` / `account` / `status` / `notice` forever.

`history` is the backfill, and the adapter is what makes it safe: from the
moment the socket opens it holds live frames, replays `history` when it lands,
drains what it held, and only then streams. Without that, trades landing during
the backfill are either lost or applied out of order. A reconnect re-arms the
whole sequence and fires `onReset` first.

A server that sends no `history` frame is fine — a timeout releases the hold.

## Running the demo

```sh
npm install && npm run build:demo   # compile the package into static/lib
cargo build --release -p blox-server
(cd ../blox-sim && go build -o blox-sim .)
go build -o blox-visualize . && ./blox-visualize
```

`./blox-visualize` with no flags spawns `blox-server` and `blox-sim` and serves
the dashboard on <http://127.0.0.1:8080>. `-addr` attaches to a server you
already run, `-no-sim` leaves the market to you, `-history` sets how many trades
are retained and replayed to each new client (default 50000).

## Developing

```sh
npm run check       # typecheck + the feed's backfill/ordering self-check
npm run build:demo  # compile src/ into static/lib
./blox-visualize -static-dir static   # serve the front end off disk
```

`static/` is baked into the binary by `//go:embed`, so a front-end change is
invisible until you `go build` again — which is easy to forget and looks like
your edit did nothing. `-static-dir static` serves from disk instead; use it
while iterating, and rebuild the binary when you ship.

`static/app.js` is page glue — topbar, status bar, splitters — and deliberately
not part of the package.
