export type PriceFormat = {
    div: number;
    dec: number;
    fmtPx: (p: number) => string;
    parsePrice: (str: string) => number | null;
    fmtMoney: (ticks: number) => string;
};
export declare function fmtQty(q: number): string;
export declare function fmtTime(ts: number): string;
export declare function fmtTimeShort(ts: number): string;
/** Signed dollar string from a value that is already in currency units. */
export declare function fmtCurrency(v: number): string;
export declare function createPriceFormat(priceScale: number): PriceFormat;
