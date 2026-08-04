import type { PriceFormat } from "../format.js";
import type { Book } from "../feed.js";
import { type PanelChrome } from "../dom.js";
export type DepthOptions = PanelChrome & {
    fmt: PriceFormat;
    /**
     * Levels further than this percentage from mid are left out of the view, so
     * one far-off order cannot flatten the whole curve. 0 shows the entire book.
     */
    maxRangePct?: number;
};
/**
 * Cumulative depth curve on a canvas, with a hover readout.
 *
 * `new Depth(host, opts)`, then `update(book)` on every book snapshot.
 * `destroy()` disconnects the resize observer and cancels the pending frame.
 */
export declare class Depth {
    private readonly cv;
    private readonly ctx;
    private readonly tip;
    private readonly wrap;
    private readonly statusEl;
    private readonly fmt;
    private readonly maxRangePct;
    private readonly ro;
    private readonly ac;
    private book;
    private hoverX;
    private hoverY;
    private w;
    private h;
    private raf;
    constructor(host: HTMLElement, opts: DepthOptions);
    /** A fresh book snapshot. Frame-batched — cheap to call on every tick. */
    update(book: Book): void;
    /** Clear to an empty book — a reconnect, since the old one may be stale. */
    reset(): void;
    destroy(): void;
    /** `"<spread> · <spread%>"`, or `""` with no two-sided book. */
    spreadText(): string;
    private frame;
    private resize;
    private draw;
}
