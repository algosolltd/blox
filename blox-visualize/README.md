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

# blox-visualize

Trading panels you can drop on a page: framework-agnostic, build their own
DOM, and ship one feed adapter that speaks the wire protocol.

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

## Developing

```sh
npm install
npm run check   # typecheck + the feed's backfill/ordering self-check
npm run build   # compile src/ into dist/
```

## License

MIT. blox-visualize is free to use, modify, and ship in personal or
commercial projects, with no obligation beyond keeping the copyright notice
in the license text.

blox-visualize is built and maintained by
[Algorithmica Solutions](https://algorithmicasolutions.com).

> **A small credit is appreciated, never required.** If blox-visualize ends
> up running under the hood of something you ship — especially something
> public or commercial — a line like *"Powered by blox-visualize"* in your
> README, docs, or about page helps other people find their way back here.
