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
    /** Draw the position VWAP line. */
    vwap?: boolean;
    /** Fill markers retained. Older ones fall off the chart. */
    maxFills?: number;
    /** The LightweightCharts namespace. Omitted → resolved on demand. */
    charts?: any;
};
/** Seconds per bucket → button label. */
export declare const TF_LABEL: Record<number, string>;
/**
 * Candlestick + volume chart, with your live position's VWAP, working
 * orders as dashed price lines, and a dot on every fill.
 *
 * `new MarketChart(host, opts)`, then `seed(trades)` for backfill and
 * `addTrade(t)` per live tick. `setOrders`, `setFills`/`addFill` and
 * `setAvg` are optional and independent of the trade stream. `destroy()`
 * tears down the chart and its listeners.
 */
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
    private readonly ac;
    private posAvg;
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
    private posOpenedAt;
    constructor(host: HTMLElement, opts: MarketChartOptions);
    private init;
    /** Local wall clock, at a resolution that suits the bar size. */
    private fmtAxisTime;
    /** Bulk backfill, oldest first. One repaint at the end, not one per trade. */
    seed(trades: TradeTick[]): void;
    /** One live trade. */
    addTrade(t: TradeTick): void;
    /** Your working orders, drawn as dashed price lines. */
    setOrders(open: OpenOrder[]): void;
    /** Dots where your orders filled — green under a buy, red over a sell. */
    setFills(fills: ChartFill[]): void;
    /** One live fill, appended to what `setFills` already holds. */
    addFill(f: ChartFill): void;
    /** Your position's average open price, in ticks. 0/null means flat. */
    setAvg(avg: number | null): void;
    /** Drop every candle, fill, order and VWAP point — a reconnect, since the past may no longer apply. */
    reset(): void;
    /** Switch the visible timeframe and repaint. */
    setTF(tf: number): void;
    /** Tear down the chart and its listeners. */
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
    /** A value point, or a whitespace point (gap) while flat / avg unknown. */
    private toVwapPoint;
    /** VWAP for a candle at `key` (bucket seconds). Pre-open candles are null. */
    private vwapFor;
    private lastBarOhlc;
    private setOhlc;
}
