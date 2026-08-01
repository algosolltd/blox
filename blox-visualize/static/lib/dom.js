// Panels build their own markup — that is the whole point of the library.
// This is the only piece they share: the optional header, and working out
// where status text ("12×14", "3.2/s", the OHLC readout) should be written.
/**
 * Replace `host`'s contents with the panel body, optionally prepending a
 * header. Returns the element status text belongs in, or null if nowhere.
 */
export function mount(host, bodyHtml, opts = {}) {
    host.classList.add("bx-panel");
    host.innerHTML = bodyHtml;
    if (opts.title != null) {
        const head = document.createElement("header");
        head.className = "bx-head";
        const title = document.createElement("span");
        title.className = "bx-title";
        title.textContent = opts.title; // textContent, not innerHTML — a title may be data
        const spacer = document.createElement("span");
        spacer.className = "bx-spacer";
        const status = document.createElement("span");
        status.className = "bx-status mono";
        head.append(title, spacer, status);
        host.prepend(head);
    }
    return opts.statusEl ?? host.querySelector(".bx-status");
}
/** Non-null querySelector for markup this module just wrote. */
export function q(host, sel) {
    const el = host.querySelector(sel);
    if (!el)
        throw new Error(`blox-visualize: missing ${sel}`);
    return el;
}
//# sourceMappingURL=dom.js.map