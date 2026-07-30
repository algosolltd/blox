'use strict';
// Formatting helpers shared by the panels. Framework-free, no DOM access —
// safe to import from any codebase.

export const fmtQty = q => q.toLocaleString('en-US');

export const fmtTime = ts => {
  const d = new Date(ts);
  const p = n => String(n).padStart(2, '0');
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}.${String(d.getMilliseconds()).padStart(3, '0')}`;
};

export const fmtTimeShort = ts => fmtTime(ts).slice(0, 8);

// Integer ticks <-> display string for a given price scale (decimal places).
// Prices are always integers on the wire (no floats — see docs/CORE.md D10).
export function createPriceFormat(priceScale) {
  const div = 10 ** priceScale;
  const dec = priceScale;

  const fmtPx = p => (p / div).toLocaleString('en-US', { minimumFractionDigits: dec, maximumFractionDigits: dec });

  // Rejects excess precision rather than truncating it.
  const parsePrice = str => {
    const m = /^\s*(\d+)(?:\.(\d+))?\s*$/.exec(str);
    if (!m) return null;
    const frac = m[2] || '';
    if (frac.length > dec) return null;
    return parseInt(m[1] + frac.padEnd(dec, '0'), 10);
  };

  const fmtMoney = tl => {
    const sign = tl < 0 ? '-' : '+';
    const v = Math.abs(tl) / div;
    return sign + '$' + v.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 });
  };

  return { div, dec, fmtPx, parsePrice, fmtMoney };
}
