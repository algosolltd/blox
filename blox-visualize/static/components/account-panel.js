'use strict';
// Open + closed orders tables, including the inline reduce-qty editor.
//
//   import { AccountOrdersPanel } from './components/account-panel.js';
//   const acct = new AccountOrdersPanel(els, {
//     fmtPx, fmtQty, fmtTimeShort,
//     onCancel: id => sendJson({ type: 'cancel', id }),
//     onReduce: (id, qty) => sendJson({ type: 'reduce', id, qty }),
//   });
//   acct.setOpen(openOrders);
//   acct.setClosed(closedOrders);

const shortId = id => '#' + String(id).slice(-4);
const prettyReason = r => !r ? '' : ' · ' + r.replace(/([a-z])([A-Z])/g, '$1 $2').toLowerCase();

export class AccountOrdersPanel {
  constructor(els, { fmtPx, fmtQty, fmtTimeShort, onCancel, onReduce }) {
    this.els = els;
    this.fmtPx = fmtPx;
    this.fmtQty = fmtQty;
    this.fmtTimeShort = fmtTimeShort;
    this.onCancel = onCancel;
    this.onReduce = onReduce;
    this.open = [];
    this.closed = [];
    // Set while the inline qty editor is open, so a fresh setOpen() call
    // doesn't wipe the input out from under the user mid-type.
    this.reduceEdit = null;

    els.openRowsEl.addEventListener('click', e => this._onClick(e));
    els.openRowsEl.addEventListener('keydown', e => this._onKeydown(e));
    // focusout bubbles (blur doesn't) — clicking or tabbing away cancels the edit.
    els.openRowsEl.addEventListener('focusout', e => {
      if (e.target.closest('.o-reduce-input')) this._revertReduceEdit();
    });
  }

  setOpen(rows) {
    this.open = rows;
    this._renderOpen();
  }

  setClosed(rows) {
    this.closed = rows;
    this._renderClosed();
  }

  _buildActions(id, remaining) {
    const actions = document.createElement('div');
    actions.className = 'o-actions';
    if (remaining > 1) {
      const reduce = document.createElement('button');
      reduce.className = 'o-reduce';
      reduce.dataset.reduce = id;
      reduce.dataset.max = remaining;
      reduce.title = 'reduce resting qty';
      reduce.textContent = '−';
      actions.appendChild(reduce);
    }
    const cancel = document.createElement('button');
    cancel.className = 'o-cancel';
    cancel.dataset.cancel = id;
    cancel.title = 'cancel';
    cancel.textContent = '✕';
    actions.appendChild(cancel);
    return actions;
  }

  _renderOpen() {
    if (this.reduceEdit) return;
    const { openRowsEl, openCountEl } = this.els;
    const rows = this.open;
    if (openCountEl) openCountEl.textContent = rows.length ? `${rows.length} working` : '';
    if (!rows.length) {
      openRowsEl.innerHTML = '<div class="empty muted">no working orders — place one on the left</div>';
      return;
    }
    openRowsEl.textContent = '';
    for (const o of rows) {
      const row = document.createElement('div');
      row.className = 'o-row oc-grid';
      row.innerHTML =
        `<span class="muted">${this.fmtTimeShort(o.ts)}</span>` +
        `<span class="side-${o.side.toLowerCase()}">${o.side === 'B' ? 'BUY' : 'SELL'}</span>` +
        `<span>${o.kind === 'MARKET' ? 'MKT' : this.fmtPx(o.price)}</span>` +
        `<span class="r">${this.fmtQty(o.qty)}</span>` +
        `<span class="r">${o.filled ? this.fmtQty(o.filled) : '—'}</span>` +
        `<span class="muted">${o.kind}</span>` +
        `<span class="oid" title="${o.id}">${shortId(o.id)}</span>`;
      row.appendChild(this._buildActions(o.id, o.remaining));
      openRowsEl.appendChild(row);
    }
  }

  _renderClosed() {
    const { closedRowsEl } = this.els;
    const rows = this.closed;
    if (!rows.length) {
      closedRowsEl.innerHTML = '<div class="empty muted">nothing yet</div>';
      return;
    }
    closedRowsEl.textContent = '';
    for (const o of rows) {
      const row = document.createElement('div');
      row.className = 'o-row oc-grid-cl';
      row.innerHTML =
        `<span class="muted">${this.fmtTimeShort(o.tsEnd || o.ts)}</span>` +
        `<span class="side-${o.side.toLowerCase()}">${o.side === 'B' ? 'BUY' : 'SELL'}</span>` +
        `<span>${o.filled ? this.fmtPx(o.avgFill) : (o.price ? this.fmtPx(o.price) : '—')}</span>` +
        `<span class="r">${this.fmtQty(o.filled)}/${this.fmtQty(o.qty)}</span>` +
        `<span><span class="st-chip st-${o.status}"${o.reason ? ` title="${o.status}${prettyReason(o.reason)}"` : ''}>${o.status}</span></span>` +
        `<span class="oid" title="${o.id}">${shortId(o.id)}</span>`;
      closedRowsEl.appendChild(row);
    }
  }

  // Closes the inline qty editor and repaints from whatever state arrived
  // while it was open. Safe to call twice (Enter reverts, then the input's
  // own focusout fires on the now-detached node and finds nothing to do).
  _revertReduceEdit() {
    if (!this.reduceEdit) return;
    this.reduceEdit = null;
    this._renderOpen();
  }

  _onClick(e) {
    const btn = e.target.closest('[data-cancel]');
    if (btn) this.onCancel(+btn.dataset.cancel);

    const rbtn = e.target.closest('[data-reduce]');
    if (rbtn) {
      // A number input in the row, not window.prompt(): prompt() blocks the
      // whole page's JS thread, so the book, chart and WebSocket all appear
      // to freeze for as long as the dialog is open.
      const id = rbtn.dataset.reduce, max = +rbtn.dataset.max;
      const actions = rbtn.closest('.o-actions');
      actions.innerHTML = '';
      const input = document.createElement('input');
      // type=text + strip, not type=number: number inputs still let you type
      // "e", "+", "-" and report the value as "" instead of rejecting the key.
      input.type = 'text';
      input.inputMode = 'numeric';
      input.className = 'o-reduce-input';
      input.value = max - 1;
      input.dataset.reduceInput = id;
      input.dataset.max = max;
      input.addEventListener('input', () => {
        const v = input.value.replace(/\D/g, '');
        input.value = +v > max - 1 ? String(max - 1) : v;
      });
      this.reduceEdit = id;
      actions.appendChild(input);
      input.focus();
      input.select();
    }
  }

  _onKeydown(e) {
    const input = e.target.closest('.o-reduce-input');
    if (!input) return;
    if (e.key === 'Enter') {
      const max = +input.dataset.max;
      const qty = parseInt(input.value, 10);
      if (Number.isFinite(qty) && qty > 0 && qty < max) {
        this.onReduce(+input.dataset.reduceInput, qty);
      }
      this._revertReduceEdit();
    } else if (e.key === 'Escape') {
      this._revertReduceEdit();
    }
  }
}
