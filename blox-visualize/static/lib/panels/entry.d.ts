import type { PriceFormat } from "../format.js";
import type { Book, OrderKind, Side } from "../feed.js";
import { type PanelChrome } from "../dom.js";
export type PlaceOrder = {
    side: Side;
    kind: OrderKind;
    price: number;
    qty: number;
};
export type OrderEntryOptions = PanelChrome & {
    fmt: PriceFormat;
    symbol: string;
    getBook: () => Book;
    onSubmit: (o: PlaceOrder) => void;
};
export declare class OrderEntry {
    private readonly fmt;
    private readonly symbol;
    private readonly getBook;
    private readonly onSubmit;
    private readonly ac;
    private readonly buyBtn;
    private readonly sellBtn;
    private readonly kindSel;
    private readonly priceIn;
    private readonly qtyIn;
    private readonly notionalEl;
    private readonly submitBtn;
    private side;
    private priceTouched;
    constructor(host: HTMLElement, opts: OrderEntryOptions);
    /** Fill an untouched price field once the book is known. */
    update(book: Book): void;
    destroy(): void;
    private quickFill;
    private setSide;
    private updateNotional;
    private submit;
}
