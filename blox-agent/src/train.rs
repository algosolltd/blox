//! Training environment and metric collection.

use crate::action::{Action, Side};
use crate::agent::Agent;
use crate::features::FeatureExtractor;

#[derive(Clone)]
pub struct Obs {
    pub prices: Vec<f64>,
    pub volumes: Vec<f64>,
    pub bid: f64,
    pub ask: f64,
    pub trade_price: f64,
    pub trade_volume: f64,
}

#[derive(Clone, Debug)]
pub struct Metrics {
    pub episodes: usize,
    pub total_pnl: f64,
    pub sharpe: f64,
    pub max_drawdown: f64,
    pub win_rate: f64,
    pub trade_count: usize,
}

pub struct BloxEnv {
    pub prices: Vec<f64>,
    pub volumes: Vec<f64>,
    pub capital: f64,
    pub cash: f64,
    pub position: f64,
    pub step: usize,
    pub max_steps: usize,
    pub cost: f64,
    pub trade_prices: Vec<f64>,
    pub trade_volumes: Vec<f64>,
}

impl BloxEnv {
    pub fn new(prices: Vec<f64>, volumes: Vec<f64>, initial_capital: f64) -> Self {
        let max_steps = prices.len().max(2);
        BloxEnv {
            prices,
            volumes,
            capital: initial_capital,
            cash: initial_capital,
            position: 0.0,
            step: 0,
            max_steps,
            cost: 0.0,
            trade_prices: Vec::new(),
            trade_volumes: Vec::new(),
        }
    }

    pub fn reset(&mut self) {
        self.step = 0;
        self.position = 0.0;
        self.cash = self.capital;
        self.cost = 0.0;
        self.trade_prices.clear();
        self.trade_volumes.clear();
    }

    pub fn observe(&self) -> Obs {
        let upto = (self.step + 1).min(self.prices.len()).max(1);
        let mut prices = self.prices[..upto].to_vec();
        if prices.is_empty() {
            prices.push(100.0);
        }
        let volumes: Vec<f64> = (0..upto)
            .map(|i| self.volumes.get(i % self.volumes.len().max(1)).copied().unwrap_or(0.0))
            .collect();
        let last = *prices.last().unwrap();
        Obs {
            prices,
            volumes,
            bid: last * 0.999,
            ask: last * 1.001,
            trade_price: last,
            trade_volume: 1.0,
        }
    }

    fn pnl_at(&self, price: f64) -> f64 {
        self.cash + self.position * price - self.cost
    }

    pub fn step(&mut self, action: &Action) -> f64 {
        if self.prices.is_empty() || self.step >= self.max_steps - 1 {
            return 0.0;
        }
        let idx = self.step.min(self.prices.len() - 1);
        let nxt = (self.step + 1).min(self.prices.len() - 1);
        let price = self.prices[idx];
        let next_price = self.prices[nxt];
        let before = self.pnl_at(price);

        match action.side {
            Side::Buy => {
                let qty = action.qty as f64;
                let cost = qty * price;
                if cost <= self.cash {
                    self.cash -= cost;
                    self.position += qty;
                    self.cost += cost * 0.0005;
                    self.trade_prices.push(price);
                    self.trade_volumes.push(qty);
                }
            }
            Side::Sell => {
                let qty = action.qty.min(self.position);
                if qty > 0.0 {
                    let proceeds = qty * price;
                    self.cash += proceeds;
                    self.position -= qty;
                    self.cost += proceeds * 0.0005;
                    self.trade_prices.push(price);
                    self.trade_volumes.push(qty);
                }
            }
            Side::Hold => {}
        }

        let after = self.pnl_at(next_price);
        self.step += 1;
        after - before
    }

    pub fn pnl(&self) -> f64 {
        let last = self.prices.last().copied().unwrap_or(100.0);
        self.cash + self.position * last - self.cost
    }
}

// ─── Training matrix ────────────────────────────────────────────────────────────

pub struct MatrixResult {
    pub agent: String,
    pub extractor: String,
    pub model: String,
    pub pnl: f64,
    pub sharpe: f64,
}

pub struct TrainingMatrix {
    pub results: Vec<MatrixResult>,
}

impl TrainingMatrix {
    pub fn new() -> Self {
        TrainingMatrix { results: Vec::new() }
    }

    pub fn run<F: FeatureExtractor + ?Sized>(
        &mut self,
        agent: &mut dyn Agent,
        extractor: &mut F,
        env: &mut BloxEnv,
        episodes: usize,
    ) {
        let mut rewards_all = Vec::new();
        for _ in 0..episodes {
            env.reset();
            let mut ep_rewards = Vec::new();
            for _ in 0..env.max_steps - 1 {
                let obs = env.observe();
                let state = extractor.extract(&obs);
                let action = agent.act(&state);
                ep_rewards.push(env.step(&action));
            }
            agent.train_episode(&ep_rewards);
            rewards_all.push(ep_rewards.iter().sum::<f64>());
        }
        let pnl = env.pnl();
        let mean = rewards_all.iter().sum::<f64>() / rewards_all.len().max(1) as f64;
        let std = (rewards_all
            .iter()
            .map(|r| (r - mean).powi(2))
            .sum::<f64>()
            / rewards_all.len().max(1) as f64)
            .sqrt()
            .max(1e-8);
        self.results.push(MatrixResult {
            agent: agent.name().to_string(),
            extractor: extractor.name().to_string(),
            model: String::new(),
            pnl: pnl - env.capital,
            sharpe: mean / std * (rewards_all.len() as f64).sqrt(),
        });
    }

    pub fn print(&self) {
        for r in &self.results {
            println!("{} / {} pnl={:.2} sharpe={:.2}", r.agent, r.extractor, r.pnl, r.sharpe);
        }
    }
}

pub fn grid_search(
    agents: &[(&str, fn(Box<dyn crate::model::Model>, usize, usize) -> Box<dyn Agent>)],
    models: &[(&str, fn(usize, usize) -> Box<dyn crate::model::Model>)],
    env: &mut BloxEnv,
    episodes: usize,
) -> TrainingMatrix {
    let _ = (agents, models, env, episodes);
    TrainingMatrix::new()
}
