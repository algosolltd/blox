// Candlestick + volume chart, with your live position's VWAP, working orders
// as dashed price lines, and a dot on every fill.
//
// LightweightCharts is an optional peer dep: pass the namespace in, or leave
// it out and it is imported on first mount. Trades aggregate into candles
// whether or not the chart has loaded yet, so a history backfill that lands
// before the library does is not lost.
import { mount, q } from "../dom.js";
let chartsPromise = null;
/** Window global first (a <script> tag already did the work), then the peer dep. */
function loadCharts() {
    const g = globalThis.LightweightCharts;
    if (g)
        return Promise.resolve(g);
    // @ts-ignore optional peer dependency, resolved by the consumer's bundler
    chartsPromise ??= import("lightweight-charts");
    return chartsPromise;
}
const MIN = 60, HOUR = 3600, DAY = 86400;
/** Seconds per bucket → button label. */
export const TF_LABEL = {
    1: "1s", 5: "5s", 15: "15s", 30: "30s",
    [MIN]: "1m", [5 * MIN]: "5m", [15 * MIN]: "15m", [30 * MIN]: "30m",
    [HOUR]: "1h", [4 * HOUR]: "4h", [12 * HOUR]: "12h", [DAY]: "1D",
};
const DEFAULT_TFS = [1, 5, 15, MIN, 5 * MIN, 15 * MIN, HOUR, 4 * HOUR, DAY];
const UP = "#00ff88", DOWN = "#ff3d57";
const DASHED = 2; // LightweightCharts LineStyle.Dashed
/**
 * Candlestick + volume chart, with your live position's VWAP, working
 * orders as dashed price lines, and a dot on every fill.
 *
 * `new MarketChart(host, opts)`, then `seed(trades)` for backfill and
 * `addTrade(t)` per live tick. `setOrders`, `setFills`/`addFill` and
 * `setAvg` are optional and independent of the trade stream. `destroy()`
 * tears down the chart and its listeners.
 */
export class MarketChart {
    chartEl;
    tfEl;
    statusEl;
    fmt;
    tfList;
    maxCandles;
    maxFills;
    showVwap;
    candles;
    // Newest bucket key per timeframe. Without it the draw path walks every key
    // in the map — a 4000-element array allocated on each frame.
    lastKey = new Map();
    ac = new AbortController();
    // Your live position's average open price, in ticks. Null while flat.
    // Server-computed (volume-weighted across your fills) — the chart just
    // plots it, it doesn't recompute it from a fill stream.
    posAvg = null;
    tf;
    chart = null;
    candleSeries = null;
    volSeries = null;
    vwapSeries = null;
    raf = 0;
    destroyed = false;
    // Held until the chart exists, then applied — both can arrive first.
    fills = [];
    orders = [];
    priceLines = [];
    // Wall-clock seconds when the current position was opened. Null while flat.
    // The VWAP line starts here — candles before it stay null, so a fresh open
    // draws a fresh line instead of stretching back across pre-open history.
    // Captured on the null→open transition in `setAvg`; cleared on flatten.
    posOpenedAt = null;
    constructor(host, opts) {
        this.fmt = opts.fmt;
        this.tfList = opts.tfList ?? DEFAULT_TFS;
        this.tf = this.tfList[0];
        this.maxCandles = opts.maxCandles ?? 4000;
        this.maxFills = opts.maxFills ?? 500;
        this.showVwap = opts.vwap ?? true;
        this.candles = new Map(this.tfList.map((tf) => [tf, new Map()]));
        const buttons = this.tfList
            .map((tf, i) => `<button data-tf="${tf}"${i === 0 ? ' class="on"' : ""}>${TF_LABEL[tf] ?? tf + "s"}</button>`)
            .join("");
        this.statusEl = mount(host, `<div class="bx-tf tf-group">${buttons}</div><div class="chart-wrap"></div>`, opts);
        this.tfEl = q(host, ".bx-tf");
        this.chartEl = q(host, ".chart-wrap");
        this.tfEl.addEventListener("click", (e) => {
            const b = e.target.closest("[data-tf]");
            if (b)
                this.setTF(+b.dataset.tf);
        }, { signal: this.ac.signal });
        // Trades keep landing in `this.candles` while the tab is hidden — only
        // the draw is missed, because a backgrounded tab throttles or fully
        // suspends rAF, and `frame()` only ever ships the single newest bar. On
        // return that leaves every bar in between sitting in memory but never
        // drawn: a full repaint from the (complete, up to date) map catches it up
        // in one shot instead of leaving a hole.
        document.addEventListener("visibilitychange", () => {
            if (!document.hidden)
                this.repaint();
        }, { signal: this.ac.signal });
        void this.init(opts.charts);
    }
    async init(charts) {
        const LC = charts ?? await loadCharts();
        if (this.destroyed)
            return;
        this.chart = LC.createChart(this.chartEl, {
            autoSize: true,
            layout: { background: { type: "solid", color: "transparent" }, textColor: "#8b95b0", fontSize: 11, fontFamily: "ui-monospace, Menlo, monospace" },
            grid: { vertLines: { color: "rgba(120,110,160,0.08)" }, horzLines: { color: "rgba(120,110,160,0.08)" } },
            crosshair: { vertLine: { color: "#758696", style: 3, labelBackgroundColor: UP }, horzLine: { color: "#758696", style: 3, labelBackgroundColor: UP } },
            rightPriceScale: { borderColor: "rgba(120,110,160,0.25)" },
            timeScale: {
                borderColor: "rgba(120,110,160,0.25)", timeVisible: true, secondsVisible: true,
                rightOffset: 3, shiftVisibleRangeOnNewBar: true, barSpacing: 9,
                // LightweightCharts labels the axis in UTC. Every other clock on the
                // page (the tape, the order rows) is local, so an unconverted axis
                // reads hours off from the trade that produced the bar.
                tickMarkFormatter: (t) => this.fmtAxisTime(t),
            },
            localization: {
                priceFormatter: (p) => p.toFixed(this.fmt.dec),
                timeFormatter: (t) => this.fmtAxisTime(t, true),
            },
        });
        this.candleSeries = this.chart.addCandlestickSeries({
            upColor: UP, downColor: DOWN, borderVisible: false,
            wickUpColor: UP, wickDownColor: DOWN,
            priceFormat: { type: "price", precision: this.fmt.dec, minMove: 1 / this.fmt.div },
            scaleMargins: { top: 0.08, bottom: 0.18 },
        });
        this.volSeries = this.chart.addHistogramSeries({
            priceFormat: { type: "volume" }, priceScaleId: "",
            lastValueVisible: false, priceLineVisible: false,
        });
        this.chart.priceScale("").applyOptions({ scaleMargins: { top: 0.86, bottom: 0 } });
        if (this.showVwap) {
            this.vwapSeries = this.chart.addLineSeries({
                color: "rgba(255,209,102,0.9)", lineWidth: 1, priceLineVisible: false,
                lastValueVisible: false, crosshairMarkerVisible: false, title: "VWAP",
            });
        }
        this.chart.subscribeCrosshairMove((p) => {
            const bar = p.seriesData?.get(this.candleSeries);
            if (bar)
                this.setOhlc(bar);
            else
                this.lastBarOhlc();
        });
        // Paint whatever arrived while the library was loading.
        this.repaint();
    }
    /** Local wall clock, at a resolution that suits the bar size. */
    fmtAxisTime(t, full = false) {
        const d = new Date(t * 1000);
        const p = (n) => String(n).padStart(2, "0");
        const date = `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
        if (this.tf >= DAY && !full)
            return date;
        const clock = this.tf >= MIN && !full
            ? `${p(d.getHours())}:${p(d.getMinutes())}`
            : `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
        return full ? `${date} ${clock}` : clock;
    }
    /** Bulk backfill, oldest first. One repaint at the end, not one per trade. */
    seed(trades) {
        for (const t of trades) {
            for (const tf of this.tfList)
                this.addCandle(tf, t);
        }
        this.repaint();
    }
    /** One live trade. */
    addTrade(t) {
        for (const tf of this.tfList)
            this.addCandle(tf, t);
    }
    /** Your working orders, drawn as dashed price lines. */
    setOrders(open) {
        this.orders = open;
        this.drawOrderLines();
    }
    /** Dots where your orders filled — green under a buy, red over a sell. */
    setFills(fills) {
        this.fills = fills.length > this.maxFills ? fills.slice(-this.maxFills) : fills;
        this.drawMarkers();
    }
    /** One live fill, appended to what `setFills` already holds. */
    addFill(f) {
        this.fills.push(f);
        if (this.fills.length > this.maxFills)
            this.fills.splice(0, this.fills.length - this.maxFills);
        this.drawMarkers();
    }
    /** Your position's average open price, in ticks. 0/null means flat. */
    setAvg(avg) {
        const next = avg || null;
        const opened = next !== null && this.posAvg === null;
        const closed = next === null && this.posAvg !== null;
        this.posAvg = next;
        if (closed)
            this.posOpenedAt = null;
        else if (opened)
            this.posOpenedAt = Math.floor(Date.now() / 1000);
        // Re-stamp every candle: a fresh open must not paint VWAP across history,
        // and a fresh close must clear it from every bucket. Any transition
        // reshapes the line (it can vanish or appear from "now"), so push the
        // whole visible series at once — `update` only ships a single point and
        // leaves the chart holding the previous run's values, which would bridge
        // straight across the flat gap.
        if (opened || closed)
            this.repaint();
        else {
            // Same position, just a fresh avg: the latest candle is the only one
            // that visibly moves, so an `update` is enough.
            for (const tf of this.tfList) {
                const last = this.lastKey.get(tf);
                if (last === undefined || tf !== this.tf)
                    continue;
                const c = this.candles.get(tf).get(last);
                if (!c)
                    continue;
                c.vwap = this.vwapFor(c.time);
                this.vwapSeries?.update(this.toVwapPoint(c));
            }
        }
    }
    /** Drop every candle, fill, order and VWAP point — a reconnect, since the past may no longer apply. */
    reset() {
        for (const m of this.candles.values())
            m.clear();
        this.lastKey.clear();
        this.posAvg = null;
        this.posOpenedAt = null;
        this.fills = [];
        this.orders = [];
        if (this.candleSeries) {
            this.candleSeries.setData([]);
            this.volSeries.setData([]);
            this.vwapSeries?.setData([]);
            this.drawMarkers();
            this.drawOrderLines();
        }
        this.setOhlc(null);
    }
    /** Switch the visible timeframe and repaint. */
    setTF(tf) {
        this.tf = tf;
        for (const b of this.tfEl.querySelectorAll("[data-tf]")) {
            b.classList.toggle("on", +b.dataset.tf === tf);
        }
        this.repaint();
        this.chart?.timeScale().scrollToRealTime();
    }
    /** Tear down the chart and its listeners. */
    destroy() {
        this.destroyed = true;
        this.ac.abort();
        if (this.raf)
            cancelAnimationFrame(this.raf);
        this.chart?.remove();
        this.chart = null;
    }
    /** Full redraw of the visible timeframe. For seeds and timeframe switches. */
    repaint() {
        if (!this.candleSeries)
            return;
        const arr = [...this.candles.get(this.tf).values()].sort((a, b) => a.time - b.time);
        this.candleSeries.setData(arr.map((c) => this.toBar(c)));
        this.volSeries.setData(arr.map((c) => this.toVol(c)));
        this.vwapSeries?.setData(arr.map((c) => this.toVwapPoint(c)));
        this.drawMarkers();
        this.drawOrderLines();
        this.lastBarOhlc();
    }
    // Markers must be sorted and land on a bucket boundary, so they are
    // recomputed against whichever timeframe is on screen. Keyed by bucket so
    // a candle with several fills still gets one dot, not a stack of them —
    // the last fill in the bucket wins.
    drawMarkers() {
        if (!this.candleSeries)
            return;
        const tf = this.tf;
        const byBucket = new Map();
        for (const f of this.fills) {
            const time = Math.floor(f.ts / 1000 / tf) * tf;
            byBucket.set(time, {
                time,
                position: f.side === "B" ? "belowBar" : "aboveBar",
                color: f.side === "B" ? UP : DOWN,
                shape: "circle",
                size: 0.8,
                // Dot only. A price label per fill collides with the candles, the
                // order lines and the other fills the moment there is more than one;
                // the exact prices are in the orders table.
            });
        }
        const markers = [...byBucket.values()].sort((a, b) => a.time - b.time);
        this.candleSeries.setMarkers(markers);
    }
    drawOrderLines() {
        if (!this.candleSeries)
            return;
        for (const line of this.priceLines)
            this.candleSeries.removePriceLine(line);
        this.priceLines = [];
        for (const o of this.orders) {
            if (o.kind === "MARKET" || !o.price)
                continue;
            this.priceLines.push(this.candleSeries.createPriceLine({
                price: o.price / this.fmt.div,
                color: o.side === "B" ? UP : DOWN,
                lineWidth: 1,
                lineStyle: DASHED,
                axisLabelVisible: true,
                title: `${o.side === "B" ? "BUY" : "SELL"} ${o.remaining}`,
            }));
        }
    }
    toBar(c) {
        return {
            time: c.time,
            open: c.open / this.fmt.div, high: c.high / this.fmt.div,
            low: c.low / this.fmt.div, close: c.close / this.fmt.div,
        };
    }
    toVol(c) {
        return { time: c.time, value: c.vol, color: c.close >= c.open ? "rgba(0,255,136,0.45)" : "rgba(255,61,87,0.45)" };
    }
    addCandle(tf, t) {
        const key = Math.floor(t.ts / 1000 / tf) * tf;
        const m = this.candles.get(tf);
        let c = m.get(key);
        if (!c) {
            c = { time: key, open: t.price, high: t.price, low: t.price, close: t.price, vol: 0, vwap: this.vwapFor(key) };
            m.set(key, c);
            if (m.size > this.maxCandles) {
                const oldest = m.keys().next().value;
                if (oldest !== undefined)
                    m.delete(oldest);
            }
        }
        c.high = Math.max(c.high, t.price);
        c.low = Math.min(c.low, t.price);
        c.close = t.price;
        c.vol += t.qty;
        // Guarded so a late or out-of-order trade cannot rewind the newest bucket.
        if (key > (this.lastKey.get(tf) ?? -Infinity))
            this.lastKey.set(tf, key);
        if (tf === this.tf)
            this.scheduleDraw();
    }
    // Draw on write. A permanent 60fps loop spins whether or not a trade landed;
    // a pending frame coalesces every tick that arrives before it fires.
    scheduleDraw() {
        if (!this.raf)
            this.raf = requestAnimationFrame(this.frame);
    }
    frame = () => {
        this.raf = 0;
        if (!this.candleSeries)
            return;
        const key = this.lastKey.get(this.tf);
        if (key === undefined)
            return;
        const c = this.candles.get(this.tf).get(key);
        if (!c)
            return;
        this.candleSeries.update(this.toBar(c));
        this.volSeries.update(this.toVol(c));
        this.vwapSeries?.update(this.toVwapPoint(c));
        this.lastBarOhlc();
    };
    /** A value point, or a whitespace point (gap) while flat / avg unknown. */
    toVwapPoint(c) {
        return c.vwap == null ? { time: c.time } : { time: c.time, value: c.vwap / this.fmt.div };
    }
    /** VWAP for a candle at `key` (bucket seconds). Pre-open candles are null. */
    vwapFor(key) {
        if (this.posAvg === null || this.posOpenedAt === null)
            return null;
        return key >= this.posOpenedAt ? this.posAvg : null;
    }
    lastBarOhlc() {
        const key = this.lastKey.get(this.tf);
        const c = key === undefined ? undefined : this.candles.get(this.tf).get(key);
        this.setOhlc(c ? this.toBar(c) : null);
    }
    setOhlc(bar) {
        if (!this.statusEl)
            return;
        if (!bar) {
            this.statusEl.textContent = "";
            return;
        }
        const cls = bar.close >= bar.open ? "up" : "down";
        const d = this.fmt.dec;
        this.statusEl.innerHTML =
            `O <b class="${cls}">${bar.open.toFixed(d)}</b> ` +
                `H <b class="${cls}">${bar.high.toFixed(d)}</b> ` +
                `L <b class="${cls}">${bar.low.toFixed(d)}</b> ` +
                `C <b class="${cls}">${bar.close.toFixed(d)}</b>`;
    }
}
//# sourceMappingURL=chart.js.map