// Time & sales tape. Trades queue and flush on the next frame, so a burst of
// a thousand ticks costs one DOM write of at most `renderMax` rows.

import type { PriceFormat } from "../format.js";
import type { TradeTick } from "../feed.js";
import { fmtQty, fmtTime } from "../format.js";
import { mount, q, type PanelChrome } from "../dom.js";

export type TradesOptions = PanelChrome & {
  fmt: PriceFormat;
  /** Rows kept in the DOM. */
  maxRows?: number;
  /** Rows built from a single flush — a burst never renders more than this. */
  renderMax?: number;
};

/**
 * Time & sales tape, with a trade-rate readout and your own fills
 * highlighted (`TradeTick.mine`).
 *
 * `new Trades(host, opts)`, then `seed(trades)` for backfill and `push(t)`
 * per live trade. `destroy()` when done.
 */
export class Trades {
  private readonly listEl: HTMLElement;
  private readonly statusEl: HTMLElement | null;
  private readonly fmt: PriceFormat;
  private readonly maxRows: number;
  private readonly renderMax: number;

  private buf: TradeTick[] = [];
  private times: number[] = [];
  private raf = 0;

  constructor(host: HTMLElement, opts: TradesOptions) {
    this.fmt = opts.fmt;
    this.maxRows = opts.maxRows ?? 250;
    this.renderMax = opts.renderMax ?? 90;
    this.statusEl = mount(host, `
      <div class="cols mono muted"><span>Price</span><span class="r">Qty</span><span class="r">Time</span></div>
      <div class="rows"></div>`, opts);
    this.listEl = q(host, ".rows");
  }

  /** One live trade. */
  push(t: TradeTick): void {
    this.buf.push(t);
    this.times.push(t.ts);
    this.schedule();
  }

  /** Bulk backfill, oldest first. */
  seed(trades: TradeTick[]): void {
    for (const t of trades) this.push(t);
  }

  /** Clear the tape — a reconnect, since the old trades may be stale. */
  reset(): void {
    this.buf.length = 0;
    this.times.length = 0;
    this.listEl.textContent = "";
    if (this.statusEl) this.statusEl.textContent = "";
  }

  /** Cancel the pending frame. */
  destroy(): void {
    if (this.raf) cancelAnimationFrame(this.raf);
  }

  private schedule(): void {
    if (!this.raf) this.raf = requestAnimationFrame(this.frame);
  }

  // An idle tape used to hold a 60fps loop open forever. Keep ticking only
  // while the 5s rate window still has samples left to expire.
  private frame = (now: number) => {
    this.raf = 0;
    this.flush();
    this.updateRate(now);
    if (this.times.length) this.schedule();
  };

  private flush(): void {
    if (!this.buf.length) return;
    const show = this.buf.length > this.renderMax ? this.buf.slice(-this.renderMax) : this.buf;
    const frag = document.createDocumentFragment();
    let newest: HTMLDivElement | null = null;
    for (const t of show) {
      const row = document.createElement("div");
      const up = t.side === "B";
      row.className = "t-row mono " + (up ? "up" : "down") + (t.mine ? " mine" : "");
      row.innerHTML =
        `<span class="t-px"><span class="arr">${up ? "▲" : "▼"}</span>${this.fmt.fmtPx(t.price)}</span>` +
        `<span class="t-qty">${fmtQty(t.qty)}</span>` +
        `<span class="t-ts">${fmtTime(t.ts)}</span>`;
      frag.prepend(row);
      newest = row;
    }
    this.buf.length = 0;
    this.listEl.prepend(frag);
    while (this.listEl.children.length > this.maxRows) this.listEl.lastChild?.remove();
    newest?.animate(
      [{ backgroundColor: "rgba(0,255,136,0.22)" }, { backgroundColor: "transparent" }],
      { duration: 550, easing: "ease-out" },
    );
  }

  private updateRate(now: number): void {
    const cutoff = now - 5000;
    // One splice, not a shift per sample — a history seed drops thousands of
    // out-of-window timestamps in a single pass.
    let i = 0;
    while (i < this.times.length && this.times[i] < cutoff) i++;
    if (i) this.times.splice(0, i);
    if (this.times.length > 1000) this.times.splice(0, this.times.length - 1000);
    const rate = this.times.length / 5;
    if (this.statusEl) this.statusEl.textContent = rate > 0.05 ? `${rate.toFixed(1)}/s` : "";
  }
}
