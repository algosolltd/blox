'use strict';
// Two-sided order book ladder with depth bars. Self-contained: sizes its own
// rows to fit the container and batches repaints to one per animation frame.
//
//   import { OrderBookPanel } from './components/order-book.js';
//   const book = new OrderBookPanel(
//     { asksEl, bidsEl, spreadEl, depthCountEl },
//     { fmtPx, fmtQty },
//   );
//   book.update({ bids: [[price, qty], ...], asks: [...] });

export class OrderBookPanel {
  constructor(els, { fmtPx, fmtQty }) {
    this.els = els;
    this.fmtPx = fmtPx;
    this.fmtQty = fmtQty;
    this.book = { bids: [], asks: [] };
    this.askRows = [];
    this.bidRows = [];
    this.rowsPerSide = 0;
    this._scheduled = false;
    this._buildRows();
    this.ro = new ResizeObserver(() => this._buildRows());
    this.ro.observe(els.asksEl.parentElement);
  }

  update(book) {
    this.book = book;
    this._schedule();
  }

  destroy() {
    this.ro.disconnect();
  }

  _schedule() {
    if (this._scheduled) return;
    this._scheduled = true;
    requestAnimationFrame(() => { this._scheduled = false; this._render(); });
  }

  _buildRows() {
    const h = this.els.asksEl.parentElement.clientHeight;
    const want = Math.max(6, Math.min(40, Math.floor((h / 2 - 22) / 22)));
    if (want === this.rowsPerSide) return;
    this.rowsPerSide = want;
    const make = (host, cls) => {
      host.textContent = '';
      const rows = [];
      for (let i = 0; i < this.rowsPerSide; i++) {
        const row = document.createElement('div');
        row.className = `ob-row ${cls}`;
        row.innerHTML =
          `<div class="ob-bar"></div>` +
          (cls === 'ask'
            ? `<span class="ob-bq dim"></span><span class="ob-px"></span><span class="ob-aq"></span>`
            : `<span class="ob-bq"></span><span class="ob-px"></span><span class="ob-aq dim"></span>`);
        host.appendChild(row);
        rows.push({
          row,
          bar: row.children[0],
          qEl: row.children[cls === 'ask' ? 3 : 1],
          pxEl: row.children[2],
          q: -1, px: -1,
        });
      }
      return rows;
    };
    this.askRows = make(this.els.asksEl, 'ask');
    this.bidRows = make(this.els.bidsEl, 'bid');
    this._render();
  }

  _render() {
    const { bids, asks } = this.book;
    let cumB = 0, cumA = 0;
    const cB = bids.map(l => (cumB += l[1]));
    const cA = asks.map(l => (cumA += l[1]));
    const maxCum = Math.max(cumB, cumA, 1);

    const paint = (rows, levels, cums, isAsk) => {
      for (let k = 0; k < rows.length; k++) {
        const r = rows[k];
        // asks are stacked worst-at-top: top row shows the furthest level.
        const i = isAsk ? rows.length - 1 - k : k;
        const lvl = levels[i];
        if (!lvl) {
          if (r.px !== -1) { r.row.style.visibility = 'hidden'; r.px = -1; r.q = -1; }
          continue;
        }
        r.row.style.visibility = 'visible';
        const [px, q] = lvl;
        if (r.px !== px) { r.pxEl.textContent = this.fmtPx(px); r.px = px; }
        if (r.q !== q) { r.qEl.textContent = this.fmtQty(q); r.q = q; }
        r.bar.style.transform = `scaleX(${(cums[i] / maxCum).toFixed(4)})`;
      }
    };
    paint(this.askRows, asks, cA, true);
    paint(this.bidRows, bids, cB, false);

    const bb = bids[0], ba = asks[0];
    if (this.els.depthCountEl) this.els.depthCountEl.textContent = `${bids.length}×${asks.length}`;
    if (this.els.spreadEl) {
      if (bb && ba) {
        const sp = ba[0] - bb[0];
        const mid = (ba[0] + bb[0]) / 2;
        this.els.spreadEl.innerHTML =
          `<span>Spread</span><b>${this.fmtPx(sp)}</b><span>${(sp / mid * 100).toFixed(3)}%</span>`;
      } else {
        this.els.spreadEl.textContent = 'Spread —';
      }
    }
  }
}
