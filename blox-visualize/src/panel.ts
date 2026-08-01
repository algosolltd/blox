// The whole trading panel: every component mounted in a grid and wired to one
// feed. Use this to drop the lot onto a page; import the individual panels
// instead when the host owns its own layout.

import type { Feed, Book, Pnl } from "./feed.js";
import { createPriceFormat, type PriceFormat } from "./format.js";
import { MarketChart } from "./panels/chart.js";
import { OrderBook } from "./panels/orderbook.js";
import { Depth } from "./panels/depth.js";
import { Trades } from "./panels/trades.js";
import { OrderEntry } from "./panels/entry.js";
import { Orders } from "./panels/orders.js";
import { PnlPanel } from "./panels/pnl.js";

export const BLOX_PANELS = ["chart", "book", "depth", "trades", "entry", "orders", "pnl"] as const;
export type BloxPanelId = (typeof BLOX_PANELS)[number];

const TITLES: Record<BloxPanelId, string> = {
  chart: "Market chart",
  book: "Order book",
  depth: "Depth chart",
  trades: "Market trades",
  entry: "Order entry",
  orders: "Orders",
  pnl: "Live PnL",
};

export type TradingPanelOptions = {
  feed: Feed;
  /** Which panels to mount, in order. Defaults to all of them. */
  panels?: readonly BloxPanelId[];
  /** Overridden by the server's hello frame when they disagree. */
  priceScale?: number;
  symbol?: string;
  /** LightweightCharts namespace, forwarded to the chart. */
  charts?: any;
};

export class TradingPanel {
  private readonly host: HTMLElement;
  private readonly feed: Feed;
  private readonly ids: readonly BloxPanelId[];
  private readonly charts: any;

  private symbol: string;
  private scale: number;
  private book: Book = { bids: [], asks: [] };
  private orderId = 1;

  private chart: MarketChart | null = null;
  private bookPanel: OrderBook | null = null;
  private depth: Depth | null = null;
  private trades: Trades | null = null;
  private entry: OrderEntry | null = null;
  private orders: Orders | null = null;
  private pnl: PnlPanel | null = null;
  private unsubscribe: (() => void) | null = null;

  constructor(host: HTMLElement, opts: TradingPanelOptions) {
    this.host = host;
    this.feed = opts.feed;
    this.ids = opts.panels ?? BLOX_PANELS;
    this.charts = opts.charts;
    this.scale = opts.priceScale ?? 2;
    this.symbol = opts.symbol ?? "BLOX";
    this.build();

    this.unsubscribe = this.feed.subscribe({
      onHello: (h) => {
        // The server is authoritative about the instrument. A different price
        // scale changes every formatter, so the panels are rebuilt.
        const symbol = h.instrument.split("/")[0] || this.symbol;
        if (h.priceScale === this.scale && symbol === this.symbol) return;
        this.scale = h.priceScale;
        this.symbol = symbol;
        this.teardownPanels();
        this.build();
      },
      onReset: () => {
        this.book = { bids: [], asks: [] };
        this.chart?.reset();
        this.trades?.reset();
        this.bookPanel?.reset();
        this.depth?.reset();
        this.orders?.reset();
        this.pnl?.reset();
      },
      onHistory: (trades) => {
        this.chart?.seed(trades);
        this.trades?.seed(trades);
      },
      onTrade: (t) => {
        this.chart?.addTrade(t);
        this.trades?.push(t);
      },
      onBook: (b) => {
        this.book = b;
        this.bookPanel?.update(b);
        this.depth?.update(b);
        this.entry?.update(b);
      },
      onAccount: (a) => {
        // `full` means "this is the whole account" — so the lists are replaced
        // wholesale, including with empty ones. Testing `a.open` instead would
        // leave the last cancelled order on screen forever: a server that
        // omits empty arrays (Go's omitempty does) never sends the signal that
        // clears them.
        if (a.full) {
          const open = a.open ?? [];
          const closed = a.closed ?? [];
          this.orders?.update({ open, closed });
          // Resting orders become dashed lines; anything filled leaves a dot.
          this.chart?.setOrders(open);
          this.chart?.setFills(
            closed
              .filter((o) => o.filled > 0)
              .map((o) => ({ ts: o.tsEnd ?? o.ts, price: o.avgFill, side: o.side })),
          );
        }
        if (a.pnl) this.pnl?.update(a.pnl as Pnl);
      },
    });
  }

  destroy(): void {
    this.unsubscribe?.();
    this.unsubscribe = null;
    this.teardownPanels();
    this.host.textContent = "";
  }

  private build(): void {
    const fmt: PriceFormat = createPriceFormat(this.scale);
    this.host.classList.add("bx-grid");
    this.host.textContent = "";

    for (const id of this.ids) {
      const cell = document.createElement("section");
      cell.className = `bx-cell bx-cell-${id}`;
      this.host.appendChild(cell);
      const chrome = { title: TITLES[id] };
      switch (id) {
        case "chart":
          this.chart = new MarketChart(cell, { ...chrome, fmt, charts: this.charts });
          break;
        case "book":
          this.bookPanel = new OrderBook(cell, { ...chrome, fmt });
          break;
        case "depth":
          this.depth = new Depth(cell, { ...chrome, fmt });
          break;
        case "trades":
          this.trades = new Trades(cell, { ...chrome, fmt });
          break;
        case "entry":
          this.entry = new OrderEntry(cell, {
            ...chrome, fmt, symbol: this.symbol,
            getBook: () => this.book,
            onSubmit: (o) => this.feed.send({ type: "order", id: this.orderId++, ...o }),
          });
          break;
        case "orders":
          this.orders = new Orders(cell, {
            ...chrome, fmt,
            onCancel: (id2) => this.feed.send({ type: "cancel", id: id2 }),
            onReduce: (id2, qty) => this.feed.send({ type: "reduce", id: id2, qty }),
          });
          break;
        case "pnl":
          this.pnl = new PnlPanel(cell, { ...chrome, fmt });
          break;
      }
    }
  }

  private teardownPanels(): void {
    this.chart?.destroy(); this.chart = null;
    this.bookPanel?.destroy(); this.bookPanel = null;
    this.depth?.destroy(); this.depth = null;
    this.trades?.destroy(); this.trades = null;
    this.entry?.destroy(); this.entry = null;
    this.orders?.destroy(); this.orders = null;
    this.pnl?.destroy(); this.pnl = null;
  }
}
