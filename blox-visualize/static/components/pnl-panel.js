'use strict';
// Live PnL readout + rolling sparkline. Self-contained: samples once a
// second on its own timer so the line keeps moving even between updates.
//
//   import { PnlPanel } from './components/pnl-panel.js';
//   const pnl = new PnlPanel(els, sparkCanvas, { fmtMoney, fmtPx, fmtQty });
//   pnl.update({ total, realized, unrealized, pos, avg, mark, volume, fills });
//   pnl.destroy();

export class PnlPanel {
  constructor(els, sparkCanvas, { fmtMoney, fmtPx, fmtQty, maxSamples = 240 }) {
    this.els = els;
    this.spark = sparkCanvas;
    this.fmtMoney = fmtMoney;
    this.fmtPx = fmtPx;
    this.fmtQty = fmtQty;
    this.pnl = null;
    this.samples = [];
    this.maxSamples = maxSamples;
    this.ro = new ResizeObserver(() => this._drawSparkline());
    this.ro.observe(sparkCanvas.parentElement);
    this._timer = setInterval(() => this._sample(), 1000);
  }

  destroy() {
    clearInterval(this._timer);
    this.ro.disconnect();
  }

  update(pnl) {
    this.pnl = pnl;
    this._render();
  }

  _sample() {
    if (!this.pnl) return;
    this.samples.push(this.pnl.total);
    if (this.samples.length > this.maxSamples) this.samples.shift();
    this._drawSparkline();
  }

  _render() {
    const p = this.pnl;
    if (!p) return;
    const { totalEl, realizedEl, unrealEl, posEl, avgEl, markEl, volEl, fillsEl } = this.els;
    const cls = p.total > 0 ? 'up' : p.total < 0 ? 'down' : '';
    totalEl.textContent = this.fmtMoney(p.total);
    totalEl.className = 'pnl-total mono ' + cls;
    realizedEl.textContent = this.fmtMoney(p.realized);
    realizedEl.className = 'r ' + (p.realized >= 0 ? 'up' : 'down');
    unrealEl.textContent = this.fmtMoney(p.unrealized);
    unrealEl.className = 'r ' + (p.unrealized >= 0 ? 'up' : 'down');
    posEl.textContent = p.pos === 0 ? 'flat'
      : `${p.pos > 0 ? '+' : '−'}${this.fmtQty(Math.abs(p.pos))} ${p.pos > 0 ? 'long' : 'short'}`;
    posEl.className = 'r ' + (p.pos > 0 ? 'up' : p.pos < 0 ? 'down' : '');
    avgEl.textContent = p.avg ? this.fmtPx(p.avg) : '—';
    markEl.textContent = p.mark ? this.fmtPx(p.mark) : '—';
    volEl.textContent = this.fmtQty(p.volume);
    if (fillsEl) fillsEl.textContent = p.fills ? `${p.fills} fills` : '';
  }

  _drawSparkline() {
    const cv = this.spark;
    const w = cv.clientWidth, h = cv.clientHeight;
    if (!w || !h) return;
    const dpr = window.devicePixelRatio || 1;
    if (cv.width !== w * dpr || cv.height !== h * dpr) {
      cv.width = w * dpr; cv.height = h * dpr;
    }
    const ctx = cv.getContext('2d');
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, w, h);
    const s = this.samples;
    if (s.length < 2) return;
    let lo = Math.min(0, ...s), hi = Math.max(0, ...s);
    if (hi - lo < 1) { hi += 1; lo -= 1; }
    const max = this.maxSamples;
    const X = i => i / (max - 1) * w;
    const Y = v => 3 + (1 - (v - lo) / (hi - lo)) * (h - 6);
    // zero line
    ctx.strokeStyle = 'rgba(135,128,159,0.35)';
    ctx.setLineDash([3, 4]);
    ctx.beginPath(); ctx.moveTo(0, Y(0) + 0.5); ctx.lineTo(w, Y(0) + 0.5); ctx.stroke();
    ctx.setLineDash([]);
    // area + line
    const last = s[s.length - 1];
    const rgb = last >= 0 ? '46,189,133' : '246,70,93';
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
