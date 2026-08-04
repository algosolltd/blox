// Working and closed orders. Cancel/reduce are delegated off the row
// container, so re-rendering the table never re-binds a listener.
import { fmtQty, fmtTimeShort } from "../format.js";
import { mount, q } from "../dom.js";
const shortId = (id) => "#" + String(id).slice(-4);
const prettyReason = (r) => !r ? "" : " · " + r.replace(/([a-z])([A-Z])/g, "$1 $2").toLowerCase();
/**
 * Working and closed orders, in two independently resizable tables (drag
 * the bar between them). Cancel and reduce-qty are wired to `onCancel` /
 * `onReduce` from `opts`.
 *
 * `new Orders(host, opts)`, then `update({ open, closed })` — pass both
 * arrays even when one is empty; a field left `undefined` is "unchanged",
 * not "cleared". `destroy()` when done.
 */
export class Orders {
    openEl;
    closedEl;
    statusEl;
    fmt;
    onCancel;
    onReduce;
    ac = new AbortController();
    open = [];
    closed = [];
    reduceEdit = null;
    constructor(host, opts) {
        this.fmt = opts.fmt;
        this.onCancel = opts.onCancel;
        this.onReduce = opts.onReduce;
        this.statusEl = mount(host, `
      <div class="o-section o-section-open">
        <div class="cols mono muted oc-grid">
          <span>Time</span><span>Side</span><span>Price</span><span class="r">Qty</span>
          <span class="r">Filled</span><span>Type</span><span>ID</span><span></span>
        </div>
        <div class="rows o-open"><div class="empty muted">no working orders</div></div>
      </div>
      <div class="o-splitter" title="drag to resize"></div>
      <div class="o-section o-section-closed">
        <div class="cols mono muted oc-grid-cl">
          <span>Time</span><span>Side</span><span>Avg px</span><span class="r">Filled</span>
          <span>Status</span><span>ID</span>
        </div>
        <div class="rows o-closed"><div class="empty muted">nothing yet</div></div>
      </div>`, opts);
        this.openEl = q(host, ".o-open");
        this.closedEl = q(host, ".o-closed");
        const signal = this.ac.signal;
        this.openEl.addEventListener("click", (e) => this.onClick(e), { signal });
        this.openEl.addEventListener("keydown", (e) => this.onKeydown(e), { signal });
        this.initSplitter(q(host, ".o-splitter"), q(host, ".o-section-closed"));
        this.openEl.addEventListener("focusout", (e) => {
            if (e.target.closest(".o-reduce-input"))
                this.revertReduceEdit();
        }, { signal });
    }
    /** Replace `open` and/or `closed` wholesale. Omit a field to leave it as-is. */
    update(a) {
        if (a.open) {
            this.open = a.open;
            this.renderOpen();
        }
        if (a.closed) {
            this.closed = a.closed;
            this.renderClosed();
        }
    }
    /** Clear both tables — a reconnect, since the old lists may be stale. */
    reset() {
        this.reduceEdit = null;
        this.update({ open: [], closed: [] });
    }
    /** Tear down this panel's listeners. */
    destroy() {
        this.ac.abort();
    }
    renderOpen() {
        // Mid-edit the input owns the row; re-rendering would drop what was typed.
        if (this.reduceEdit != null)
            return;
        const rows = this.open;
        if (this.statusEl)
            this.statusEl.textContent = rows.length ? `${rows.length} working` : "";
        if (!rows.length) {
            this.openEl.innerHTML = '<div class="empty muted">no working orders</div>';
            return;
        }
        this.openEl.textContent = "";
        for (const o of rows) {
            const row = document.createElement("div");
            row.className = "o-row oc-grid";
            row.innerHTML =
                `<span class="muted">${fmtTimeShort(o.ts)}</span>` +
                    `<span class="side-${o.side.toLowerCase()}">${o.side === "B" ? "BUY" : "SELL"}</span>` +
                    `<span>${o.kind === "MARKET" ? "MKT" : this.fmt.fmtPx(o.price)}</span>` +
                    `<span class="r">${fmtQty(o.qty)}</span>` +
                    `<span class="r">${o.filled ? fmtQty(o.filled) : "—"}</span>` +
                    `<span class="muted">${o.kind}</span>` +
                    `<span class="oid" title="${o.id}">${shortId(o.id)}</span>`;
            row.appendChild(this.buildActions(o.id, o.remaining));
            this.openEl.appendChild(row);
        }
    }
    renderClosed() {
        const rows = this.closed;
        if (!rows.length) {
            this.closedEl.innerHTML = '<div class="empty muted">nothing yet</div>';
            return;
        }
        this.closedEl.textContent = "";
        for (const o of rows) {
            const row = document.createElement("div");
            row.className = "o-row oc-grid-cl";
            row.innerHTML =
                `<span class="muted">${fmtTimeShort(o.tsEnd || o.ts)}</span>` +
                    `<span class="side-${o.side.toLowerCase()}">${o.side === "B" ? "BUY" : "SELL"}</span>` +
                    `<span>${o.filled ? this.fmt.fmtPx(o.avgFill) : (o.price ? this.fmt.fmtPx(o.price) : "—")}</span>` +
                    `<span class="r">${fmtQty(o.filled)}/${fmtQty(o.qty)}</span>` +
                    `<span><span class="st-chip st-${o.status}"${o.reason ? ` title="${o.status}${prettyReason(o.reason)}"` : ""}>${o.status}</span></span>` +
                    `<span class="oid" title="${o.id}">${shortId(o.id)}</span>`;
            this.closedEl.appendChild(row);
        }
    }
    buildActions(id, remaining) {
        const actions = document.createElement("div");
        actions.className = "o-actions";
        if (remaining > 1) {
            const reduce = document.createElement("button");
            reduce.type = "button";
            reduce.className = "o-reduce";
            reduce.dataset.reduce = String(id);
            reduce.dataset.max = String(remaining);
            reduce.title = "reduce resting qty";
            reduce.textContent = "−";
            actions.appendChild(reduce);
        }
        const cancel = document.createElement("button");
        cancel.type = "button";
        cancel.className = "o-cancel";
        cancel.dataset.cancel = String(id);
        cancel.title = "cancel";
        cancel.textContent = "✕";
        actions.appendChild(cancel);
        return actions;
    }
    /** Drag the bar between the two tables to resize the closed-orders section. */
    initSplitter(bar, closedSection) {
        const MIN = 60; // matches the section's own min-height
        bar.addEventListener("pointerdown", (e) => {
            bar.setPointerCapture(e.pointerId);
            const startY = e.clientY;
            const startH = closedSection.getBoundingClientRect().height;
            const max = closedSection.parentElement.getBoundingClientRect().height - bar.getBoundingClientRect().height - MIN;
            const onMove = (e) => {
                const h = Math.min(max, Math.max(MIN, startH - (e.clientY - startY)));
                closedSection.style.flexBasis = `${h}px`;
            };
            const onUp = () => bar.removeEventListener("pointermove", onMove);
            bar.addEventListener("pointermove", onMove);
            bar.addEventListener("pointerup", onUp, { once: true });
        }, { signal: this.ac.signal });
    }
    revertReduceEdit() {
        if (this.reduceEdit == null)
            return;
        this.reduceEdit = null;
        this.renderOpen();
    }
    onClick(e) {
        const target = e.target;
        const cancel = target.closest("[data-cancel]");
        if (cancel)
            this.onCancel(+cancel.dataset.cancel);
        const rbtn = target.closest("[data-reduce]");
        if (!rbtn)
            return;
        const id = +rbtn.dataset.reduce;
        const max = +rbtn.dataset.max;
        const actions = rbtn.closest(".o-actions");
        actions.innerHTML = "";
        const input = document.createElement("input");
        input.type = "text";
        input.inputMode = "numeric";
        input.className = "o-reduce-input";
        input.value = String(max - 1);
        input.dataset.reduceInput = String(id);
        input.dataset.max = String(max);
        input.addEventListener("input", () => {
            const v = input.value.replace(/\D/g, "");
            input.value = +v > max - 1 ? String(max - 1) : v;
        });
        this.reduceEdit = id;
        actions.appendChild(input);
        input.focus();
        input.select();
    }
    onKeydown(e) {
        const input = e.target.closest(".o-reduce-input");
        if (!input)
            return;
        if (e.key === "Enter") {
            const max = +input.dataset.max;
            const qty = parseInt(input.value, 10);
            if (Number.isFinite(qty) && qty > 0 && qty < max) {
                this.onReduce(+input.dataset.reduceInput, qty);
            }
            this.revertReduceEdit();
        }
        else if (e.key === "Escape") {
            this.revertReduceEdit();
        }
    }
}
//# sourceMappingURL=orders.js.map