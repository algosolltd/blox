// Order ticket. Reads the live book through a callback rather than a prop, so
// a book update ten times a second never rebuilds the form under the user's
// cursor or wipes a half-typed price.

import type { PriceFormat } from "../format.js";
import type { Book, OrderKind, Side } from "../feed.js";
import { mount, q, type PanelChrome } from "../dom.js";

export type PlaceOrder = { side: Side; kind: OrderKind; price: number; qty: number };

export type OrderEntryOptions = PanelChrome & {
  fmt: PriceFormat;
  symbol: string;
  getBook: () => Book;
  onSubmit: (o: PlaceOrder) => void;
};

const KINDS: [OrderKind, string][] = [
  ["LIMIT", "Limit"], ["MARKET", "Market"], ["IOC", "IOC"], ["FOK", "FOK"], ["POST", "Post-only"],
];

export class OrderEntry {
  private readonly fmt: PriceFormat;
  private readonly symbol: string;
  private readonly getBook: () => Book;
  private readonly onSubmit: (o: PlaceOrder) => void;
  private readonly ac = new AbortController();

  private readonly buyBtn: HTMLButtonElement;
  private readonly sellBtn: HTMLButtonElement;
  private readonly kindSel: HTMLSelectElement;
  private readonly priceIn: HTMLInputElement;
  private readonly qtyIn: HTMLInputElement;
  private readonly notionalEl: HTMLElement;
  private readonly submitBtn: HTMLButtonElement;

  private side: Side = "B";
  private priceTouched = false;

  constructor(host: HTMLElement, opts: OrderEntryOptions) {
    this.fmt = opts.fmt;
    this.symbol = opts.symbol;
    this.getBook = opts.getBook;
    this.onSubmit = opts.onSubmit;

    mount(host, `
      <div class="oe">
        <div class="oe-side">
          <button type="button" class="oe-buy on">BUY</button>
          <button type="button" class="oe-sell">SELL</button>
        </div>
        <div class="oe-row">
          <label>Type</label>
          <select class="oe-kind mono">
            ${KINDS.map(([v, label]) => `<option value="${v}">${label}</option>`).join("")}
          </select>
        </div>
        <div class="oe-row">
          <label>Price</label>
          <input class="oe-price mono" inputmode="decimal" autocomplete="off" spellcheck="false">
        </div>
        <div class="oe-quick">
          <button type="button" data-q="bid">best bid</button>
          <button type="button" data-q="mid">mid</button>
          <button type="button" data-q="ask">best ask</button>
        </div>
        <div class="oe-row">
          <label>Qty</label>
          <input class="oe-qty mono" inputmode="numeric" autocomplete="off" value="10">
        </div>
        <div class="oe-est mono muted">≈ <span class="oe-notional">—</span></div>
        <button type="button" class="oe-submit buy"></button>
      </div>`, opts);

    this.buyBtn = q<HTMLButtonElement>(host, ".oe-buy");
    this.sellBtn = q<HTMLButtonElement>(host, ".oe-sell");
    this.kindSel = q<HTMLSelectElement>(host, ".oe-kind");
    this.priceIn = q<HTMLInputElement>(host, ".oe-price");
    this.qtyIn = q<HTMLInputElement>(host, ".oe-qty");
    this.notionalEl = q(host, ".oe-notional");
    this.submitBtn = q<HTMLButtonElement>(host, ".oe-submit");

    const signal = this.ac.signal;
    this.buyBtn.addEventListener("click", () => this.setSide("B"), { signal });
    this.sellBtn.addEventListener("click", () => this.setSide("S"), { signal });
    this.kindSel.addEventListener("change", () => {
      const mkt = this.kindSel.value === "MARKET";
      this.priceIn.disabled = mkt;
      this.priceIn.placeholder = mkt ? "market" : "";
      this.updateNotional();
    }, { signal });
    // Scoped to this host — a document-wide query grabs whichever ticket
    // happens to be first in the DOM when more than one is mounted.
    q(host, ".oe-quick").addEventListener("click", (e) => {
      const b = (e.target as HTMLElement).closest<HTMLElement>("[data-q]");
      if (b) this.quickFill(b.dataset.q!);
    }, { signal });
    this.priceIn.addEventListener("input", () => { this.priceTouched = true; this.updateNotional(); }, { signal });
    this.qtyIn.addEventListener("input", () => this.updateNotional(), { signal });
    this.submitBtn.addEventListener("click", () => this.submit(), { signal });
    for (const inp of [this.priceIn, this.qtyIn]) {
      inp.addEventListener("keydown", (e) => { if (e.key === "Enter") this.submit(); }, { signal });
    }

    this.setSide("B");
  }

  /** Fill an untouched price field once the book is known. */
  update(book: Book): void {
    if (this.priceTouched || this.priceIn.value) return;
    const bb = book.bids[0], ba = book.asks[0];
    if (bb && ba) this.priceIn.value = this.fmt.fmtPx(Math.round((bb[0] + ba[0]) / 2));
    this.updateNotional();
  }

  destroy(): void {
    this.ac.abort();
  }

  private quickFill(which: string): void {
    const book = this.getBook();
    const bb = book.bids[0], ba = book.asks[0];
    let px: number | null = null;
    if (which === "bid" && bb) px = bb[0];
    if (which === "ask" && ba) px = ba[0];
    if (which === "mid" && bb && ba) px = Math.round((bb[0] + ba[0]) / 2);
    if (px == null) return;
    this.priceIn.value = this.fmt.fmtPx(px);
    this.priceIn.classList.remove("err");
    this.priceTouched = true;
    this.updateNotional();
  }

  private setSide(side: Side): void {
    this.side = side;
    this.buyBtn.classList.toggle("on", side === "B");
    this.sellBtn.classList.toggle("on", side === "S");
    this.submitBtn.textContent = (side === "B" ? "BUY " : "SELL ") + this.symbol;
    this.submitBtn.className = "oe-submit " + (side === "B" ? "buy" : "sell");
    this.updateNotional();
  }

  private updateNotional(): void {
    const qty = parseInt(this.qtyIn.value, 10);
    let px: number | null = this.fmt.parsePrice(this.priceIn.value);
    if (this.kindSel.value === "MARKET") {
      const book = this.getBook();
      const lvl = this.side === "B" ? book.asks[0] : book.bids[0];
      px = lvl ? lvl[0] : null;
    }
    this.notionalEl.textContent = (px != null && qty > 0) ? this.fmt.fmtMoney(px * qty).slice(1) : "—";
  }

  private submit(): void {
    const kind = this.kindSel.value as OrderKind;
    const qty = parseInt(this.qtyIn.value, 10);
    if (!(qty > 0)) { this.qtyIn.classList.add("err"); return; }
    this.qtyIn.classList.remove("err");
    let price = 0;
    if (kind !== "MARKET") {
      const parsed = this.fmt.parsePrice(this.priceIn.value);
      if (parsed == null || parsed <= 0) { this.priceIn.classList.add("err"); return; }
      price = parsed;
      this.priceIn.classList.remove("err");
    }
    this.onSubmit({ side: this.side, kind, price, qty });
  }
}
