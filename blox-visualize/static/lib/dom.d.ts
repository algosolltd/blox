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
export declare function mount(host: HTMLElement, bodyHtml: string, opts?: PanelChrome): HTMLElement | null;
/** Non-null querySelector for markup this module just wrote. */
export declare function q<T extends HTMLElement>(host: HTMLElement, sel: string): T;
