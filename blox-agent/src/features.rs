//! Feature extractors that transform market observations into model inputs.

use crate::train::Obs;

#[derive(Clone, Debug)]
pub enum FeatureValue {
    Scalar(f64),
    Vector(Vec<f64>),
    Bid(f64, f64),
    Ask(f64, f64),
    Trade(f64, f64),
}

pub trait FeatureExtractor: Send + Sync {
    fn name(&self) -> &'static str;
    fn dim(&self) -> usize;
    fn extract(&mut self, obs: &Obs) -> Vec<f64>;
    fn reset(&mut self);
}

/// Ensure an extractor always returns exactly `dim` values, padding with zeros
/// or truncating overflow. Protects RNN models from size mismatches.
pub fn trim_to_dim(mut v: Vec<f64>, dim: usize) -> Vec<f64> {
    if v.len() > dim {
        v.truncate(dim);
    } else {
        while v.len() < dim {
            v.push(0.0);
        }
    }
    v
}

// ─── PriceVolume ────────────────────────────────────────────────────────────────

pub struct PriceVolume {
    pub last_price: f64,
    pub last_volume: f64,
}

impl PriceVolume {
    pub fn new() -> Self {
        PriceVolume {
            last_price: 0.0,
            last_volume: 0.0,
        }
    }
}

impl FeatureExtractor for PriceVolume {
    fn name(&self) -> &'static str {
        "PriceVolume"
    }

    fn dim(&self) -> usize {
        2
    }

    fn extract(&mut self, obs: &Obs) -> Vec<f64> {
        self.last_price = obs.prices.last().copied().unwrap_or(0.0);
        self.last_volume = obs.volumes.last().copied().unwrap_or(0.0);
        vec![self.last_price, self.last_volume]
    }

    fn reset(&mut self) {
        self.last_price = 0.0;
        self.last_volume = 0.0;
    }
}

// ─── Range ──────────────────────────────────────────────────────────────────────

pub struct Range {
    pub window: usize,
    pub prices: Vec<f64>,
}

impl Range {
    pub fn new() -> Self {
        Range {
            window: 14,
            prices: Vec::new(),
        }
    }
}

impl FeatureExtractor for Range {
    fn name(&self) -> &'static str {
        "Range"
    }

    fn dim(&self) -> usize {
        2
    }

    fn extract(&mut self, obs: &Obs) -> Vec<f64> {
        if let Some(&p) = obs.prices.last() {
            self.prices.push(p);
            if self.prices.len() > self.window {
                self.prices.remove(0);
            }
        }
        if self.prices.len() < 2 {
            return vec![0.0, 0.0];
        }
        let hi = self.prices.iter().cloned().fold(f64::MIN, f64::max);
        let lo = self.prices.iter().cloned().fold(f64::MAX, f64::min);
        let open = self.prices[0];
        let last = *self.prices.last().unwrap();
        vec![hi - lo, last - open]
    }

    fn reset(&mut self) {
        self.prices.clear();
    }
}

// ─── ATR ────────────────────────────────────────────────────────────────────────

pub struct ATR {
    pub period: usize,
    pub prev: Vec<f64>,
}

impl ATR {
    pub fn new(period: usize) -> Self {
        ATR {
            period,
            prev: Vec::new(),
        }
    }
}

impl FeatureExtractor for ATR {
    fn name(&self) -> &'static str {
        "ATR"
    }

    fn dim(&self) -> usize {
        1
    }

    fn extract(&mut self, obs: &Obs) -> Vec<f64> {
        if let Some(&p) = obs.prices.last() {
            self.prev.push(p);
            if self.prev.len() > self.period {
                self.prev.remove(0);
            }
        }
        if self.prev.len() < 2 {
            return vec![0.0];
        }
        let mut trs = Vec::new();
        for w in self.prev.windows(2) {
            trs.push((w[1] - w[0]).abs());
        }
        if trs.is_empty() {
            return vec![0.0];
        }
        let atr = trs.iter().sum::<f64>() / trs.len() as f64;
        vec![atr]
    }

    fn reset(&mut self) {
        self.prev.clear();
    }
}

// ─── SMA ────────────────────────────────────────────────────────────────────────

pub struct SMA {
    pub periods: Vec<usize>,
    pub prices: Vec<f64>,
}

impl SMA {
    pub fn new(periods: &[usize]) -> Self {
        SMA {
            periods: periods.to_vec(),
            prices: Vec::new(),
        }
    }
}

impl FeatureExtractor for SMA {
    fn name(&self) -> &'static str {
        "SMA"
    }

    fn dim(&self) -> usize {
        self.periods.len()
    }

    fn extract(&mut self, obs: &Obs) -> Vec<f64> {
        if let Some(&p) = obs.prices.last() {
            self.prices.push(p);
            if self.prices.len() > 200 {
                let drop = self.prices.len() - 200;
                self.prices.drain(0..drop);
            }
        }
        let mut out = Vec::new();
        for &per in &self.periods {
            if self.prices.len() >= per {
                let s: f64 = self.prices[self.prices.len() - per..].iter().sum();
                out.push(s / per as f64);
            } else {
                out.push(self.prices.last().copied().unwrap_or(0.0));
            }
        }
        out
    }

    fn reset(&mut self) {
        self.prices.clear();
    }
}

// ─── BookDepth ──────────────────────────────────────────────────────────────────

pub struct BookDepth {
    pub depth: usize,
}

impl BookDepth {
    pub fn new(depth: usize) -> Self {
        BookDepth { depth }
    }
}

impl FeatureExtractor for BookDepth {
    fn name(&self) -> &'static str {
        "BookDepth"
    }

    fn dim(&self) -> usize {
        2 * self.depth
    }

    fn extract(&mut self, obs: &Obs) -> Vec<f64> {
        let mut out = Vec::new();
        for _ in 0..self.depth {
            out.push(obs.bid);
        }
        for _ in 0..self.depth {
            out.push(obs.ask);
        }
        out
    }

    fn reset(&mut self) {}
}

// ─── HFQ (High-Frequency Quotes) ────────────────────────────────────────────────

pub struct HFQ {
    pub quotes: Vec<f64>,
}

impl HFQ {
    pub fn new() -> Self {
        HFQ { quotes: Vec::new() }
    }
}

impl FeatureExtractor for HFQ {
    fn name(&self) -> &'static str {
        "HFQ"
    }

    fn dim(&self) -> usize {
        4
    }

    fn extract(&mut self, obs: &Obs) -> Vec<f64> {
        if obs.trade_price > 0.0 {
            self.quotes.push(obs.trade_price);
            if self.quotes.len() > 50 {
                self.quotes.remove(0);
            }
        }
        let n = self.quotes.len();
        if n == 0 {
            return vec![0.0, 0.0, 0.0, 0.0];
        }
        let last = *self.quotes.last().unwrap();
        let mean: f64 = self.quotes.iter().sum::<f64>() / n as f64;
        let var: f64 = self.quotes.iter().map(|q| (q - mean).powi(2)).sum::<f64>() / n as f64;
        vec![last, mean, var, obs.trade_volume]
    }

    fn reset(&mut self) {
        self.quotes.clear();
    }
}

// ─── Combined ───────────────────────────────────────────────────────────────────

pub struct Combined {
    pub extractors: Vec<Box<dyn FeatureExtractor>>,
    pub dim_: usize,
}

impl Combined {
    pub fn new(extractors: Vec<Box<dyn FeatureExtractor>>) -> Self {
        let dim_ = extractors.iter().map(|e| e.dim()).sum();
        Combined { extractors, dim_ }
    }
}

impl FeatureExtractor for Combined {
    fn name(&self) -> &'static str {
        "Combined"
    }

    fn dim(&self) -> usize {
        self.dim_
    }

    fn extract(&mut self, obs: &Obs) -> Vec<f64> {
        let mut out = Vec::new();
        for e in self.extractors.iter_mut() {
            let part = e.extract(obs);
            out.extend(trim_to_dim(part, e.dim()));
        }
        trim_to_dim(out, self.dim_)
    }

    fn reset(&mut self) {
        for e in self.extractors.iter_mut() {
            e.reset();
        }
    }
}
