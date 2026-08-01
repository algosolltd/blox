// The wire protocol and the one thing that speaks it.
//
// Panels never touch a WebSocket. The adapter parses each frame once, fans it
// out to whichever listeners asked for that channel, and — the part worth
// reading — makes the historical backfill and the live stream arrive in the
// right order.

export type Side = "B" | "S";
export type OrderKind = "LIMIT" | "MARKET" | "IOC" | "FOK" | "POST";

export type TradeTick = { ts: number; price: number; qty: number; side: Side; mine?: boolean };
export type BookLevel = [price: number, qty: number];
export type Book = { bids: BookLevel[]; asks: BookLevel[] };

export type Pnl = {
  total: number; realized: number; unrealized: number;
  pos: number; avg: number; mark: number; volume: number; fills: number;
};

export type OpenOrder = {
  id: number; ts: number; side: Side; kind: OrderKind;
  price: number; qty: number; remaining: number; filled: number;
};

export type ClosedOrder = {
  id: number; ts: number; tsEnd?: number; side: Side;
  avgFill: number; filled: number; qty: number;
  status: "FILLED" | "CANCELLED" | "REJECTED" | "LOST" | "LIVE";
  reason?: string; price?: number;
};

export type Hello = { instrument: string; priceScale: number; server?: string; instrumentId?: number };

/** Engine counters, for a status bar. Not needed to trade. */
export type Stats = { ts: number; applied: number; errors?: number; books?: number; conns?: number; dropped?: number };

export type EngineStatus = { state: "up" | "down"; detail?: string };

/** Everything the server may say. */
export type ServerMessage =
  | { type: "hello"; payload: Hello }
  | { type: "history"; payload: { trades: TradeTick[]; from?: number; to?: number } }
  | { type: "update"; payload: { book?: Book; trades?: TradeTick[]; stats?: Stats } }
  | { type: "account"; payload: { full?: boolean; open?: OpenOrder[]; closed?: ClosedOrder[]; pnl?: Pnl } }
  | { type: "status"; payload: { state: "up" | "down"; detail?: string } }
  | { type: "notice"; payload: { level: "info" | "warn" | "err"; text: string } };

/** Everything the client may say. */
export type ClientMessage =
  | { type: "order"; id: number; side: Side; kind: OrderKind; price: number; qty: number }
  | { type: "cancel"; id: number }
  | { type: "reduce"; id: number; qty: number };

export type ConnectionState = "connecting" | "live" | "down";

/** Subscribe to the channels a panel actually needs; everything is optional. */
export type FeedListeners = {
  onHello?: (h: Hello) => void;
  /** Bulk backfill, oldest first. Fires once per (re)connect, before any onTrade. */
  onHistory?: (trades: TradeTick[]) => void;
  onTrade?: (t: TradeTick) => void;
  onBook?: (b: Book) => void;
  onAccount?: (a: { full?: boolean; open?: OpenOrder[]; closed?: ClosedOrder[]; pnl?: Pnl }) => void;
  onStats?: (s: Stats) => void;
  onNotice?: (n: { level: "info" | "warn" | "err"; text: string }) => void;
  /** The engine behind the server. Distinct from the socket to this server. */
  onEngineStatus?: (s: EngineStatus) => void;
  onConnection?: (state: ConnectionState) => void;
  /** Fires before onHistory on every reconnect — panels drop stale state here. */
  onReset?: () => void;
};

export interface Feed {
  subscribe(l: FeedListeners): () => void;
  send(msg: ClientMessage): void;
  destroy(): void;
}

export type FeedOptions = {
  /** How long to wait for a `history` frame before going live without one. */
  historyTimeoutMs?: number;
  maxBackoffMs?: number;
};

/**
 * Fan-out plus the backfill handshake.
 *
 * The ordering problem: a client that paints history and *then* starts
 * listening loses every trade that happened in between, and one that listens
 * first paints them twice out of order. So the adapter holds live frames from
 * the moment the socket opens, releases them only after history has been
 * replayed, and from then on passes them straight through.
 */
export class FeedAdapter implements Feed {
  private ws: WebSocket | null = null;
  private readonly subs = new Set<FeedListeners>();
  private readonly historyTimeoutMs: number;
  private readonly maxBackoffMs: number;

  private live = false;
  private held: ServerMessage[] = [];
  private historyTimer: ReturnType<typeof setTimeout> | null = null;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private attempts = 0;
  private destroyed = false;

  constructor(private readonly url: string, opts: FeedOptions = {}) {
    this.historyTimeoutMs = opts.historyTimeoutMs ?? 2000;
    this.maxBackoffMs = opts.maxBackoffMs ?? 30000;
  }

  connect(): void {
    if (this.destroyed) return;
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
      let msg: ServerMessage;
      try {
        msg = JSON.parse(e.data as string) as ServerMessage; // parsed once, for everyone
      } catch {
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
  private release(trades: TradeTick[]): void {
    if (this.live) return;
    if (this.historyTimer) {
      clearTimeout(this.historyTimer);
      this.historyTimer = null;
    }
    this.live = true;
    if (trades.length) this.emit((l) => l.onHistory?.(trades));
    const held = this.held;
    this.held = [];
    for (const m of held) this.dispatch(m);
  }

  private dispatch(msg: ServerMessage): void {
    switch (msg.type) {
      case "hello":
        this.emit((l) => l.onHello?.(msg.payload));
        break;
      case "update": {
        // Trades first: a frame is a batch of fills plus the book they left
        // behind, so replaying it the other way shows the aftermath before
        // the cause.
        const { book, trades, stats } = msg.payload;
        if (trades) for (const t of trades) this.emit((l) => l.onTrade?.(t));
        if (book) this.emit((l) => l.onBook?.(book));
        if (stats) this.emit((l) => l.onStats?.(stats));
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

  private emit(fn: (l: FeedListeners) => void): void {
    for (const l of this.subs) fn(l);
  }

  private scheduleReconnect(): void {
    if (this.destroyed) return;
    this.emit((l) => l.onConnection?.("down"));
    const delay = Math.min(this.maxBackoffMs, 1000 * 2 ** this.attempts);
    this.attempts++;
    this.reconnectTimer = setTimeout(() => this.connect(), delay);
  }

  subscribe(l: FeedListeners): () => void {
    this.subs.add(l);
    return () => { this.subs.delete(l); };
  }

  send(msg: ClientMessage): void {
    if (this.ws?.readyState === WebSocket.OPEN) this.ws.send(JSON.stringify(msg));
  }

  destroy(): void {
    this.destroyed = true;
    if (this.historyTimer) clearTimeout(this.historyTimer);
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer);
    if (this.ws) {
      this.ws.onclose = null; // don't let teardown trigger a reconnect
      this.ws.close();
      this.ws = null;
    }
    this.subs.clear();
    this.held.length = 0;
  }
}
