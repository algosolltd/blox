import type { PriceFormat } from "../format.js";
import type { TradeTick } from "../feed.js";
import { type PanelChrome } from "../dom.js";
export type TradesOptions = PanelChrome & {
    fmt: PriceFormat;
    /** Rows kept in the DOM. */
    maxRows?: number;
    /** Rows built from a single flush — a burst never renders more than this. */
    renderMax?: number;
};
export declare class Trades {
    private readonly listEl;
    private readonly statusEl;
    private readonly fmt;
    private readonly maxRows;
    private readonly renderMax;
    private buf;
    private times;
    private raf;
    constructor(host: HTMLElement, opts: TradesOptions);
    push(t: TradeTick): void;
    /** Bulk backfill, oldest first. */
    seed(trades: TradeTick[]): void;
    reset(): void;
    destroy(): void;
    private schedule;
    private frame;
    private flush;
    private updateRate;
}
