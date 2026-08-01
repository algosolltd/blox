import type { PriceFormat } from "../format.js";
import type { Pnl } from "../feed.js";
import { type PanelChrome } from "../dom.js";
export type PnlOptions = PanelChrome & {
    fmt: PriceFormat;
    /** Sparkline width in samples. One sample per sampleMs. */
    maxSamples?: number;
    sampleMs?: number;
};
export declare class PnlPanel {
    private readonly els;
    private readonly spark;
    private readonly fmt;
    private readonly maxSamples;
    private readonly ro;
    private readonly timer;
    private pnl;
    private samples;
    constructor(host: HTMLElement, opts: PnlOptions);
    update(pnl: Pnl): void;
    reset(): void;
    destroy(): void;
    private sample;
    private render;
    private drawSparkline;
}
