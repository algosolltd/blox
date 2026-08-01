export type Side = "B" | "S";
export type OrderKind = "LIMIT" | "MARKET" | "IOC" | "FOK" | "POST";
export type TradeTick = {
    ts: number;
    price: number;
    qty: number;
    side: Side;
    mine?: boolean;
};
export type BookLevel = [price: number, qty: number];
export type Book = {
    bids: BookLevel[];
    asks: BookLevel[];
};
export type Pnl = {
    total: number;
    realized: number;
    unrealized: number;
    pos: number;
    avg: number;
    mark: number;
    volume: number;
    fills: number;
};
export type OpenOrder = {
    id: number;
    ts: number;
    side: Side;
    kind: OrderKind;
    price: number;
    qty: number;
    remaining: number;
    filled: number;
};
export type ClosedOrder = {
    id: number;
    ts: number;
    tsEnd?: number;
    side: Side;
    avgFill: number;
    filled: number;
    qty: number;
    status: "FILLED" | "CANCELLED" | "REJECTED" | "LOST" | "LIVE";
    reason?: string;
    price?: number;
};
export type Hello = {
    instrument: string;
    priceScale: number;
    server?: string;
    instrumentId?: number;
};
/** Engine counters, for a status bar. Not needed to trade. */
export type Stats = {
    ts: number;
    applied: number;
    errors?: number;
    books?: number;
    conns?: number;
    dropped?: number;
};
export type EngineStatus = {
    state: "up" | "down";
    detail?: string;
};
/** Everything the server may say. */
export type ServerMessage = {
    type: "hello";
    payload: Hello;
} | {
    type: "history";
    payload: {
        trades: TradeTick[];
        from?: number;
        to?: number;
    };
} | {
    type: "update";
    payload: {
        book?: Book;
        trades?: TradeTick[];
        stats?: Stats;
    };
} | {
    type: "account";
    payload: {
        full?: boolean;
        open?: OpenOrder[];
        closed?: ClosedOrder[];
        pnl?: Pnl;
    };
} | {
    type: "status";
    payload: {
        state: "up" | "down";
        detail?: string;
    };
} | {
    type: "notice";
    payload: {
        level: "info" | "warn" | "err";
        text: string;
    };
};
/** Everything the client may say. */
export type ClientMessage = {
    type: "order";
    id: number;
    side: Side;
    kind: OrderKind;
    price: number;
    qty: number;
} | {
    type: "cancel";
    id: number;
} | {
    type: "reduce";
    id: number;
    qty: number;
};
export type ConnectionState = "connecting" | "live" | "down";
/** Subscribe to the channels a panel actually needs; everything is optional. */
export type FeedListeners = {
    onHello?: (h: Hello) => void;
    /** Bulk backfill, oldest first. Fires once per (re)connect, before any onTrade. */
    onHistory?: (trades: TradeTick[]) => void;
    onTrade?: (t: TradeTick) => void;
    onBook?: (b: Book) => void;
    onAccount?: (a: {
        full?: boolean;
        open?: OpenOrder[];
        closed?: ClosedOrder[];
        pnl?: Pnl;
    }) => void;
    onStats?: (s: Stats) => void;
    onNotice?: (n: {
        level: "info" | "warn" | "err";
        text: string;
    }) => void;
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
export declare class FeedAdapter implements Feed {
    private readonly url;
    private ws;
    private readonly subs;
    private readonly historyTimeoutMs;
    private readonly maxBackoffMs;
    private live;
    private held;
    private historyTimer;
    private reconnectTimer;
    private attempts;
    private destroyed;
    constructor(url: string, opts?: FeedOptions);
    connect(): void;
    /** Replay the backfill, then drain everything that arrived while we waited. */
    private release;
    private dispatch;
    private emit;
    private scheduleReconnect;
    subscribe(l: FeedListeners): () => void;
    send(msg: ClientMessage): void;
    destroy(): void;
}
