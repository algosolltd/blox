// Cumulative depth curve on a canvas, with a hover readout. Redraws are
// frame-batched; the hover path redraws directly because it is already
// pointer-rate.
import { fmtQty } from "../format.js";
import { mount, q } from "../dom.js";
/**
 * Cumulative depth curve on a canvas, with a hover readout.
 *
 * `new Depth(host, opts)`, then `update(book)` on every book snapshot.
 * `destroy()` disconnects the resize observer and cancels the pending frame.
 */
export class Depth {
    cv;
    ctx;
    tip;
    wrap;
    statusEl;
    fmt;
    maxRangePct;
    ro;
    ac = new AbortController();
    book = { bids: [], asks: [] };
    hoverX = null;
    hoverY = 0;
    w = 0;
    h = 0;
    raf = 0;
    constructor(host, opts) {
        this.fmt = opts.fmt;
        this.maxRangePct = opts.maxRangePct ?? 1;
        this.statusEl = mount(host, `
      <div class="depth-wrap">
        <canvas class="depth-cv"></canvas>
        <div class="tip mono"></div>
      </div>`, opts);
        this.wrap = q(host, ".depth-wrap");
        this.cv = q(host, ".depth-cv");
        this.tip = q(host, ".tip");
        this.ctx = this.cv.getContext("2d");
        const signal = this.ac.signal;
        this.cv.addEventListener("mousemove", (e) => {
            const r = this.cv.getBoundingClientRect();
            this.hoverX = e.clientX - r.left;
            this.hoverY = e.clientY - r.top;
            this.draw();
        }, { signal });
        this.cv.addEventListener("mouseleave", () => {
            this.hoverX = null;
            this.tip.style.display = "none";
            this.draw();
        }, { signal });
        this.ro = new ResizeObserver(() => this.resize());
        this.ro.observe(this.wrap);
        this.resize();
    }
    /** A fresh book snapshot. Frame-batched — cheap to call on every tick. */
    update(book) {
        this.book = book;
        if (this.statusEl)
            this.statusEl.textContent = this.spreadText();
        if (!this.raf)
            this.raf = requestAnimationFrame(this.frame);
    }
    /** Clear to an empty book — a reconnect, since the old one may be stale. */
    reset() {
        this.update({ bids: [], asks: [] });
    }
    destroy() {
        this.ro.disconnect();
        this.ac.abort();
        if (this.raf)
            cancelAnimationFrame(this.raf);
    }
    /** `"<spread> · <spread%>"`, or `""` with no two-sided book. */
    spreadText() {
        const bb = this.book.bids[0], ba = this.book.asks[0];
        if (!bb || !ba)
            return "";
        const sp = ba[0] - bb[0];
        const mid = (bb[0] + ba[0]) / 2;
        return `${this.fmt.fmtPx(sp)} · ${(sp / mid * 100).toFixed(3)}%`;
    }
    frame = () => {
        this.raf = 0;
        this.draw();
    };
    resize() {
        const dpr = window.devicePixelRatio || 1;
        this.w = this.wrap.clientWidth;
        this.h = this.wrap.clientHeight;
        this.cv.width = Math.max(1, this.w * dpr);
        this.cv.height = Math.max(1, this.h * dpr);
        this.ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
        this.draw();
    }
    draw() {
        const { ctx, w, h } = this;
        const fmtPx = this.fmt.fmtPx;
        ctx.clearRect(0, 0, w, h);
        const { bids, asks } = this.book;
        if (!bids.length && !asks.length) {
            ctx.fillStyle = "#5b6480";
            ctx.font = "12px system-ui";
            ctx.textAlign = "center";
            ctx.fillText("waiting for book…", w / 2, h / 2);
            return;
        }
        const cum = (lvls) => { let c = 0; return lvls.map(([p, qty]) => [p, (c += qty)]); };
        const bidsCum = cum(bids), asksCum = cum(asks);
        const bb = bidsCum.length ? bidsCum[0][0] : null, ba = asksCum.length ? asksCum[0][0] : null;
        const mid = bb != null && ba != null ? (bb + ba) / 2 : (bb ?? ba ?? 0);
        // One order resting far from mid stretches the price axis across dead space
        // and squashes the levels that matter into a few pixels. Drop levels beyond
        // ±maxRangePct of mid — but the axis is still fitted to what survives, so a
        // book tighter than the window keeps auto-fitting exactly as before and the
        // clamp only ever bites on outliers.
        const limit = mid > 0 ? mid * this.maxRangePct / 100 : 0;
        const B = limit > 0 ? bidsCum.filter(([p]) => p >= mid - limit) : bidsCum;
        const A = limit > 0 ? asksCum.filter(([p]) => p <= mid + limit) : asksCum;
        // Scale to the depth actually on screen, not to a total that includes the
        // levels just cut — otherwise the visible curve is flattened by a ghost.
        const maxCum = Math.max(B.length ? B[B.length - 1][1] : 0, A.length ? A[A.length - 1][1] : 0, 1);
        const hidden = (bidsCum.length - B.length) + (asksCum.length - A.length);
        const lo = (B.length ? B[B.length - 1][0] : mid) - 1;
        const hi = (A.length ? A[A.length - 1][0] : mid) + 1;
        const pad = Math.max((hi - lo) * 0.05, 1);
        const pLo = lo - pad, pHi = hi + pad;
        const padT = 14, padB = 22, padR = 44, padL = 6;
        const X = (p) => padL + (p - pLo) / (pHi - pLo) * (w - padL - padR);
        const Y = (c) => padT + (1 - c / (maxCum * 1.06)) * (h - padT - padB);
        ctx.strokeStyle = "rgba(120,110,160,0.10)";
        ctx.fillStyle = "#5b6480";
        ctx.font = "10px ui-monospace, Menlo, monospace";
        ctx.lineWidth = 1;
        ctx.textAlign = "left";
        for (let i = 1; i <= 4; i++) {
            const c = maxCum * i / 4, y = Math.round(Y(c)) + 0.5;
            ctx.beginPath();
            ctx.moveTo(padL, y);
            ctx.lineTo(w - padR, y);
            ctx.stroke();
            ctx.fillText(fmtQty(c), w - padR + 6, y + 3);
        }
        ctx.textAlign = "center";
        // Six price labels collide below roughly 550px, and a depth chart is often
        // the narrowest panel on the page. Scale the tick count to the width.
        const nTicks = Math.max(2, Math.min(5, Math.floor((w - padL - padR) / 95)));
        for (let i = 0; i <= nTicks; i++) {
            const p = pLo + (pHi - pLo) * i / nTicks;
            const x = X(p);
            ctx.strokeStyle = "rgba(120,110,160,0.06)";
            ctx.beginPath();
            ctx.moveTo(Math.round(x) + 0.5, padT);
            ctx.lineTo(Math.round(x) + 0.5, h - padB);
            ctx.stroke();
            const lx = Math.max(padL + 18, Math.min(w - padR - 18, x));
            ctx.fillText(fmtPx(Math.round(p)), lx, h - 8);
        }
        // Say so when levels were cut, or your own far order looks like it vanished.
        if (hidden > 0) {
            ctx.textAlign = "left";
            ctx.fillStyle = "#5b6480";
            ctx.fillText(`${hidden} level${hidden > 1 ? "s" : ""} outside ±${this.maxRangePct}%`, padL + 2, padT + 8);
            ctx.textAlign = "center";
        }
        const area = (pts, dir, rgb) => {
            if (!pts.length)
                return;
            const x0 = X(pts[0][0]);
            ctx.beginPath();
            ctx.moveTo(x0, Y(0));
            ctx.lineTo(x0, Y(pts[0][1]));
            for (let i = 1; i < pts.length; i++) {
                ctx.lineTo(X(pts[i][0]), Y(pts[i - 1][1]));
                ctx.lineTo(X(pts[i][0]), Y(pts[i][1]));
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
        area(B, -1, "0,255,136");
        area(A, 1, "255,61,87");
        if (bb != null && ba != null) {
            const x = Math.round(X(mid)) + 0.5;
            ctx.strokeStyle = "rgba(135,128,159,0.5)";
            ctx.setLineDash([4, 4]);
            ctx.beginPath();
            ctx.moveTo(x, padT);
            ctx.lineTo(x, h - padB);
            ctx.stroke();
            ctx.setLineDash([]);
        }
        if (this.hoverX != null) {
            const p = pLo + (this.hoverX - padL) / (w - padL - padR) * (pHi - pLo);
            if (p < pLo || p > pHi)
                return;
            const cumAt = (pts, keep) => {
                let c = 0;
                for (const [lp, lq] of pts) {
                    if (!keep(lp))
                        break;
                    c += lq;
                }
                return c;
            };
            const cb = cumAt(B, (lp) => lp >= p);
            const ca = cumAt(A, (lp) => lp <= p);
            const x = Math.round(X(p)) + 0.5;
            ctx.strokeStyle = "rgba(117,134,150,0.6)";
            ctx.setLineDash([3, 3]);
            ctx.beginPath();
            ctx.moveTo(x, padT);
            ctx.lineTo(x, h - padB);
            ctx.stroke();
            ctx.setLineDash([]);
            const dot = (c, rgb) => {
                if (c <= 0)
                    return;
                ctx.beginPath();
                ctx.arc(X(p), Y(c), 3.2, 0, Math.PI * 2);
                ctx.fillStyle = `rgba(${rgb},1)`;
                ctx.fill();
                ctx.strokeStyle = "#0d1322";
                ctx.lineWidth = 1.5;
                ctx.stroke();
            };
            dot(cb, "0,255,136");
            dot(ca, "255,61,87");
            this.tip.style.display = "block";
            this.tip.innerHTML =
                `<span class="p">${fmtPx(Math.round(p))}</span><br>` +
                    `<span class="b">bid ${fmtQty(cb)}</span><br>` +
                    `<span class="a">ask ${fmtQty(ca)}</span>`;
            const tw = this.tip.offsetWidth;
            this.tip.style.left = Math.min(Math.max(4, this.hoverX + 14), w - tw - 4) + "px";
            this.tip.style.top = Math.max(4, this.hoverY - 48) + "px";
        }
    }
}
//# sourceMappingURL=depth.js.map