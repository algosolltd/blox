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
    update(book: Book): void;
    reset(): void;
    destroy(): void;
    spreadText(): string;
    private frame;
    private resize;
    private draw;
}
