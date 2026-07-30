'use strict';
// Cumulative order-book depth chart, drawn on a <canvas>. Self-contained:
// owns its own resize handling and hover state.
//
//   import { DepthChart } from './components/depth-chart.js';
//   const chart = new DepthChart(canvasEl, tipEl, { fmtPx, fmtQty });
//   chart.render({ bids: [[price, qty], ...], asks: [...] });

export class DepthChart {
  constructor(canvas, tip, { fmtPx, fmtQty }) {
    this.cv = canvas;
    this.ctx = canvas.getContext('2d');
    this.tip = tip;
    this.fmtPx = fmtPx;
    this.fmtQty = fmtQty;
    this.hover = null;
    this.book = { bids: [], asks: [] };
    this.ro = new ResizeObserver(() => this._resize());
    this.ro.observe(canvas.parentElement);
    this._resize();
    canvas.addEventListener('mousemove', e => {
      const r = canvas.getBoundingClientRect();
      this.hoverX = e.clientX - r.left;
      this.hoverY = e.clientY - r.top;
      this._draw();
    });
    canvas.addEventListener('mouseleave', () => {
      this.hoverX = null;
      this.tip.style.display = 'none';
      this._draw();
    });
  }

  _resize() {
    const p = this.cv.parentElement;
    const dpr = window.devicePixelRatio || 1;
    this.w = p.clientWidth; this.h = p.clientHeight;
    this.cv.width = Math.max(1, this.w * dpr);
    this.cv.height = Math.max(1, this.h * dpr);
    this.ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    this._draw();
  }

  render(book) {
    this.book = book;
    // Batch: several book updates can land within one animation frame on a
    // busy market — only the last one before paint needs to hit the canvas.
    if (this._scheduled) return;
    this._scheduled = true;
    requestAnimationFrame(() => { this._scheduled = false; this._draw(); });
  }

  destroy() {
    this.ro.disconnect();
  }

  _draw() {
    const { ctx, w, h, fmtPx, fmtQty } = this;
    ctx.clearRect(0, 0, w, h);
    const { bids, asks } = this.book;
    if (!bids.length && !asks.length) {
      ctx.fillStyle = '#5b5476';
      ctx.font = '12px system-ui';
      ctx.textAlign = 'center';
      ctx.fillText('waiting for book…', w / 2, h / 2);
      return;
    }

    // Cumulative depth, best → worst.
    const cum = lvls => {
      let c = 0;
      return lvls.map(([p, q]) => [p, (c += q)]);
    };
    const B = cum(bids), A = cum(asks);
    const maxCum = Math.max(B.length ? B[B.length - 1][1] : 0, A.length ? A[A.length - 1][1] : 0, 1);

    const bb = B.length ? B[0][0] : null, ba = A.length ? A[0][0] : null;
    const mid = bb != null && ba != null ? (bb + ba) / 2 : (bb ?? ba ?? 0);
    const lo = (B.length ? B[B.length - 1][0] : mid) - 1;
    const hi = (A.length ? A[A.length - 1][0] : mid) + 1;
    const pad = Math.max((hi - lo) * 0.05, 1);
    const pLo = lo - pad, pHi = hi + pad;

    const padT = 14, padB = 22, padR = 44, padL = 6;
    const X = p => padL + (p - pLo) / (pHi - pLo) * (w - padL - padR);
    const Y = c => padT + (1 - c / (maxCum * 1.06)) * (h - padT - padB);

    // Grid: horizontals with qty labels, verticals with price labels.
    ctx.strokeStyle = 'rgba(120,110,160,0.10)';
    ctx.fillStyle = '#5b5476';
    ctx.font = '10px ui-monospace, Menlo, monospace';
    ctx.lineWidth = 1;
    ctx.textAlign = 'left';
    for (let i = 1; i <= 4; i++) {
      const c = maxCum * i / 4, y = Math.round(Y(c)) + 0.5;
      ctx.beginPath(); ctx.moveTo(padL, y); ctx.lineTo(w - padR, y); ctx.stroke();
      ctx.fillText(fmtQty(c), w - padR + 6, y + 3);
    }
    ctx.textAlign = 'center';
    const nTicks = 5;
    for (let i = 0; i <= nTicks; i++) {
      const p = pLo + (pHi - pLo) * i / nTicks;
      const x = X(p);
      ctx.strokeStyle = 'rgba(120,110,160,0.06)';
      ctx.beginPath(); ctx.moveTo(Math.round(x) + 0.5, padT); ctx.lineTo(Math.round(x) + 0.5, h - padB); ctx.stroke();
      // Keep labels inside the plot: the edge ticks would otherwise clip.
      const lx = Math.max(padL + 18, Math.min(w - padR - 18, x));
      ctx.fillText(fmtPx(Math.round(p)), lx, h - 8);
    }

    const area = (pts, dir, rgb) => {
      if (!pts.length) return;
      const x0 = X(pts[0][0]);
      ctx.beginPath();
      ctx.moveTo(x0, Y(0));
      ctx.lineTo(x0, Y(pts[0][1]));
      for (let i = 1; i < pts.length; i++) {
        ctx.lineTo(X(pts[i][0]), Y(pts[i - 1][1])); // step out
        ctx.lineTo(X(pts[i][0]), Y(pts[i][1]));     // step up
      }
      const xEnd = X(dir < 0 ? pLo : pHi);
      ctx.lineTo(xEnd, Y(pts[pts.length - 1][1]));
      ctx.lineTo(xEnd, Y(0));
      ctx.closePath();
      const g = ctx.createLinearGradient(0, padT, 0, h - padB);
      g.addColorStop(0, `rgba(${rgb},0.34)`);
      g.addColorStop(1, `rgba(${rgb},0.02)`);
      ctx.fillStyle = g;
      ctx.fill();
      // Crisp line along the stepped top edge.
      ctx.beginPath();
      ctx.moveTo(x0, Y(pts[0][1]));
      for (let i = 1; i < pts.length; i++) {
        ctx.lineTo(X(pts[i][0]), Y(pts[i - 1][1]));
        ctx.lineTo(X(pts[i][0]), Y(pts[i][1]));
      }
      ctx.lineTo(xEnd, Y(pts[pts.length - 1][1]));
      ctx.strokeStyle = `rgba(${rgb},0.95)`;
      ctx.lineWidth = 1.5;
      ctx.stroke();
    };
    area(B, -1, '46,189,133');
    area(A, 1, '246,70,93');

    // Mid line.
    if (bb != null && ba != null) {
      const x = Math.round(X(mid)) + 0.5;
      ctx.strokeStyle = 'rgba(135,128,159,0.5)';
      ctx.setLineDash([4, 4]);
      ctx.beginPath(); ctx.moveTo(x, padT); ctx.lineTo(x, h - padB); ctx.stroke();
      ctx.setLineDash([]);
    }

    // Hover crosshair + tooltip.
    if (this.hoverX != null) {
      const p = pLo + (this.hoverX - padL) / (w - padL - padR) * (pHi - pLo);
      if (p >= pLo && p <= pHi) {
        const cumAt = (pts, keep) => {
          let c = 0;
          for (const [lp, lq] of pts) { if (!keep(lp)) break; c += lq; }
          return c;
        };
        const cb = cumAt(B, lp => lp >= p);
        const ca = cumAt(A, lp => lp <= p);
        const x = Math.round(X(p)) + 0.5;
        ctx.strokeStyle = 'rgba(117,134,150,0.6)';
        ctx.setLineDash([3, 3]);
        ctx.beginPath(); ctx.moveTo(x, padT); ctx.lineTo(x, h - padB); ctx.stroke();
        ctx.setLineDash([]);
        const dot = (c, rgb) => {
          if (c <= 0) return;
          ctx.beginPath();
          ctx.arc(X(p), Y(c), 3.2, 0, Math.PI * 2);
          ctx.fillStyle = `rgba(${rgb},1)`;
          ctx.fill();
          ctx.strokeStyle = '#1c1729';
          ctx.lineWidth = 1.5;
          ctx.stroke();
        };
        dot(cb, '46,189,133');
        dot(ca, '246,70,93');

        this.tip.style.display = 'block';
        this.tip.innerHTML =
          `<span class="p">${fmtPx(Math.round(p))}</span><br>` +
          `<span class="b">bid ${fmtQty(cb)}</span><br>` +
          `<span class="a">ask ${fmtQty(ca)}</span>`;
        const tw = this.tip.offsetWidth;
        this.tip.style.left = Math.min(Math.max(4, this.hoverX + 14), w - tw - 4) + 'px';
        this.tip.style.top = Math.max(4, this.hoverY - 48) + 'px';
      }
    }
  }

  // Spread text for the panel header — original wrote this straight into a
  // dedicated element from inside render(); callers now read it back.
  spreadText() {
    const bb = this.book.bids[0], ba = this.book.asks[0];
    if (!bb || !ba) return '';
    const sp = ba[0] - bb[0];
    const mid = (ba[0] + bb[0]) / 2;
    return `${this.fmtPx(sp)} · ${(sp / mid * 100).toFixed(3)}%`;
  }
}
