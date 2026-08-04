import type { PriceFormat } from "../format.js";
import type { Pnl } from "../feed.js";
import { type PanelChrome } from "../dom.js";
export type PnlOptions = PanelChrome & {
    fmt: PriceFormat;
    /** Sparkline width in samples. One sample per sampleMs. */
    maxSamples?: number;
    sampleMs?: number;
};
/**
 * Live PnL readout (realized, unrealized, position, avg open, mark, volume)
 * with a sparkline of total PnL over time.
 *
 * `new PnlPanel(host, opts)`, then `update(pnl)` on every account snapshot
 * that carries a `pnl` field. `destroy()` when done.
 */
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
    /** A fresh PnL snapshot. */
    update(pnl: Pnl): void;
    /** Clear the readout and sparkline — a reconnect, since the old numbers may be stale. */
    reset(): void;
    /** Stop the sparkline's sample timer and disconnect the resize observer. */
    destroy(): void;
    private sample;
    private render;
    private drawSparkline;
}
