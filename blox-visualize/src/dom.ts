// Panels build their own markup — that is the whole point of the library.
// This is the only piece they share: the optional header, and working out
// where status text ("12×14", "3.2/s", the OHLC readout) should be written.

export type PanelChrome = {
  /** Render a header with this title. Omit if the host draws its own chrome. */
  title?: string;
  /** Write status text here instead — for hosts with their own panel header. */
  statusEl?: HTMLElement | null;
};

/**
 * Replace `host`'s contents with the panel body, optionally prepending a
 * header. Returns the element status text belongs in, or null if nowhere.
 */
export function mount(host: HTMLElement, bodyHtml: string, opts: PanelChrome = {}): HTMLElement | null {
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
  return opts.statusEl ?? host.querySelector<HTMLElement>(".bx-status");
}

/** Non-null querySelector for markup this module just wrote. */
export function q<T extends HTMLElement>(host: HTMLElement, sel: string): T {
  const el = host.querySelector<T>(sel);
  if (!el) throw new Error(`blox-visualize: missing ${sel}`);
  return el;
}
