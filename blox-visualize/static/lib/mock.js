// A synthetic feed, so panels can be developed and tested with no server.
// Same interface as FeedAdapter, including the history handshake: it emits a
// backfill of past trades on connect before it starts streaming.
//
// ponytail: market data only. `send()` is a no-op — matching orders would mean
// reimplementing the engine the real server already has.
export class MockFeed {
    subs = new Set();
    hello;
    base;
    tick;
    vol;
    historyCount;
    intervalMs;
    last;
    timer = null;
    started = false;
    constructor(opts = {}) {
        this.hello = {
            instrument: opts.instrument ?? "BLOX/USD",
            priceScale: opts.priceScale ?? 2,
            server: "mock",
        };
        this.base = opts.basePrice ?? 1245_00;
        this.tick = opts.tick ?? 1;
        this.vol = opts.volatility ?? 0.004;
        this.historyCount = opts.historyCount ?? 600;
        this.intervalMs = opts.intervalMs ?? 100;
        this.last = this.base;
    }
    connect() {
        if (this.started)
            return;
        this.started = true;
        this.emit((l) => l.onConnection?.("connecting"));
        // Deferred so a subscriber registered right after connect() still sees
        // the opening frames.
        this.timer = setTimeout(() => {
            this.emit((l) => l.onReset?.());
            this.emit((l) => l.onConnection?.("live"));
            this.emit((l) => l.onHello?.(this.hello));
            this.emit((l) => l.onHistory?.(this.backfill()));
            this.step();
        }, 0);
    }
    /** Walk backwards from the base price, then hand the trades back in order. */
    backfill() {
        const out = [];
        const now = Date.now();
        let px = this.base;
        for (let i = this.historyCount; i > 0; i--) {
            const step = (Math.random() - 0.5) * this.vol * px;
            px = Math.max(1, Math.round(px + step));
            out.push({
                ts: now - i * this.intervalMs,
                price: px,
                qty: 50 + Math.floor(Math.random() * 250),
                side: step >= 0 ? "B" : "S",
            });
        }
        this.last = px;
        return out;
    }
    step = () => {
        const step = (Math.random() - 0.5) * this.vol * this.last;
        this.last = Math.max(1, Math.round(this.last + step));
        const t = {
            ts: Date.now(),
            price: this.last,
            qty: 50 + Math.floor(Math.random() * 250),
            side: step >= 0 ? "B" : "S",
        };
        const book = buildBook(this.last, this.tick);
        this.emit((l) => l.onTrade?.(t));
        this.emit((l) => l.onBook?.(book));
        this.timer = setTimeout(this.step, this.intervalMs * (0.5 + Math.random()));
    };
    emit(fn) {
        for (const l of this.subs)
            fn(l);
    }
    subscribe(l) {
        this.subs.add(l);
        return () => { this.subs.delete(l); };
    }
    send(_msg) {
        // no-op — see the note at the top of this file
    }
    destroy() {
        if (this.timer)
            clearTimeout(this.timer);
        this.timer = null;
        this.subs.clear();
    }
}
function buildBook(mid, tick, depth = 14) {
    const bids = [];
    const asks = [];
    for (let i = 1; i <= depth; i++) {
        const decay = 0.6 + Math.random() * 0.8;
        const size = () => Math.round((1500 + Math.random() * 2200) * decay);
        const bid = mid - i * tick;
        if (bid > 0)
            bids.push([bid, size()]);
        asks.push([mid + i * tick, size()]);
    }
    return { bids, asks };
}
//# sourceMappingURL=mock.js.map