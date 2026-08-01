// The wire protocol and the one thing that speaks it.
//
// Panels never touch a WebSocket. The adapter parses each frame once, fans it
// out to whichever listeners asked for that channel, and — the part worth
// reading — makes the historical backfill and the live stream arrive in the
// right order.
/**
 * Fan-out plus the backfill handshake.
 *
 * The ordering problem: a client that paints history and *then* starts
 * listening loses every trade that happened in between, and one that listens
 * first paints them twice out of order. So the adapter holds live frames from
 * the moment the socket opens, releases them only after history has been
 * replayed, and from then on passes them straight through.
 */
export class FeedAdapter {
    url;
    ws = null;
    subs = new Set();
    historyTimeoutMs;
    maxBackoffMs;
    live = false;
    held = [];
    historyTimer = null;
    reconnectTimer = null;
    attempts = 0;
    destroyed = false;
    constructor(url, opts = {}) {
        this.url = url;
        this.historyTimeoutMs = opts.historyTimeoutMs ?? 2000;
        this.maxBackoffMs = opts.maxBackoffMs ?? 30000;
    }
    connect() {
        if (this.destroyed)
            return;
        this.emit((l) => l.onConnection?.("connecting"));
        const ws = new WebSocket(this.url);
        this.ws = ws;
        ws.onopen = () => {
            this.attempts = 0;
            this.live = false;
            this.held.length = 0;
            // Panels drop whatever the previous connection left behind — on a
            // reconnect the book and order tables are certainly stale.
            this.emit((l) => l.onReset?.());
            this.emit((l) => l.onConnection?.("live"));
            // ponytail: a server that never sends `history` would otherwise hold
            // live frames forever. Drop the timer once every server sends one.
            this.historyTimer = setTimeout(() => this.release([]), this.historyTimeoutMs);
        };
        ws.onmessage = (e) => {
            let msg;
            try {
                msg = JSON.parse(e.data); // parsed once, for everyone
            }
            catch {
                return;
            }
            if (msg.type === "history") {
                this.release(msg.payload.trades ?? []);
                return;
            }
            if (!this.live && (msg.type === "update" || msg.type === "account")) {
                this.held.push(msg);
                return;
            }
            this.dispatch(msg);
        };
        ws.onclose = () => this.scheduleReconnect();
        ws.onerror = () => ws.close();
    }
    /** Replay the backfill, then drain everything that arrived while we waited. */
    release(trades) {
        if (this.live)
            return;
        if (this.historyTimer) {
            clearTimeout(this.historyTimer);
            this.historyTimer = null;
        }
        this.live = true;
        if (trades.length)
            this.emit((l) => l.onHistory?.(trades));
        const held = this.held;
        this.held = [];
        for (const m of held)
            this.dispatch(m);
    }
    dispatch(msg) {
        switch (msg.type) {
            case "hello":
                this.emit((l) => l.onHello?.(msg.payload));
                break;
            case "update": {
                // Trades first: a frame is a batch of fills plus the book they left
                // behind, so replaying it the other way shows the aftermath before
                // the cause.
                const { book, trades, stats } = msg.payload;
                if (trades)
                    for (const t of trades)
                        this.emit((l) => l.onTrade?.(t));
                if (book)
                    this.emit((l) => l.onBook?.(book));
                if (stats)
                    this.emit((l) => l.onStats?.(stats));
                break;
            }
            case "account":
                this.emit((l) => l.onAccount?.(msg.payload));
                break;
            case "status":
                // A dead engine is not a dead socket — the page can still say which
                // one broke, so these stay separate signals.
                this.emit((l) => l.onEngineStatus?.(msg.payload));
                break;
            case "notice":
                this.emit((l) => l.onNotice?.(msg.payload));
                break;
        }
    }
    emit(fn) {
        for (const l of this.subs)
            fn(l);
    }
    scheduleReconnect() {
        if (this.destroyed)
            return;
        this.emit((l) => l.onConnection?.("down"));
        const delay = Math.min(this.maxBackoffMs, 1000 * 2 ** this.attempts);
        this.attempts++;
        this.reconnectTimer = setTimeout(() => this.connect(), delay);
    }
    subscribe(l) {
        this.subs.add(l);
        return () => { this.subs.delete(l); };
    }
    send(msg) {
        if (this.ws?.readyState === WebSocket.OPEN)
            this.ws.send(JSON.stringify(msg));
    }
    destroy() {
        this.destroyed = true;
        if (this.historyTimer)
            clearTimeout(this.historyTimer);
        if (this.reconnectTimer)
            clearTimeout(this.reconnectTimer);
        if (this.ws) {
            this.ws.onclose = null; // don't let teardown trigger a reconnect
            this.ws.close();
            this.ws = null;
        }
        this.subs.clear();
        this.held.length = 0;
    }
}
//# sourceMappingURL=feed.js.map