// Order book ladder. Rows are allocated once and rebuilt only when the panel
// is resized — a book update rewrites text and a bar transform, never DOM.

import type { PriceFormat } from "../format.js";
import type { Book, BookLevel } from "../feed.js";
import { fmtQty } from "../format.js";
import { mount, q, type PanelChrome } from "../dom.js";

type Row = { row: HTMLDivElement; bar: HTMLDivElement; qEl: HTMLElement; pxEl: HTMLElement; q: number; px: number };

export type OrderBookOptions = PanelChrome & { fmt: PriceFormat };

export class OrderBook {
  private readonly asksEl: HTMLElement;
  private readonly bidsEl: HTMLElement;
  private readonly spreadEl: HTMLElement;
  private readonly statusEl: HTMLElement | null;
  private readonly obEl: HTMLElement;
  private readonly fmt: PriceFormat;
  private readonly ro: ResizeObserver;

  private book: Book = { bids: [], asks: [] };
  private askRows: Row[] = [];
  private bidRows: Row[] = [];
  private rowsPerSide = 0;
  private raf = 0;

  constructor(host: HTMLElement, opts: OrderBookOptions) {
    this.fmt = opts.fmt;
    this.statusEl = mount(host, `
      <div class="cols mono muted ob-cols"><span>Bids</span><span>Price</span><span class="r">Asks</span></div>
      <div class="ob">
        <div class="ob-side ob-asks"></div>
        <div class="ob-spread mono">—</div>
        <div class="ob-side ob-bids"></div>
      </div>`, opts);
    this.obEl = q(host, ".ob");
    this.asksEl = q(host, ".ob-asks");
    this.bidsEl = q(host, ".ob-bids");
    this.spreadEl = q(host, ".ob-spread");

    this.buildRows();
    this.ro = new ResizeObserver(() => this.buildRows());
    this.ro.observe(this.obEl);
  }

  update(book: Book): void {
    this.book = book;
    if (!this.raf) this.raf = requestAnimationFrame(this.frame);
  }

  reset(): void {
    this.update({ bids: [], asks: [] });
  }

  destroy(): void {
    this.ro.disconnect();
    if (this.raf) cancelAnimationFrame(this.raf);
  }

  private frame = () => {
    this.raf = 0;
    this.render();
  };

  private buildRows(): void {
    const h = this.obEl.clientHeight;
    const want = Math.max(6, Math.min(40, Math.floor((h / 2 - 22) / 22)));
    if (want === this.rowsPerSide) return;
    this.rowsPerSide = want;
    const make = (hostEl: HTMLElement, cls: "ask" | "bid"): Row[] => {
      hostEl.textContent = "";
      const rows: Row[] = [];
      for (let i = 0; i < this.rowsPerSide; i++) {
        const row = document.createElement("div");
        row.className = `ob-row ${cls}`;
        row.innerHTML =
          `<div class="ob-bar"></div>` +
          (cls === "ask"
            ? `<span class="ob-bq dim"></span><span class="ob-px"></span><span class="ob-aq"></span>`
            : `<span class="ob-bq"></span><span class="ob-px"></span><span class="ob-aq dim"></span>`);
        hostEl.appendChild(row);
        rows.push({
          row,
          bar: row.children[0] as HTMLDivElement,
          qEl: row.children[cls === "ask" ? 3 : 1] as HTMLElement,
          pxEl: row.children[2] as HTMLElement,
          q: -1, px: -1,
        });
      }
      return rows;
    };
    this.askRows = make(this.asksEl, "ask");
    this.bidRows = make(this.bidsEl, "bid");
    this.render();
  }

  private render(): void {
    const { bids, asks } = this.book;
    let cumB = 0, cumA = 0;
    const cB = bids.map((l) => (cumB += l[1]));
    const cA = asks.map((l) => (cumA += l[1]));
    const maxCum = Math.max(cumB, cumA, 1);

    const paint = (rows: Row[], levels: BookLevel[], cums: number[], isAsk: boolean) => {
      for (let k = 0; k < rows.length; k++) {
        const r = rows[k];
        const i = isAsk ? rows.length - 1 - k : k;
        const lvl = levels[i];
        if (!lvl) {
          if (r.px !== -1) { r.row.style.visibility = "hidden"; r.px = -1; r.q = -1; }
          continue;
        }
        r.row.style.visibility = "visible";
        const [px, qty] = lvl;
        if (r.px !== px) { r.pxEl.textContent = this.fmt.fmtPx(px); r.px = px; }
        if (r.q !== qty) { r.qEl.textContent = fmtQty(qty); r.q = qty; }
        r.bar.style.transform = `scaleX(${(cums[i] / maxCum).toFixed(4)})`;
      }
    };
    paint(this.askRows, asks, cA, true);
    paint(this.bidRows, bids, cB, false);

    const bb = bids[0], ba = asks[0];
    if (this.statusEl) this.statusEl.textContent = `${bids.length}×${asks.length}`;
    if (bb && ba) {
      const sp = ba[0] - bb[0];
      const mid = (bb[0] + ba[0]) / 2;
      this.spreadEl.innerHTML =
        `<span>Spread</span><b>${this.fmt.fmtPx(sp)}</b><span>${(sp / mid * 100).toFixed(3)}%</span>`;
    } else {
      this.spreadEl.textContent = "Spread —";
    }
  }
}
