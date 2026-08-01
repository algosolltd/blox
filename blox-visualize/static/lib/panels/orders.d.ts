import type { PriceFormat } from "../format.js";
import type { ClosedOrder, OpenOrder } from "../feed.js";
import { type PanelChrome } from "../dom.js";
export type OrdersOptions = PanelChrome & {
    fmt: PriceFormat;
    onCancel: (id: number) => void;
    onReduce: (id: number, qty: number) => void;
};
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
    update(a: {
        open?: OpenOrder[];
        closed?: ClosedOrder[];
    }): void;
    reset(): void;
    destroy(): void;
    private renderOpen;
    private renderClosed;
    private buildActions;
    private revertReduceEdit;
    private onClick;
    private onKeydown;
}
