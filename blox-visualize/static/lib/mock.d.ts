import type { ClientMessage, Feed, FeedListeners } from "./feed.js";
export type MockFeedOptions = {
    instrument?: string;
    priceScale?: number;
    basePrice?: number;
    /** Price tick — the book's level spacing. Belongs to the instrument. */
    tick?: number;
    volatility?: number;
    /** Trades to synthesise as backfill before going live. */
    historyCount?: number;
    /** Mean interval between live trades. */
    intervalMs?: number;
};
export declare class MockFeed implements Feed {
    private readonly subs;
    private readonly hello;
    private readonly base;
    private readonly tick;
    private readonly vol;
    private readonly historyCount;
    private readonly intervalMs;
    private last;
    private timer;
    private started;
    constructor(opts?: MockFeedOptions);
    connect(): void;
    /** Walk backwards from the base price, then hand the trades back in order. */
    private backfill;
    private step;
    private emit;
    subscribe(l: FeedListeners): () => void;
    send(_msg: ClientMessage): void;
    destroy(): void;
}
