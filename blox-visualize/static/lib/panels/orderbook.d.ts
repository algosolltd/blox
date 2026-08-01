import type { PriceFormat } from "../format.js";
import type { Book } from "../feed.js";
import { type PanelChrome } from "../dom.js";
export type OrderBookOptions = PanelChrome & {
    fmt: PriceFormat;
};
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
    update(book: Book): void;
    reset(): void;
    destroy(): void;
    private frame;
    private buildRows;
    private render;
}
