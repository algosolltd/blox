'use strict';
// Buy/sell order-entry form. Takes explicit element refs (no hardcoded IDs),
// so more than one can live on a page.
//
//   import { OrderEntryForm } from './components/order-entry.js';
//   const entry = new OrderEntryForm(els, {
//     fmtPx, parsePrice, fmtMoney, symbol: 'BLOX',
//     getBook: () => currentBook,
//     onSubmit: order => sendJson({ type: 'order', ...order }),
//   });
//   entry.maybePrefillFromBook(book); // call after each book update

export class OrderEntryForm {
  constructor(els, { fmtPx, parsePrice, fmtMoney, symbol, getBook, onSubmit }) {
    this.els = els;
    this.fmtPx = fmtPx;
    this.parsePrice = parsePrice;
    this.fmtMoney = fmtMoney;
    this.symbol = symbol;
    this.getBook = getBook;
    this.onSubmit = onSubmit;
    this.side = 'B';
    this.priceTouched = false;

    els.buyBtn.addEventListener('click', () => this._setSide('B'));
    els.sellBtn.addEventListener('click', () => this._setSide('S'));
    els.kindSelect.addEventListener('change', () => {
      const mkt = els.kindSelect.value === 'MARKET';
      els.priceInput.disabled = mkt;
      els.priceInput.placeholder = mkt ? 'market' : '';
      this._updateNotional();
    });
    (els.quickBtns || []).forEach(b => {
      b.addEventListener('click', () => {
        const book = this.getBook();
        const bb = book.bids[0], ba = book.asks[0];
        let px = null;
        if (b.dataset.q === 'bid' && bb) px = bb[0];
        if (b.dataset.q === 'ask' && ba) px = ba[0];
        if (b.dataset.q === 'mid' && bb && ba) px = Math.round((bb[0] + ba[0]) / 2);
        if (px != null) {
          els.priceInput.value = this.fmtPx(px);
          els.priceInput.classList.remove('err');
          this.priceTouched = true;
          this._updateNotional();
        }
      });
    });
    els.priceInput.addEventListener('input', () => { this.priceTouched = true; this._updateNotional(); });
    els.qtyInput.addEventListener('input', () => this._updateNotional());
    els.submitBtn.addEventListener('click', () => this._submit());
    [els.priceInput, els.qtyInput].forEach(inp =>
      inp.addEventListener('keydown', e => { if (e.key === 'Enter') this._submit(); }));

    this._setSide('B');
  }

  // Prefill the price box with the mid until the user types their own.
  maybePrefillFromBook(book) {
    if (this.priceTouched || this.els.priceInput.value) return;
    const bb = book.bids[0], ba = book.asks[0];
    if (bb && ba) this.els.priceInput.value = this.fmtPx(Math.round((bb[0] + ba[0]) / 2));
    this._updateNotional();
  }

  _setSide(side) {
    this.side = side;
    const { buyBtn, sellBtn, submitBtn } = this.els;
    buyBtn.classList.toggle('on', side === 'B');
    sellBtn.classList.toggle('on', side === 'S');
    submitBtn.textContent = (side === 'B' ? 'BUY ' : 'SELL ') + this.symbol;
    submitBtn.className = side === 'B' ? 'buy' : 'sell';
    this._updateNotional();
  }

  _updateNotional() {
    const { kindSelect, priceInput, qtyInput, notionalEl } = this.els;
    const qty = parseInt(qtyInput.value, 10);
    let px = this.parsePrice(priceInput.value);
    if (kindSelect.value === 'MARKET') {
      const book = this.getBook();
      const lvl = this.side === 'B' ? book.asks[0] : book.bids[0];
      px = lvl ? lvl[0] : null;
    }
    notionalEl.textContent = (px != null && qty > 0) ? this.fmtMoney(px * qty).slice(1) : '—';
  }

  _submit() {
    const { kindSelect, priceInput, qtyInput } = this.els;
    const kind = kindSelect.value;
    const qty = parseInt(qtyInput.value, 10);
    let price = 0;
    if (!(qty > 0)) { qtyInput.classList.add('err'); return; }
    qtyInput.classList.remove('err');
    if (kind !== 'MARKET') {
      price = this.parsePrice(priceInput.value);
      if (price == null || price <= 0) { priceInput.classList.add('err'); return; }
      priceInput.classList.remove('err');
    }
    this.onSubmit({ side: this.side, kind, price, qty });
  }
}
