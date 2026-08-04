import type { PriceFormat } from "../format.js";
import type { ClosedOrder, OpenOrder } from "../feed.js";
import { type PanelChrome } from "../dom.js";
export type OrdersOptions = PanelChrome & {
    fmt: PriceFormat;
    onCancel: (id: number) => void;
    onReduce: (id: number, qty: number) => void;
};
/**
 * Working and closed orders, in two independently resizable tables (drag
 * the bar between them). Cancel and reduce-qty are wired to `onCancel` /
 * `onReduce` from `opts`.
 *
 * `new Orders(host, opts)`, then `update({ open, closed })` — pass both
 * arrays even when one is empty; a field left `undefined` is "unchanged",
 * not "cleared". `destroy()` when done.
 */
export declare class Orders {
    private readonly openEl;
    private readonly closedEl;
    private readonly statusEl;
    private readonly fmt;
    private readonly onCancel;
    private readonly onReduce;
    private readonly ac;
    private open;
    private closed;
    private reduceEdit;
    constructor(host: HTMLElement, opts: OrdersOptions);
    /** Replace `open` and/or `closed` wholesale. Omit a field to leave it as-is. */
    update(a: {
        open?: OpenOrder[];
        closed?: ClosedOrder[];
    }): void;
    /** Clear both tables — a reconnect, since the old lists may be stale. */
    reset(): void;
    /** Tear down this panel's listeners. */
    destroy(): void;
    private renderOpen;
    private renderClosed;
    private buildActions;
    /** Drag the bar between the two tables to resize the closed-orders section. */
    private initSplitter;
    private revertReduceEdit;
    private onClick;
    private onKeydown;
}
