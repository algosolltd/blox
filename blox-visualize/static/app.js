'use strict';
// blox-visualize — front end. Page-specific glue only: it mounts panels from
// the blox-visualize library into the page's shells, keeps the topbar and
// status bar in sync, and owns the resizable layout. Everything that draws a
// market lives in /lib and is published as an npm package — this file is the
// demo host, not a component.

import {
  FeedAdapter, createPriceFormat, fmtQty,
  MarketChart, OrderBook, Depth, Trades, OrderEntry, Orders, PnlPanel,
} from './lib/index.js';
import { Toaster } from './components/toaster.js';
import { Portal } from './components/portal.js';

(() => {

const $ = id => document.getElementById(id);

let CFG = { instrument: 'BLOX/USD', instrumentId: 1, priceScale: 2, server: '' };
// ponytail: priceScale is read from hello once and baked into every panel's
// formatter. An instrument's scale is fixed server-side, so this holds; if it
// ever changes mid-session, rebuild `fmt` and remount the panels.
let fmt = createPriceFormat(CFG.priceScale);

const S = {
  book: { bids: [], asks: [] },
  last: null, prev: null, first: null, high: null, low: null, vol: 0,
  stats: null, statsPrev: null, evs: 0,
  connected: false, engineUp: false,
};

const toaster = new Toaster($('toasts'));
const feed = new FeedAdapter(`${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/ws`);

// ---------------------------------------------------------------- panels
//
// Each panel is mounted into a portal-managed body/chrome pair instead of a
// fixed div from index.html — that's what makes it a freely draggable and
// resizable window rather than a cell in a static grid. The panel classes
// themselves don't know the difference: they still just get a host element
// and an optional status slot.

const portal = new Portal({
  canvas: $('canvas'),
  guideV: $('guide-v'),
  guideH: $('guide-h'),
  dock: $('dock'),
});

let chart, book, depth, trades, entry, orders, pnl;

portal.register('chart', (body, chrome) => {
  chart = new MarketChart(body, { fmt, statusEl: chrome });
});
portal.register('trades', (body, chrome) => {
  trades = new Trades(body, { fmt, statusEl: chrome });
});
portal.register('depth', (body, chrome) => {
  depth = new Depth(body, { fmt, statusEl: chrome });
});
portal.register('book', (body, chrome) => {
  book = new OrderBook(body, { fmt, statusEl: chrome });
});
portal.register('entry', (body) => {
  entry = new OrderEntry(body, {
    fmt,
    symbol: CFG.instrument.split('/')[0],
    getBook: () => S.book,
    onSubmit: o => feed.send({ type: 'order', ...o }),
  });
});
portal.register('orders', (body, chrome) => {
  orders = new Orders(body, {
    fmt,
    statusEl: chrome,
    onCancel: id => feed.send({ type: 'cancel', id }),
    onReduce: (id, qty) => feed.send({ type: 'reduce', id, qty }),
  });
});
portal.register('pnl', (body, chrome) => {
  pnl = new PnlPanel(body, { fmt, statusEl: chrome });
});

portal.start();

// ---------------------------------------------------------------- feed

feed.subscribe({
  onHello(h) {
    CFG = h;
    $('inst-name').textContent = h.instrument;
    document.title = `${h.instrument} · blox-visualize`;
    scheduleFooter();
  },

  // The backfill. Panels get it in bulk so the chart has bars on the longer
  // timeframes before a single live tick arrives.
  onHistory(list) {
    for (const t of list) trackTrade(t);
    chart.seed(list);
    trades.seed(list);
    scheduleHeader();
  },

  onTrade(t) {
    trackTrade(t);
    chart.addTrade(t);
    trades.push(t);
    scheduleHeader();
  },

  onBook(b) {
    S.book = b;
    book.update(b);
    depth.update(b);
    entry.update(b);
    scheduleHeader();
  },

  // `full` is the whole account, so the lists are replaced wholesale — empty
  // ones included. Testing `a.open` instead would strand the last order's
  // dashed line on the chart: omitempty means "no orders" arrives as nothing
  // at all, not as an empty array.
  onAccount(a) {
    if (a.full) {
      const open = a.open || [], closed = a.closed || [];
      orders.update({ open, closed });
      chart.setOrders(open);
      chart.setFills(closed
        .filter(o => o.filled > 0)
        .map(o => ({ ts: o.tsEnd || o.ts, price: o.avgFill, side: o.side })));
    }
    if (a.pnl) pnl.update(a.pnl);
  },

  onStats(s) {
    S.statsPrev = S.stats;
    S.stats = s;
    if (S.statsPrev) {
      const dt = (s.ts - S.statsPrev.ts) / 1000;
      if (dt > 0) S.evs = (s.applied - S.statsPrev.applied) / dt;
    }
    scheduleHeader();
    scheduleFooter();
  },

  onEngineStatus(s) {
    S.engineUp = s.state === 'up';
    scheduleFooter();
  },

  onNotice(n) { toaster.show(n.level, n.text); },

  // A reconnect means the bridge restarted: everything before it is fiction.
  onReset() {
    Object.assign(S, { last: null, prev: null, first: null, high: null, low: null, vol: 0 });
    scheduleHeader();
  },

  onConnection(state) {
    S.connected = state === 'live';
    $('conn').className = 'conn ' + (S.connected ? 'ok' : 'bad');
    $('conn-text').textContent = S.connected ? 'connected' : 'disconnected';
    scheduleFooter();
  },
});

function trackTrade(t) {
  S.prev = S.last;
  S.last = t.price;
  if (S.first == null) S.first = t.price;
  S.high = S.high == null ? t.price : Math.max(S.high, t.price);
  S.low = S.low == null ? t.price : Math.min(S.low, t.price);
  S.vol += t.qty;
}

// ---------------------------------------------------------------- header & footer
//
// Coalesced to one repaint per animation frame — book/trade/stats messages
// arrive far faster than the DOM needs to redraw, and a backfill replays
// thousands of trades in one go.

function batched(fn) {
  let scheduled = false;
  return () => {
    if (scheduled) return;
    scheduled = true;
    requestAnimationFrame(() => { scheduled = false; fn(); });
  };
}

function updateHeader() {
  if (S.last != null) {
    const last = $('h-last');
    last.textContent = fmt.fmtPx(S.last);
    const dir = S.prev == null ? 0 : Math.sign(S.last - S.prev);
    last.className = 'big mono ' + (dir > 0 ? 'up' : dir < 0 ? 'down' : '');
    if (S.first) {
      const d = S.last - S.first;
      const change = $('h-change');
      change.textContent = `${d >= 0 ? '+' : ''}${(d / S.first * 100).toFixed(2)}%`;
      change.className = 'mono ' + (d >= 0 ? 'up' : 'down');
    }
    $('h-high').textContent = fmt.fmtPx(S.high);
    $('h-low').textContent = fmt.fmtPx(S.low);
    $('h-vol').textContent = fmtQty(S.vol);
  }
  const bb = S.book.bids[0], ba = S.book.asks[0];
  if (bb && ba) {
    const sp = ba[0] - bb[0];
    const mid = (ba[0] + bb[0]) / 2;
    $('h-spread').textContent = `${fmt.fmtPx(sp)} (${(sp / mid * 100).toFixed(3)}%)`;
  }
  if (S.stats) $('h-engine').textContent = `${S.evs.toFixed(0)} ev/s · ${S.stats.conns} conn`;
}

function updateFooter() {
  const st = $('f-status');
  if (!S.connected) {
    st.textContent = 'webSocket disconnected — reconnecting…';
    st.className = 'err';
  } else if (!S.engineUp) {
    st.textContent = 'engine disconnected — retrying…';
    st.className = 'warn';
  } else {
    st.textContent = `live · ${CFG.instrument} · instrument ${CFG.instrumentId} · synthetic market`;
    st.className = 'ok';
  }
  if (S.stats) {
    $('f-engine').textContent =
      `engine ${CFG.server} · applied ${fmtQty(S.stats.applied)} · books ${S.stats.books} · errors ${S.stats.errors}`;
  }
}

const scheduleHeader = batched(updateHeader);
const scheduleFooter = batched(updateFooter);

// ---------------------------------------------------------------- portal toggles

$('snap-toggle').addEventListener('click', () => {
  const on = !portal.snapOn;
  portal.setSnap(on);
  $('snap-toggle').classList.toggle('on', on);
  $('snap-toggle').setAttribute('aria-pressed', String(on));
});

$('lock-toggle').addEventListener('click', () => {
  const on = !portal.locked;
  portal.setLocked(on);
  const btn = $('lock-toggle');
  btn.classList.toggle('on', on);
  btn.setAttribute('aria-pressed', String(on));
  btn.innerHTML = on ? '<span aria-hidden="true">🔒</span> Locked' : '<span aria-hidden="true">🔓</span> Unlocked';
});

// ---------------------------------------------------------------- boot

feed.connect();

})();
