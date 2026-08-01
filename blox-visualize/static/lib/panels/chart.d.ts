import type { PriceFormat } from "../format.js";
import type { OpenOrder, Side, TradeTick } from "../feed.js";
import { type PanelChrome } from "../dom.js";
/** A fill to mark on the chart. */
export type ChartFill = {
    ts: number;
    price: number;
    side: Side;
};
export type MarketChartOptions = PanelChrome & {
    fmt: PriceFormat;
    tfList?: number[];
    maxCandles?: number;
    /** Draw the session VWAP line. */
    vwap?: boolean;
    /** Fill markers retained. Older ones fall off the chart. */
    maxFills?: number;
    /** The LightweightCharts namespace. Omitted → resolved on demand. */
    charts?: any;
};
/** Seconds per bucket → button label. */
export declare const TF_LABEL: Record<number, string>;
export declare class MarketChart {
    private readonly chartEl;
    private readonly tfEl;
    private readonly statusEl;
    private readonly fmt;
    private readonly tfList;
    private readonly maxCandles;
    private readonly maxFills;
    private readonly showVwap;
    private readonly candles;
    private readonly lastKey;
    private readonly cumPv;
    private readonly cumV;
    private readonly ac;
    private tf;
    private chart;
    private candleSeries;
    private volSeries;
    private vwapSeries;
    private raf;
    private destroyed;
    private fills;
    private orders;
    private priceLines;
    constructor(host: HTMLElement, opts: MarketChartOptions);
    private init;
    /** Local wall clock, at a resolution that suits the bar size. */
    private fmtAxisTime;
    /** Bulk backfill, oldest first. One repaint at the end, not one per trade. */
    seed(trades: TradeTick[]): void;
    addTrade(t: TradeTick): void;
    /** Your working orders, drawn as dashed price lines. */
    setOrders(open: OpenOrder[]): void;
    /** Dots where your orders filled — green under a buy, red over a sell. */
    setFills(fills: ChartFill[]): void;
    addFill(f: ChartFill): void;
    reset(): void;
    setTF(tf: number): void;
    destroy(): void;
    /** Full redraw of the visible timeframe. For seeds and timeframe switches. */
    private repaint;
    private drawMarkers;
    private drawOrderLines;
    private toBar;
    private toVol;
    private addCandle;
    private scheduleDraw;
    private frame;
    private lastBarOhlc;
    private setOhlc;
}
