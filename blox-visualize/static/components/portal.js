// Portal — a free-form draggable/resizable panel canvas, matching the
// gesture vocabulary of the tradearena trading portal: drag a header to
// move, drag any edge or corner to resize, double-click a header to
// maximise, hold Alt to bypass the magnetic edges, arrow keys to nudge a
// focused panel, a dock to show/hide panels.
//
// Ported from that portal's use-portal-layout.ts / PortalPanel.tsx /
// default-layout.ts, minus React: panels here are plain DOM nodes, so a
// state change is a style write, not a re-render. A gesture writes
// left/top/width/height straight onto the node on every pointermove and
// commits one rect on pointerup — the same "zero re-renders mid-drag" trick,
// simpler here because there was no render tree to avoid re-rendering.
//
// ponytail: a hidden panel is `display:none`, not unmounted — toggling one
// back on doesn't need to re-subscribe it to the feed. Revisit only if a
// hidden panel's idle work shows up in a profile; at this tick rate it won't.
// Likewise minW/minH live on PANEL_META, not in the persisted layout — this
// panel set is fixed, so there's nothing per-instance to override yet.

import { applyHandle, magnet } from './snap.js';

export const HEAD_H = 34; // keep in sync with .panel-head height in style.css
const RESIZE_HANDLES = ['n', 's', 'e', 'w', 'ne', 'nw', 'se', 'sw'];
const STORAGE_KEY = 'blox-viz-portal-v1';

const TOP = 0.62; // height fraction of the upper band (chart/trades/depth/book)

export const PANEL_META = [
  { id: 'chart', label: 'Market Chart', icon: '📈', frac: { x: 0, y: 0, w: 0.44, h: TOP }, minW: 380, minH: 260 },
  { id: 'trades', label: 'Market Trades', icon: '💹', frac: { x: 0.44, y: 0, w: 0.18, h: TOP }, minW: 190, minH: 200 },
  { id: 'depth', label: 'Depth Chart', icon: '📊', frac: { x: 0.62, y: 0, w: 0.20, h: TOP }, minW: 220, minH: 200 },
  { id: 'book', label: 'Order Book', icon: '📘', frac: { x: 0.82, y: 0, w: 0.18, h: TOP }, minW: 210, minH: 200 },
  { id: 'entry', label: 'Order Entry', icon: '✏️', frac: { x: 0, y: TOP, w: 0.16, h: 1 - TOP }, minW: 250, minH: 320 },
  { id: 'orders', label: 'Orders', icon: '📋', frac: { x: 0.16, y: TOP, w: 0.56, h: 1 - TOP }, minW: 340, minH: 220 },
  { id: 'pnl', label: 'Live PnL', icon: '💰', frac: { x: 0.72, y: TOP, w: 0.28, h: 1 - TOP }, minW: 260, minH: 220 },
];

// Fit `want` into `total` without going under `mins`, taking the excess from
// whoever has the most slack. Ported from default-layout.ts's fit().
function fit(want, mins, total) {
  const out = [...want];
  for (let pass = 0; pass < 4; pass++) {
    const excess = out.reduce((a, b) => a + b, 0) - total;
    if (excess <= 0) break;
    const slack = out.map((v, i) => v - mins[i]);
    const slackSum = slack.reduce((a, b) => a + b, 0);
    if (slackSum <= 0) break;
    for (let i = 0; i < out.length; i++) {
      out[i] = Math.max(mins[i], out[i] - Math.round((excess * slack[i]) / slackSum));
    }
  }
  const sum = out.reduce((a, b) => a + b, 0);
  if (sum > total) for (let i = 0; i < out.length; i++) out[i] = Math.floor((out[i] * total) / sum);
  const drift = total - out.reduce((a, b) => a + b, 0);
  if (drift !== 0) {
    let widest = 0;
    for (let i = 1; i < out.length; i++) if (out[i] > out[widest]) widest = i;
    out[widest] += drift;
  }
  return out;
}

function buildDefaultLayout(W, H) {
  const panels = {};
  const rows = [0, TOP].map((y) => PANEL_META.filter((m) => m.frac.y === y));
  const rowH = fit(
    rows.map((r) => Math.round(r[0].frac.h * H)),
    rows.map((r) => Math.max(...r.map((m) => m.minH))),
    H,
  );
  let y = 0;
  rows.forEach((row, ri) => {
    const ws = fit(row.map((m) => Math.round(m.frac.w * W)), row.map((m) => m.minW), W);
    let x = 0;
    row.forEach((m, i) => {
      panels[m.id] = { visible: true, rect: { x, y, w: ws[i], h: rowH[ri] }, z: PANEL_META.indexOf(m), collapsed: false };
      x += ws[i];
    });
    y += rowH[ri];
  });
  return { version: 1, panels };
}

// Pulls a stored (or default-built) layout back inside a canvas of (W,H).
// A panel missing from the stored blob (older version, panel set changed)
// falls back to its slot in a freshly built default rather than vanishing.
function clampLayoutToCanvas(layout, W, H) {
  const fresh = buildDefaultLayout(W, H);
  const panels = {};
  for (const m of PANEL_META) {
    const p = layout.panels[m.id];
    if (!p) { panels[m.id] = fresh.panels[m.id]; continue; }
    const w = Math.min(Math.max(p.rect.w, m.minW), W);
    const h = Math.min(Math.max(p.rect.h, m.minH), H);
    const x = Math.max(0, Math.min(p.rect.x, W - w));
    const yv = Math.max(0, Math.min(p.rect.y, H - h));
    panels[m.id] = { ...p, rect: { x, y: yv, w, h } };
  }
  return { version: 1, panels };
}

function loadStored() {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw);
    return parsed.version === 1 ? parsed : null;
  } catch {
    return null;
  }
}

export class Portal {
  constructor({ canvas, guideV, guideH, dock }) {
    this.canvasEl = canvas;
    this.guideV = guideV;
    this.guideH = guideH;
    this.dockEl = dock;
    this.panels = new Map();
    this.layout = null;
    this.canvasSize = { w: 0, h: 0 };
    this.snapOn = true;
    this.locked = false;
    this._saveTimer = null;
    this._hydrated = false;

    this._ro = new ResizeObserver(() => this._onResize());
    this._ro.observe(canvas);
  }

  // Register a panel. `mount(bodyEl, chromeEl)` is called once, synchronously,
  // and may return a destroy function (not currently invoked — see the
  // ponytail note above; kept so the shape is ready if that changes).
  register(id, mount) {
    const meta = PANEL_META.find((m) => m.id === id);
    const el = document.createElement('section');
    el.className = 'portal-panel';
    el.dataset.panelId = id;
    el.innerHTML = `
      <header class="panel-head" tabindex="0" role="toolbar" aria-label="${meta.label} panel">
        <span class="panel-grip" aria-hidden="true"></span>
        <span class="panel-icon" aria-hidden="true">${meta.icon}</span>
        <span class="panel-title on">${meta.label}</span>
        <span class="panel-chrome mono muted"></span>
        <span class="spacer"></span>
        <div class="panel-actions" data-nodrag>
          <button class="panel-action" data-act="collapse" title="Collapse panel" aria-label="Collapse ${meta.label}">
            <svg viewBox="0 0 10 10" width="10" height="10" aria-hidden="true"><path d="M1 3.5 5 7.5 9 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>
          </button>
          <button class="panel-action" data-act="maximize" title="Maximise panel" aria-label="Maximise ${meta.label}">
            <svg viewBox="0 0 10 10" width="10" height="10" aria-hidden="true"><rect x="1.2" y="1.2" width="7.6" height="7.6" rx="1.4" fill="none" stroke="currentColor" stroke-width="1.4"/></svg>
          </button>
          <button class="panel-action close" data-act="close" title="Hide panel" aria-label="Hide ${meta.label}">
            <svg viewBox="0 0 10 10" width="10" height="10" aria-hidden="true"><path d="M1.6 1.6 8.4 8.4M8.4 1.6 1.6 8.4" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/></svg>
          </button>
        </div>
      </header>
      <div class="panel-body"></div>
      ${RESIZE_HANDLES.map((h) => `<span class="rz rz-${h}" data-rz="${h}" aria-hidden="true"></span>`).join('')}
    `;
    this.canvasEl.appendChild(el);

    const panel = {
      id, meta, el,
      headerEl: el.querySelector('.panel-head'),
      chromeEl: el.querySelector('.panel-chrome'),
      bodyEl: el.querySelector('.panel-body'),
      handles: Object.fromEntries(RESIZE_HANDLES.map((h) => [h, el.querySelector(`[data-rz="${h}"]`)])),
      state: null,
      lastDown: 0,
    };
    this.panels.set(id, panel);

    mount(panel.bodyEl, panel.chromeEl);
    el.querySelector('[data-act="collapse"]').addEventListener('click', () => this.toggleCollapsed(id));
    el.querySelector('[data-act="maximize"]').addEventListener('click', () => this.toggleMaximized(id));
    el.querySelector('[data-act="close"]').addEventListener('click', () => this.togglePanel(id));
    this._wireGesture(panel);
  }

  // Call once every panel is registered.
  start() {
    this._onResize();
  }

  _onResize() {
    const w = Math.max(320, Math.round(this.canvasEl.clientWidth));
    const h = Math.max(240, Math.round(this.canvasEl.clientHeight));
    if (this._hydrated && w === this.canvasSize.w && h === this.canvasSize.h) return;
    this.canvasSize = { w, h };
    if (!this._hydrated) {
      this._hydrated = true;
      const stored = loadStored();
      this.layout = stored ? clampLayoutToCanvas(stored, w, h) : buildDefaultLayout(w, h);
    } else {
      this.layout = clampLayoutToCanvas(this.layout, w, h);
    }
    this._renderAll();
  }

  _renderAll() {
    for (const [id, panel] of this.panels) this._applyState(id, panel);
    this._renderDock();
  }

  _applyState(id, panel) {
    const s = this.layout.panels[id];
    panel.state = s;
    panel.el.style.display = s.visible ? '' : 'none';
    if (!s.visible) return;
    panel.el.style.left = `${s.rect.x}px`;
    panel.el.style.top = `${s.rect.y}px`;
    panel.el.style.width = `${s.rect.w}px`;
    panel.el.style.height = `${s.collapsed ? HEAD_H : s.rect.h}px`;
    panel.el.style.zIndex = String(s.z ?? 0);
    panel.el.classList.toggle('is-collapsed', !!s.collapsed);
    panel.el.querySelector('[data-act="collapse"]').setAttribute('aria-expanded', String(!s.collapsed));
    panel.el.querySelector('[data-act="maximize"]').title = s.restore ? 'Restore panel' : 'Maximise panel';
  }

  _renderDock() {
    this.dockEl.innerHTML = '';
    for (const m of PANEL_META) {
      const on = this.layout.panels[m.id]?.visible;
      const b = document.createElement('button');
      b.className = `tb-btn${on ? ' on' : ''}`;
      b.title = `${on ? 'Hide' : 'Show'} ${m.label}`;
      b.setAttribute('aria-pressed', String(!!on));
      b.innerHTML = `<span class="ic" aria-hidden="true">${m.icon}</span><span>${m.label}</span>`;
      b.addEventListener('click', () => this.togglePanel(m.id));
      this.dockEl.appendChild(b);
    }
    const sep = document.createElement('div');
    sep.className = 'tb-sep';
    this.dockEl.appendChild(sep);

    const reset = document.createElement('button');
    reset.className = 'tb-btn';
    reset.title = 'Restore the default panel layout';
    reset.innerHTML = '<span class="ic" aria-hidden="true">↺</span><span>Reset layout</span>';
    reset.addEventListener('click', () => this.resetLayout());
    this.dockEl.appendChild(reset);

    const spacer = document.createElement('div');
    spacer.className = 'tb-spacer';
    this.dockEl.appendChild(spacer);

    const help = document.createElement('span');
    help.className = 'tb-help';
    help.tabIndex = 0;
    help.setAttribute('role', 'note');
    help.title = 'Drag a header to move\nDrag any edge or corner to resize\nDouble-click a header to maximise\nHold Alt to ignore the magnets\nArrow keys nudge a focused panel';
    help.textContent = '?';
    this.dockEl.appendChild(help);
  }

  othersFor(selfId) {
    const out = [];
    for (const [id, p] of this.panels) {
      if (id === selfId || !p.state?.visible) continue;
      out.push({ ...p.state.rect, h: p.state.collapsed ? HEAD_H : p.state.rect.h });
    }
    return out;
  }

  setGuides(gx, gy) {
    this.guideV.style.opacity = gx == null ? '0' : '1';
    if (gx != null) this.guideV.style.transform = `translateX(${gx}px)`;
    this.guideH.style.opacity = gy == null ? '0' : '1';
    if (gy != null) this.guideH.style.transform = `translateY(${gy}px)`;
  }

  raise(id) {
    let top = 0;
    for (const p of this.layout.panels ? Object.values(this.layout.panels) : []) top = Math.max(top, p.z ?? 0);
    const cur = this.layout.panels[id];
    if ((cur.z ?? 0) === top) return;
    cur.z = top + 1;
    this._applyState(id, this.panels.get(id));
    this._persist();
  }

  commitRect(id, rect) {
    const p = this.layout.panels[id];
    p.rect = rect;
    p.restore = undefined;
    this._applyState(id, this.panels.get(id));
    this._persist();
  }

  toggleCollapsed(id) {
    const p = this.layout.panels[id];
    p.collapsed = !p.collapsed;
    this._applyState(id, this.panels.get(id));
    this._persist();
  }

  toggleMaximized(id) {
    const p = this.layout.panels[id];
    if (p.restore) {
      p.rect = p.restore;
      p.restore = undefined;
    } else {
      p.restore = { ...p.rect };
      p.rect = { x: 0, y: 0, w: this.canvasSize.w, h: this.canvasSize.h };
      p.collapsed = false;
    }
    this._applyState(id, this.panels.get(id));
    this._persist();
  }

  togglePanel(id) {
    const p = this.layout.panels[id];
    p.visible = !p.visible;
    this._applyState(id, this.panels.get(id));
    this._renderDock();
    this._persist();
  }

  resetLayout() {
    this.layout = buildDefaultLayout(this.canvasSize.w, this.canvasSize.h);
    this._renderAll();
    this._persist();
  }

  setSnap(on) {
    this.snapOn = on;
  }

  setLocked(on) {
    this.locked = on;
    this.canvasEl.classList.toggle('is-locked', on);
  }

  _persist() {
    clearTimeout(this._saveTimer);
    this._saveTimer = setTimeout(() => {
      try { localStorage.setItem(STORAGE_KEY, JSON.stringify(this.layout)); } catch { /* quota / private mode */ }
    }, 200);
  }

  _wireGesture(panel) {
    const portal = this;
    let gesture = null;
    let live = null;

    const write = (r) => {
      panel.el.style.left = `${r.x}px`;
      panel.el.style.top = `${r.y}px`;
      panel.el.style.width = `${r.w}px`;
      if (!panel.state.collapsed) panel.el.style.height = `${r.h}px`;
    };

    const begin = (h) => (e) => {
      if (e.button !== 0) return;
      portal.raise(panel.id);
      if (portal.locked) return;
      // Never start a drag from something the user meant to click.
      if (h === 'move' && e.target.closest('button, input, select, a, [data-nodrag]')) return;
      if (h === 'move') {
        const gap = e.timeStamp - panel.lastDown;
        panel.lastDown = e.timeStamp;
        if (gap > 0 && gap < 350) {
          panel.lastDown = 0;
          portal.toggleMaximized(panel.id);
          return;
        }
      }
      e.preventDefault();
      e.stopPropagation();
      e.currentTarget.focus?.();
      e.currentTarget.setPointerCapture(e.pointerId);
      gesture = {
        h, sx: e.clientX, sy: e.clientY,
        base: { ...panel.state.rect },
        ctx: { W: portal.canvasSize.w, H: portal.canvasSize.h, others: portal.othersFor(panel.id), minW: panel.meta.minW, minH: panel.meta.minH },
        pointerId: e.pointerId,
        target: e.currentTarget,
      };
      panel.el.classList.add(h === 'move' ? 'is-dragging' : 'is-resizing');
      document.body.classList.add('portal-gesturing');
    };

    // Simple click anywhere in the panel also raises it — the header/handle
    // gesture below does the same, but stopPropagation there suppresses this
    // one once a real drag starts, so it only double-fires on plain clicks.
    panel.el.addEventListener('pointerdown', () => portal.raise(panel.id));

    panel.el.addEventListener('pointermove', (e) => {
      if (!gesture) return;
      const raw = applyHandle(gesture.base, gesture.h, e.clientX - gesture.sx, e.clientY - gesture.sy, gesture.ctx);
      const snapped = magnet(raw, gesture.h, gesture.ctx, portal.snapOn && !e.altKey);
      live = snapped.rect;
      write(live);
      portal.setGuides(snapped.gx, snapped.gy);
    });

    const end = () => {
      if (!gesture) return;
      try { gesture.target.releasePointerCapture(gesture.pointerId); } catch { /* already released */ }
      const r = live;
      gesture = null;
      live = null;
      panel.el.classList.remove('is-dragging', 'is-resizing');
      document.body.classList.remove('portal-gesturing');
      portal.setGuides(null, null);
      if (r) portal.commitRect(panel.id, r);
    };
    panel.el.addEventListener('pointerup', end);
    panel.el.addEventListener('pointercancel', end);

    panel.headerEl.addEventListener('pointerdown', begin('move'));
    for (const h of RESIZE_HANDLES) panel.handles[h].addEventListener('pointerdown', begin(h));

    // Keyboard nudge: the layout has to be usable without a pointer.
    panel.headerEl.addEventListener('keydown', (e) => {
      const step = e.shiftKey ? 20 : 2;
      const d = { ArrowLeft: [-step, 0], ArrowRight: [step, 0], ArrowUp: [0, -step], ArrowDown: [0, step] }[e.key];
      if (!d || portal.locked) return;
      e.preventDefault();
      const ctx = { W: portal.canvasSize.w, H: portal.canvasSize.h, others: portal.othersFor(panel.id), minW: panel.meta.minW, minH: panel.meta.minH };
      const hh = e.altKey ? 'se' : 'move';
      portal.commitRect(panel.id, applyHandle(panel.state.rect, hh, d[0], d[1], ctx));
    });
  }
}
