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
/**
 * Order ticket: side, type, price, qty, and a submit button that calls
 * `opts.onSubmit`.
 *
 * `new OrderEntry(host, opts)`, then `update(book)` on every book snapshot
 * so an untouched price field tracks the mid. `destroy()` when done.
 */
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
    /** Tear down this ticket's listeners. */
    destroy(): void;
    private quickFill;
    private setSide;
    private updateNotional;
    private submit;
}
