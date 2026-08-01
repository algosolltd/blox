import type { Feed } from "./feed.js";
export declare const BLOX_PANELS: readonly ["chart", "book", "depth", "trades", "entry", "orders", "pnl"];
export type BloxPanelId = (typeof BLOX_PANELS)[number];
export type TradingPanelOptions = {
    feed: Feed;
    /** Which panels to mount, in order. Defaults to all of them. */
    panels?: readonly BloxPanelId[];
    /** Overridden by the server's hello frame when they disagree. */
    priceScale?: number;
    symbol?: string;
    /** LightweightCharts namespace, forwarded to the chart. */
    charts?: any;
};
export declare class TradingPanel {
    private readonly host;
    private readonly feed;
    private readonly ids;
    private readonly charts;
    private symbol;
    private scale;
    private book;
    private orderId;
    private chart;
    private bookPanel;
    private depth;
    private trades;
    private entry;
    private orders;
    private pnl;
    private unsubscribe;
    constructor(host: HTMLElement, opts: TradingPanelOptions);
    destroy(): void;
    private build;
    private teardownPanels;
}
