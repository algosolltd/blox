// Live PnL readout with a sparkline of total PnL over time. The sparkline
// samples on an interval rather than on every update — PnL changes with the
// mark, which moves far faster than a 200px canvas can show.

import type { PriceFormat } from "../format.js";
import type { Pnl } from "../feed.js";
import { fmtCurrency, fmtQty } from "../format.js";
import { mount, q, type PanelChrome } from "../dom.js";

export type PnlOptions = PanelChrome & {
  fmt: PriceFormat;
  /** Sparkline width in samples. One sample per sampleMs. */
  maxSamples?: number;
  sampleMs?: number;
};

/**
 * Live PnL readout (realized, unrealized, position, avg open, mark, volume)
 * with a sparkline of total PnL over time.
 *
 * `new PnlPanel(host, opts)`, then `update(pnl)` on every account snapshot
 * that carries a `pnl` field. `destroy()` when done.
 */
export class PnlPanel {
  private readonly els: Record<string, HTMLElement>;
  private readonly spark: HTMLCanvasElement;
  private readonly fmt: PriceFormat;
  private readonly maxSamples: number;
  private readonly ro: ResizeObserver;
  private readonly timer: ReturnType<typeof setInterval>;

  private pnl: Pnl | null = null;
  private samples: number[] = [];

  constructor(host: HTMLElement, opts: PnlOptions) {
    this.fmt = opts.fmt;
    this.maxSamples = opts.maxSamples ?? 240;
    const statusEl = mount(host, `
      <div class="pnl">
        <div class="pnl-total mono">+$0.00</div>
        <div class="pnl-grid mono">
          <span class="muted">Realized</span><span class="pnl-realized r">—</span>
          <span class="muted">Unrealized</span><span class="pnl-unreal r">—</span>
          <span class="muted">Position</span><span class="pnl-pos r">—</span>
          <span class="muted">Avg open</span><span class="pnl-avg r">—</span>
          <span class="muted">Mark</span><span class="pnl-mark r">—</span>
          <span class="muted">Volume</span><span class="pnl-vol r">—</span>
        </div>
        <canvas class="pnl-spark"></canvas>
      </div>`, opts);

    this.els = {
      total: q(host, ".pnl-total"),
      realized: q(host, ".pnl-realized"),
      unreal: q(host, ".pnl-unreal"),
      pos: q(host, ".pnl-pos"),
      avg: q(host, ".pnl-avg"),
      mark: q(host, ".pnl-mark"),
      vol: q(host, ".pnl-vol"),
    };
    if (statusEl) this.els.fills = statusEl;
    this.spark = q<HTMLCanvasElement>(host, ".pnl-spark");

    this.ro = new ResizeObserver(() => this.drawSparkline());
    this.ro.observe(q(host, ".pnl"));
    this.timer = setInterval(() => this.sample(), opts.sampleMs ?? 1000);
  }

  /** A fresh PnL snapshot. */
  update(pnl: Pnl): void {
    this.pnl = pnl;
    this.render();
  }

  /** Clear the readout and sparkline — a reconnect, since the old numbers may be stale. */
  reset(): void {
    this.pnl = null;
    this.samples.length = 0;
    this.drawSparkline();
  }

  /** Stop the sparkline's sample timer and disconnect the resize observer. */
  destroy(): void {
    clearInterval(this.timer);
    this.ro.disconnect();
  }

  private sample(): void {
    if (!this.pnl) return;
    this.samples.push(this.pnl.total);
    if (this.samples.length > this.maxSamples) this.samples.shift();
    this.drawSparkline();
  }

  private render(): void {
    const p = this.pnl;
    if (!p) return;
    const cls = p.total > 0 ? "up" : p.total < 0 ? "down" : "";
    // PnL arrives in currency units, not ticks — fmtMoney would divide again.
    this.els.total.textContent = fmtCurrency(p.total);
    this.els.total.className = "pnl-total mono " + cls;
    this.els.realized.textContent = fmtCurrency(p.realized);
    this.els.realized.className = "pnl-realized r " + (p.realized >= 0 ? "up" : "down");
    this.els.unreal.textContent = fmtCurrency(p.unrealized);
    this.els.unreal.className = "pnl-unreal r " + (p.unrealized >= 0 ? "up" : "down");
    this.els.pos.textContent = p.pos === 0 ? "flat"
      : `${p.pos > 0 ? "+" : "−"}${fmtQty(Math.abs(p.pos))} ${p.pos > 0 ? "long" : "short"}`;
    this.els.pos.className = "pnl-pos r " + (p.pos > 0 ? "up" : p.pos < 0 ? "down" : "");
    this.els.avg.textContent = p.avg ? this.fmt.fmtPx(p.avg) : "—";
    this.els.mark.textContent = p.mark ? this.fmt.fmtPx(p.mark) : "—";
    this.els.vol.textContent = fmtQty(p.volume);
    if (this.els.fills) this.els.fills.textContent = p.fills ? `${p.fills} fills` : "";
  }

  private drawSparkline(): void {
    const cv = this.spark;
    const w = cv.clientWidth, h = cv.clientHeight;
    if (!w || !h) return;
    const dpr = window.devicePixelRatio || 1;
    if (cv.width !== w * dpr || cv.height !== h * dpr) {
      cv.width = w * dpr;
      cv.height = h * dpr;
    }
    const ctx = cv.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, w, h);
    const s = this.samples;
    if (s.length < 2) return;
    let lo = Math.min(0, ...s), hi = Math.max(0, ...s);
    if (hi - lo < 1) { hi += 1; lo -= 1; }
    const max = this.maxSamples;
    const X = (i: number) => i / (max - 1) * w;
    const Y = (v: number) => 3 + (1 - (v - lo) / (hi - lo)) * (h - 6);
    ctx.strokeStyle = "rgba(135,128,159,0.35)";
    ctx.setLineDash([3, 4]);
    ctx.beginPath(); ctx.moveTo(0, Y(0) + 0.5); ctx.lineTo(w, Y(0) + 0.5); ctx.stroke();
    ctx.setLineDash([]);
    const rgb = s[s.length - 1] >= 0 ? "0,255,136" : "255,61,87";
    ctx.beginPath();
    ctx.moveTo(X(max - s.length), Y(0));
    for (let i = 0; i < s.length; i++) ctx.lineTo(X(max - s.length + i), Y(s[i]));
    ctx.lineTo(X(max - 1), Y(0));
    ctx.closePath();
    const g = ctx.createLinearGradient(0, 0, 0, h);
    g.addColorStop(0, `rgba(${rgb},0.25)`);
    g.addColorStop(1, `rgba(${rgb},0.01)`);
    ctx.fillStyle = g;
    ctx.fill();
    ctx.beginPath();
    for (let i = 0; i < s.length; i++) {
      const x = X(max - s.length + i), y = Y(s[i]);
      i ? ctx.lineTo(x, y) : ctx.moveTo(x, y);
    }
    ctx.strokeStyle = `rgba(${rgb},0.95)`;
    ctx.lineWidth = 1.5;
    ctx.stroke();
  }
}
