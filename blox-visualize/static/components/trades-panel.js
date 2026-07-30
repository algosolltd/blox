'use strict';
// Scrolling market-trades tape. Self-contained: runs its own animation-frame
// loop to batch DOM writes and decay the trade-rate badge.
//
//   import { TradesPanel } from './components/trades-panel.js';
//   const trades = new TradesPanel(listEl, rateEl, { fmtPx, fmtQty, fmtTime });
//   trades.push({ price, qty, ts, side: 'B'|'S', mine });
//   trades.clear();
//   trades.destroy();

export class TradesPanel {
  constructor(listEl, rateEl, { fmtPx, fmtQty, fmtTime, maxRows = 250, renderMax = 90 }) {
    this.listEl = listEl;
    this.rateEl = rateEl;
    this.fmtPx = fmtPx;
    this.fmtQty = fmtQty;
    this.fmtTime = fmtTime;
    this.maxRows = maxRows;
    this.renderMax = renderMax;
    this.buf = [];
    this.times = [];
    this._raf = requestAnimationFrame(now => this._frame(now));
  }

  push(trade) {
    this.buf.push(trade);
    this.times.push(trade.ts);
  }

  clear() {
    this.listEl.textContent = '';
  }

  destroy() {
    cancelAnimationFrame(this._raf);
  }

  _frame(now) {
    this._flush();
    this._updateRate(now);
    this._raf = requestAnimationFrame(t => this._frame(t));
  }

  _flush() {
    if (!this.buf.length) return;
    // On a fast market the buffer outruns what any human can read: render the
    // newest slice and let the rest be represented elsewhere (candles etc.), not DOM.
    const show = this.buf.length > this.renderMax ? this.buf.slice(-this.renderMax) : this.buf;
    const frag = document.createDocumentFragment();
    const rows = [];
    for (const t of show) {
      const row = document.createElement('div');
      const up = t.side === 'B';
      row.className = 't-row mono ' + (up ? 'up' : 'down') + (t.mine ? ' mine' : '');
      row.innerHTML =
        `<span class="t-px"><span class="arr">${up ? '▲' : '▼'}</span>${this.fmtPx(t.price)}</span>` +
        `<span class="t-qty">${this.fmtQty(t.qty)}</span>` +
        `<span class="t-ts">${this.fmtTime(t.ts)}</span>`;
      frag.prepend(row); // newest ends up on top
      rows.push(row);
    }
    this.buf.length = 0;
    this.listEl.prepend(frag);
    while (this.listEl.children.length > this.maxRows) this.listEl.lastChild.remove();
    const newest = rows[rows.length - 1];
    if (newest) newest.animate(
      [{ backgroundColor: 'rgba(123,97,255,0.22)' }, { backgroundColor: 'transparent' }],
      { duration: 550, easing: 'ease-out' });
  }

  _updateRate(now) {
    const cutoff = now - 5000;
    while (this.times.length && this.times[0] < cutoff) this.times.shift();
    if (this.times.length > 1000) this.times = this.times.slice(-1000);
    const rate = this.times.length / 5;
    if (this.rateEl) this.rateEl.textContent = rate > 0.05 ? `${rate.toFixed(1)}/s` : '';
  }
}
