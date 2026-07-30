'use strict';
// Multi-timeframe candlestick + volume chart, backed by TradingView's
// lightweight-charts (load it as a global `LightweightCharts` script tag —
// it's a peer dependency, not bundled here). Builds every timeframe
// incrementally per trade so switching timeframe costs one setData call.
//
//   import { MarketChart } from './components/market-chart.js';
//   const chart = new MarketChart(containerEl, tfButtonsEl, ohlcEl, {
//     tfList: [1, 5, 15, 60, 300], priceDecimals: 2, priceDiv: 100,
//   });
//   chart.addTrade({ price, qty, ts });
//   chart.reset();

const CANDLES_MAX = 4000;

export class MarketChart {
  constructor(container, tfButtonsEl, ohlcEl, { tfList = [1, 5, 15, 60, 300], priceDecimals = 2, priceDiv = 100 } = {}) {
    this.tfList = tfList;
    this.priceDecimals = priceDecimals;
    this.priceDiv = priceDiv;
    this.ohlcEl = ohlcEl;
    this.tf = tfList[0];
    this.candles = new Map(tfList.map(tf => [tf, new Map()]));
    this.dirty = false;

    this.chart = LightweightCharts.createChart(container, {
      autoSize: true,
      layout: {
        background: { type: 'solid', color: 'transparent' },
        textColor: '#87809f', fontSize: 11, fontFamily: 'ui-monospace, Menlo, Consolas, monospace',
      },
      grid: {
        vertLines: { color: 'rgba(120,110,160,0.08)' },
        horzLines: { color: 'rgba(120,110,160,0.08)' },
      },
      crosshair: {
        vertLine: { color: '#758696', style: 3, labelBackgroundColor: '#7b61ff' },
        horzLine: { color: '#758696', style: 3, labelBackgroundColor: '#7b61ff' },
      },
      rightPriceScale: { borderColor: 'rgba(120,110,160,0.25)' },
      timeScale: {
        borderColor: 'rgba(120,110,160,0.25)',
        timeVisible: true, secondsVisible: true,
        rightOffset: 3, shiftVisibleRangeOnNewBar: true, barSpacing: 9,
      },
      localization: { priceFormatter: p => p.toFixed(this.priceDecimals) },
    });

    this.candleSeries = this.chart.addCandlestickSeries({
      upColor: '#2ebd85', downColor: '#f6465d', borderVisible: false,
      wickUpColor: '#2ebd85', wickDownColor: '#f6465d',
      priceFormat: { type: 'price', precision: this.priceDecimals, minMove: 1 / this.priceDiv },
      scaleMargins: { top: 0.08, bottom: 0.18 },
    });
    this.volSeries = this.chart.addHistogramSeries({
      priceFormat: { type: 'volume' }, priceScaleId: '',
      lastValueVisible: false, priceLineVisible: false,
    });
    this.chart.priceScale('').applyOptions({ scaleMargins: { top: 0.86, bottom: 0 } });

    this.chart.subscribeCrosshairMove(p => {
      const bar = p.seriesData && p.seriesData.get(this.candleSeries);
      if (bar) this._setOhlc(bar);
      else this._lastBarOhlc();
    });

    this.tfButtonsEl = tfButtonsEl;
    if (tfButtonsEl) {
      tfButtonsEl.querySelectorAll('[data-tf]').forEach(b => {
        b.addEventListener('click', () => this.setTF(+b.dataset.tf));
      });
    }

    this._raf = requestAnimationFrame(() => this._frame());
  }

  destroy() {
    cancelAnimationFrame(this._raf);
    this.chart.remove();
  }

  reset() {
    for (const m of this.candles.values()) m.clear();
    this.candleSeries.setData([]);
    this.volSeries.setData([]);
    this._setOhlc(null);
  }

  addTrade(t) {
    for (const tf of this.tfList) this._addCandle(tf, t);
  }

  setTF(tf) {
    this.tf = tf;
    if (this.tfButtonsEl) {
      this.tfButtonsEl.querySelectorAll('[data-tf]').forEach(b => b.classList.toggle('on', +b.dataset.tf === tf));
    }
    const arr = [...this.candles.get(tf).values()].sort((a, b) => a.time - b.time);
    this.candleSeries.setData(arr.map(c => this._toBar(c)));
    this.volSeries.setData(arr.map(c => this._toVol(c)));
    this.chart.timeScale().scrollToRealTime();
    this._lastBarOhlc();
  }

  _toBar(c) {
    return {
      time: c.time,
      open: c.open / this.priceDiv, high: c.high / this.priceDiv,
      low: c.low / this.priceDiv, close: c.close / this.priceDiv,
    };
  }

  _toVol(c) {
    return {
      time: c.time, value: c.vol,
      color: c.close >= c.open ? 'rgba(46,189,133,0.45)' : 'rgba(246,70,93,0.45)',
    };
  }

  _addCandle(tf, t) {
    const key = Math.floor(t.ts / 1000 / tf) * tf;
    const m = this.candles.get(tf);
    let c = m.get(key);
    if (!c) {
      c = { time: key, open: t.price, high: t.price, low: t.price, close: t.price, vol: 0 };
      m.set(key, c);
      if (m.size > CANDLES_MAX) m.delete(m.keys().next().value);
    }
    c.high = Math.max(c.high, t.price);
    c.low = Math.min(c.low, t.price);
    c.close = t.price;
    c.vol += t.qty;
    // Pushing to the chart per trade would mean thousands of series.update
    // calls per second on a busy market — mark and flush once per frame.
    if (tf === this.tf) this.dirty = true;
  }

  _lastBarOhlc() {
    const m = this.candles.get(this.tf);
    if (!m.size) return this._setOhlc(null);
    this._setOhlc(this._toBar(m.get([...m.keys()].pop())));
  }

  _setOhlc(bar) {
    if (!this.ohlcEl) return;
    if (!bar) { this.ohlcEl.textContent = ''; return; }
    const cls = bar.close >= bar.open ? 'up' : 'down';
    this.ohlcEl.innerHTML =
      `O <b class="${cls}">${bar.open.toFixed(this.priceDecimals)}</b> ` +
      `H <b class="${cls}">${bar.high.toFixed(this.priceDecimals)}</b> ` +
      `L <b class="${cls}">${bar.low.toFixed(this.priceDecimals)}</b> ` +
      `C <b class="${cls}">${bar.close.toFixed(this.priceDecimals)}</b>`;
  }

  _frame() {
    if (this.dirty) {
      const m = this.candles.get(this.tf);
      if (m.size) {
        const c = m.get([...m.keys()].pop());
        this.candleSeries.update(this._toBar(c));
        this.volSeries.update(this._toVol(c));
        this._lastBarOhlc();
      }
      this.dirty = false;
    }
    this._raf = requestAnimationFrame(() => this._frame());
  }
}
