// Magnetic edges for the panel canvas. Pure geometry, no DOM — ported
// directly from the tradearena trading portal's snap.ts so the drag/resize
// feel matches exactly.
//
// Two magnet families, Figma-style: edges snap to edges (mine to theirs, and
// to the canvas sides); centres snap to centres. Edge alignment wins when
// both are in range; centre is the fallback — that's what makes "drop it
// next to that one" feel right instead of fighting you.

export const SNAP_RADIUS = 9; // px of magnetism — ~half a fingertip

export function applyHandle(base, h, dx, dy, ctx) {
  const { W, H, minW, minH } = ctx;
  if (h === 'move') {
    return {
      x: Math.max(0, Math.min(base.x + dx, W - base.w)),
      y: Math.max(0, Math.min(base.y + dy, H - base.h)),
      w: base.w,
      h: base.h,
    };
  }
  let { x, y, w, h: hh } = base;
  if (h.includes('w')) {
    const nx = Math.max(0, Math.min(base.x + dx, base.x + base.w - minW));
    w = base.x + base.w - nx;
    x = nx;
  }
  if (h.includes('e')) w = Math.max(minW, Math.min(base.w + dx, W - base.x));
  if (h.includes('n')) {
    const ny = Math.max(0, Math.min(base.y + dy, base.y + base.h - minH));
    hh = base.y + base.h - ny;
    y = ny;
  }
  if (h.includes('s')) hh = Math.max(minH, Math.min(base.h + dy, H - base.y));
  return { x, y, w, h: hh };
}

function nearest(edges, targets) {
  let bd = Infinity;
  let at = 0;
  for (const e of edges) {
    for (const t of targets) {
      const d = t - e;
      if (Math.abs(d) < Math.abs(bd)) { bd = d; at = t; }
    }
  }
  return Math.abs(bd) <= SNAP_RADIUS ? { d: bd, at } : null;
}

// Pull `r` onto the nearest magnet and report the guide line that caught it
// (canvas-space, or null). `enabled=false` (Alt held) bypasses everything.
export function magnet(r, h, ctx, enabled = true) {
  if (!enabled) return { rect: r, gx: null, gy: null };

  const ex = [0, ctx.W];
  const ey = [0, ctx.H];
  const cx = [ctx.W / 2];
  const cy = [ctx.H / 2];
  for (const o of ctx.others) {
    ex.push(o.x, o.x + o.w);
    ey.push(o.y, o.y + o.h);
    cx.push(o.x + o.w / 2);
    cy.push(o.y + o.h / 2);
  }

  let { x, y, w } = r;
  let hh = r.h;
  let gx = null;
  let gy = null;

  if (h === 'move') {
    const sx = nearest([x, x + w], ex) ?? nearest([x + w / 2], cx);
    if (sx) { x += sx.d; gx = sx.at; }
    const sy = nearest([y, y + hh], ey) ?? nearest([y + hh / 2], cy);
    if (sy) { y += sy.d; gy = sy.at; }
  } else {
    if (h.includes('w')) {
      const s = nearest([x], ex);
      if (s && w - s.d >= ctx.minW) { x += s.d; w -= s.d; gx = s.at; }
    }
    if (h.includes('e')) {
      const s = nearest([x + w], ex);
      if (s && w + s.d >= ctx.minW && x + w + s.d <= ctx.W) { w += s.d; gx = s.at; }
    }
    if (h.includes('n')) {
      const s = nearest([y], ey);
      if (s && hh - s.d >= ctx.minH) { y += s.d; hh -= s.d; gy = s.at; }
    }
    if (h.includes('s')) {
      const s = nearest([y + hh], ey);
      if (s && hh + s.d >= ctx.minH && y + hh + s.d <= ctx.H) { hh += s.d; gy = s.at; }
    }
  }

  return { rect: { x: Math.round(x), y: Math.round(y), w: Math.round(w), h: Math.round(hh) }, gx, gy };
}
