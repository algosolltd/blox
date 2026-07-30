'use strict';
// blox-visualize — front end. One WebSocket, panels wired from
// static/components/*.js. This file is page-specific glue: DOM lookups,
// message routing, header/footer text, and the resizable layout — not
// reusable, unlike the components it imports.

import { fmtQty, fmtTime, fmtTimeShort, createPriceFormat } from './components/format.js';
import { DepthChart } from './components/depth-chart.js';
import { OrderBookPanel } from './components/order-book.js';
import { TradesPanel } from './components/trades-panel.js';
import { MarketChart } from './components/market-chart.js';
import { OrderEntryForm } from './components/order-entry.js';
import { AccountOrdersPanel } from './components/account-panel.js';
import { PnlPanel } from './components/pnl-panel.js';
import { Toaster } from './components/toaster.js';

(() => {

const $ = id => document.getElementById(id);
const el = {
  last: $('h-last'), change: $('h-change'), high: $('h-high'), low: $('h-low'),
  vol: $('h-vol'), spread: $('h-spread'), engine: $('h-engine'),
  conn: $('conn'), connText: $('conn-text'),
  instName: $('inst-name'),
  ohlc: $('ohlc'), trades: $('trades'), tradesRate: $('trades-rate'),
  chart: $('chart'), tfGroup: $('tf-group'),
  depth: $('depth'), depthTip: $('depth-tip'), depthSpread: $('depth-spread'),
  bookAsks: $('book-asks'), bookBids: $('book-bids'), obSpread: $('ob-spread'),
  bookDepth: $('book-depth'),
  fStatus: $('f-status'), fEngine: $('f-engine'),
  openRows: $('open-rows'), closedRows: $('closed-rows'), openCount: $('open-count'),
  pnlTotal: $('pnl-total'), pnlRealized: $('pnl-realized'), pnlUnreal: $('pnl-unreal'),
  pnlPos: $('pnl-pos'), pnlAvg: $('pnl-avg'), pnlMark: $('pnl-mark'),
  pnlVol: $('pnl-vol'), pnlFills: $('pnl-fills'), pnlSpark: $('pnl-spark'),
  oeBuy: $('oe-buy'), oeSell: $('oe-sell'), oeKind: $('oe-kind'),
  oePrice: $('oe-price'), oeQty: $('oe-qty'), oeNotional: $('oe-notional'),
  oeSubmit: $('oe-submit'),
  toasts: $('toasts'),
};

let CFG = { name: 'BLOX/USD', instrument: 1, priceScale: 2, server: '' };
// ponytail: priceScale is read once at boot and baked into every panel's
// formatters. Fine as long as an instrument's scale never changes mid-session
// (true today — it's fixed server-side). If that changes, rebuild `fmt` and
// the components that hold it on each 'hello' instead.
const fmt = createPriceFormat(CFG.priceScale);

const S = {
  book: { bids: [], asks: [] },
  last: null, prev: null, first: null, high: null, low: null, vol: 0,
  stats: null, statsPrev: null, evs: 0,
  connected: false, engineUp: false,
};

// ---------------------------------------------------------------- panels

const toaster = new Toaster(el.toasts);

const depthChart = new DepthChart(el.depth, el.depthTip, { fmtPx: fmt.fmtPx, fmtQty });

const orderBook = new OrderBookPanel(
  { asksEl: el.bookAsks, bidsEl: el.bookBids, spreadEl: el.obSpread, depthCountEl: el.bookDepth },
  { fmtPx: fmt.fmtPx, fmtQty },
);

const trades = new TradesPanel(el.trades, el.tradesRate, { fmtPx: fmt.fmtPx, fmtQty, fmtTime });

const chart = new MarketChart(el.chart, el.tfGroup, el.ohlc, { priceDiv: fmt.div, priceDecimals: fmt.dec });

const entry = new OrderEntryForm(
  {
    buyBtn: el.oeBuy, sellBtn: el.oeSell, kindSelect: el.oeKind,
    priceInput: el.oePrice, qtyInput: el.oeQty, notionalEl: el.oeNotional, submitBtn: el.oeSubmit,
    quickBtns: [...document.querySelectorAll('.oe-quick button')],
  },
  {
    fmtPx: fmt.fmtPx, parsePrice: fmt.parsePrice, fmtMoney: fmt.fmtMoney,
    symbol: CFG.name.split('/')[0],
    getBook: () => S.book,
    onSubmit: order => sendJson({ type: 'order', ...order }),
  },
);

const account = new AccountOrdersPanel(
  { openRowsEl: el.openRows, closedRowsEl: el.closedRows, openCountEl: el.openCount },
  {
    fmtPx: fmt.fmtPx, fmtQty, fmtTimeShort,
    onCancel: id => sendJson({ type: 'cancel', id }),
    onReduce: (id, qty) => sendJson({ type: 'reduce', id, qty }),
  },
);

const pnl = new PnlPanel(
  {
    totalEl: el.pnlTotal, realizedEl: el.pnlRealized, unrealEl: el.pnlUnreal,
    posEl: el.pnlPos, avgEl: el.pnlAvg, markEl: el.pnlMark, volEl: el.pnlVol, fillsEl: el.pnlFills,
  },
  el.pnlSpark,
  { fmtMoney: fmt.fmtMoney, fmtPx: fmt.fmtPx, fmtQty },
);

// ---------------------------------------------------------------- websocket

let sock = null;
let backoff = 500;
function connect() {
  const proto = location.protocol === 'https:' ? 'wss' : 'ws';
  const ws = new WebSocket(`${proto}://${location.host}/ws`);
  sock = ws;
  ws.onopen = () => { backoff = 500; setConn(true); };
  ws.onclose = () => {
    sock = null;
    setConn(false);
    setTimeout(connect, backoff);
    backoff = Math.min(backoff * 2, 5000);
  };
  ws.onerror = () => {};
  ws.onmessage = e => route(JSON.parse(e.data));
}

function sendJson(o) {
  if (sock && sock.readyState === WebSocket.OPEN) sock.send(JSON.stringify(o));
}

function route(m) {
  switch (m.type) {
    case 'hello': onHello(m); break;
    case 'update': onUpdate(m); break;
    case 'status': onStatus(m); break;
    case 'account': onAccount(m); break;
    case 'notice': toaster.show(m.level, m.text); break;
  }
}

function onHello(m) {
  CFG = m;
  el.instName.textContent = m.name;
  document.title = `${m.name} · blox-visualize`;
  // A fresh hello means a fresh bridge: state from before is fiction.
  chart.reset();
  trades.clear();
  Object.assign(S, { last: null, prev: null, first: null, high: null, low: null, vol: 0 });
  if (m.book || m.stats) onUpdate({ book: m.book, stats: m.stats });
}

function onUpdate(m) {
  if (m.book) {
    S.book = m.book;
    orderBook.update(m.book);
    depthChart.render(m.book);
    el.depthSpread.textContent = depthChart.spreadText();
    entry.maybePrefillFromBook(m.book);
    scheduleHeader();
  }
  if (m.trades) {
    for (const t of m.trades) onTrade(t);
    scheduleHeader();
  }
  if (m.stats) {
    S.statsPrev = S.stats;
    S.stats = m.stats;
    if (S.statsPrev) {
      const dt = (m.stats.ts - S.statsPrev.ts) / 1000;
      if (dt > 0) S.evs = (m.stats.applied - S.statsPrev.applied) / dt;
    }
    scheduleHeader();
    scheduleFooter();
  }
}

function onTrade(t) {
  S.prev = S.last;
  S.last = t.price;
  if (S.first == null) S.first = t.price;
  S.high = S.high == null ? t.price : Math.max(S.high, t.price);
  S.low = S.low == null ? t.price : Math.min(S.low, t.price);
  S.vol += t.qty;
  chart.addTrade(t);
  trades.push(t);
}

function onStatus(m) {
  S.engineUp = m.state === 'up';
  scheduleFooter();
}

function onAccount(m) {
  if (m.full) {
    account.setOpen(m.open || []);
    account.setClosed(m.closed || []);
  }
  if (m.pnl) pnl.update(m.pnl);
}

// ---------------------------------------------------------------- header & footer
//
// Coalesced to one repaint per animation frame — book/trade/stats messages
// can arrive far faster than the DOM needs to redraw.

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
    el.last.textContent = fmt.fmtPx(S.last);
    const dir = S.prev == null ? 0 : Math.sign(S.last - S.prev);
    el.last.className = 'big mono ' + (dir > 0 ? 'up' : dir < 0 ? 'down' : '');
    if (S.first) {
      const d = S.last - S.first;
      const pct = d / S.first * 100;
      el.change.textContent = `${d >= 0 ? '+' : ''}${pct.toFixed(2)}%`;
      el.change.className = 'mono ' + (d >= 0 ? 'up' : 'down');
    }
    el.high.textContent = fmt.fmtPx(S.high);
    el.low.textContent = fmt.fmtPx(S.low);
    el.vol.textContent = fmtQty(S.vol);
  }
  const bb = S.book.bids[0], ba = S.book.asks[0];
  if (bb && ba) {
    const sp = ba[0] - bb[0];
    const mid = (ba[0] + bb[0]) / 2;
    el.spread.textContent = `${fmt.fmtPx(sp)} (${(sp / mid * 100).toFixed(3)}%)`;
  }
  if (S.stats) {
    el.engine.textContent = `${S.evs.toFixed(0)} ev/s · ${S.stats.conns} conn`;
  }
}

function updateFooter() {
  const st = el.fStatus;
  if (!S.connected) {
    st.textContent = 'webSocket disconnected — reconnecting…';
    st.className = 'err';
  } else if (!S.engineUp) {
    st.textContent = 'engine disconnected — retrying…';
    st.className = 'warn';
  } else {
    st.textContent = `live · ${CFG.name} · instrument ${CFG.instrument} · synthetic market`;
    st.className = 'ok';
  }
  if (S.stats) {
    el.fEngine.textContent =
      `engine ${CFG.server} · applied ${fmtQty(S.stats.applied)} · books ${S.stats.books} · errors ${S.stats.errors}`;
  }
}

const scheduleHeader = batched(updateHeader);
const scheduleFooter = batched(updateFooter);

function setConn(ok) {
  S.connected = ok;
  el.conn.className = 'conn ' + (ok ? 'ok' : 'bad');
  el.connText.textContent = ok ? 'connected' : 'disconnected';
  scheduleFooter();
}

// ---------------------------------------------------------------- layout splitters

function initHsplit() {
  const split = $('hsplit');
  const bottom = $('bottom');
  split.addEventListener('mousedown', e => {
    const startY = e.clientY, startH = bottom.offsetHeight;
    split.classList.add('drag');
    const move = ev => {
      const hpx = Math.max(140, Math.min(window.innerHeight - 260, startH - (ev.clientY - startY)));
      bottom.style.flex = `0 0 ${hpx}px`;
    };
    const up = () => {
      split.classList.remove('drag');
      window.removeEventListener('mousemove', move);
      window.removeEventListener('mouseup', up);
    };
    window.addEventListener('mousemove', move);
    window.addEventListener('mouseup', up);
    e.preventDefault();
  });
}

const LAYOUT_KEY = 'blox-viz-layout-v1';

function initSplitters() {
  const saved = JSON.parse(localStorage.getItem(LAYOUT_KEY) || 'null');
  if (saved) {
    const panels = [...document.querySelectorAll('#grid .panel')];
    panels.forEach((p, i) => { if (saved[i]) p.style.flex = `0 0 ${saved[i]}px`; });
  }
  document.querySelectorAll('.split').forEach(split => {
    split.addEventListener('mousedown', e => {
      const prev = split.previousElementSibling, next = split.nextElementSibling;
      const startX = e.clientX, pw = prev.offsetWidth, nw = next.offsetWidth;
      split.classList.add('drag');
      const move = ev => {
        const dx = ev.clientX - startX;
        const a = Math.max(170, pw + dx), b = Math.max(170, nw - dx);
        prev.style.flex = `0 0 ${a}px`;
        next.style.flex = `0 0 ${b}px`;
      };
      const up = () => {
        split.classList.remove('drag');
        window.removeEventListener('mousemove', move);
        window.removeEventListener('mouseup', up);
        const widths = [...document.querySelectorAll('#grid .panel')].map(p => p.offsetWidth);
        localStorage.setItem(LAYOUT_KEY, JSON.stringify(widths));
      };
      window.addEventListener('mousemove', move);
      window.addEventListener('mouseup', up);
      e.preventDefault();
    });
  });
}

// ---------------------------------------------------------------- boot

initSplitters();
initHsplit();
connect();

})();
