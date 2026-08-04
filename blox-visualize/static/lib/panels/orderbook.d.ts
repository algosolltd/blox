import type { PriceFormat } from "../format.js";
import type { Book } from "../feed.js";
import { type PanelChrome } from "../dom.js";
export type OrderBookOptions = PanelChrome & {
    fmt: PriceFormat;
};
/**
 * Two-sided order book ladder with depth bars.
 *
 * `new OrderBook(host, opts)`, then `update(book)` on every snapshot.
 * `destroy()` disconnects the resize observer and cancels the pending frame.
 */
export declare class OrderBook {
    private readonly asksEl;
    private readonly bidsEl;
    private readonly spreadEl;
    private readonly statusEl;
    private readonly obEl;
    private readonly fmt;
    private readonly ro;
    private book;
    private askRows;
    private bidRows;
    private rowsPerSide;
    private raf;
    constructor(host: HTMLElement, opts: OrderBookOptions);
    /** A fresh book snapshot. Frame-batched — cheap to call on every tick. */
    update(book: Book): void;
    /** Clear to an empty book — a reconnect, since the old one may be stale. */
    reset(): void;
    destroy(): void;
    private frame;
    private buildRows;
    private render;
}
