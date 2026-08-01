// Price formatting. Prices are integer ticks everywhere on the wire — the
// instrument's priceScale says how many decimals to render them at, and
// parsePrice is the inverse for anything the user types.
export function fmtQty(q) {
    return q.toLocaleString("en-US");
}
export function fmtTime(ts) {
    const d = new Date(ts);
    const p = (n) => String(n).padStart(2, "0");
    return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}.${String(d.getMilliseconds()).padStart(3, "0")}`;
}
export function fmtTimeShort(ts) {
    return fmtTime(ts).slice(0, 8);
}
/** Signed dollar string from a value that is already in currency units. */
export function fmtCurrency(v) {
    return (v < 0 ? "-" : "+") + "$" +
        Math.abs(v).toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 });
}
export function createPriceFormat(priceScale) {
    const div = 10 ** priceScale;
    const dec = priceScale;
    const fmtPx = (p) => (p / div).toLocaleString("en-US", { minimumFractionDigits: dec, maximumFractionDigits: dec });
    const parsePrice = (str) => {
        const m = /^\s*(\d+)(?:\.(\d+))?\s*$/.exec(str);
        if (!m)
            return null;
        const frac = m[2] || "";
        if (frac.length > dec)
            return null;
        return parseInt(m[1] + frac.padEnd(dec, "0"), 10);
    };
    // Takes ticks. Currency values (PnL) are already scaled — use fmtCurrency.
    const fmtMoney = (tl) => fmtCurrency(tl / div);
    return { div, dec, fmtPx, parsePrice, fmtMoney };
}
//# sourceMappingURL=format.js.map