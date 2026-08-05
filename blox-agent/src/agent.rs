//! All trading agents: RL, evolutionary, and rule-based.

use crate::action::{action_from_logits, Action};
use crate::model::Model;

pub trait Agent: Send + Sync {
    fn act(&mut self, state: &[f64]) -> Action;
    fn train_episode(&mut self, rewards: &[f64]);
    fn save(&self, path: &str);
    fn load(&mut self, path: &str);
    fn name(&self) -> &'static str;
    fn model(&self) -> &dyn Model;
    fn model_mut(&mut self) -> &mut dyn Model;
    fn reset(&mut self);
}

// â”€â”€â”€ helpers â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

fn softmax_index(logits: &[f64]) -> usize {
    if logits.is_empty() {
        return 0;
    }
    let max = logits.iter().cloned().fold(f64::MIN, f64::max);
    let exps: Vec<f64> = logits.iter().map(|&x| (x - max).exp()).collect();
    let sum: f64 = exps.iter().sum();
    let mut r = rand::random::<f64>() * sum;
    for (i, e) in exps.iter().enumerate() {
        if r < *e {
            return i;
        }
        r -= *e;
    }
    logits.len() - 1
}

fn action_from_index(idx: usize, n: usize) -> Action {
    const BUY_BUCKETS: usize = 6;
    const SELL_BUCKETS: usize = 6;
    if idx < BUY_BUCKETS.min(n) {
        Action::market_buy(idx as u64 + 1)
    } else if idx < (BUY_BUCKETS + SELL_BUCKETS).min(n) {
        Action::market_sell(idx as u64 - BUY_BUCKETS as u64 + 1)
    } else {
        Action::hold()
    }
}

fn blended(input: &[f64], history: &mut Vec<f64>) -> Vec<f64> {
    if !input.is_empty() {
        history.push(input[0]);
    }
    if history.len() > 8 {
        history.remove(0);
    }
    let mean: f64 = if history.is_empty() {
        input[0]
    } else {
        history.iter().sum::<f64>() / history.len() as f64
    };
    let mut out = input.to_vec();
    if !out.is_empty() {
        out[0] = 0.5 * out[0] + 0.5 * mean;
    }
    out
}

pub struct NoOpModel;

impl Model for NoOpModel {
    fn forward(&mut self, input: &[f64]) -> Vec<f64> {
        vec![0.0; input.len()]
    }
    fn backward(&mut self, _input: &[f64], grads: &[f64]) -> Vec<f64> {
        grads.to_vec()
    }
    fn update(&mut self, _learning_rate: f64) {}
    fn reset(&mut self) {}
    fn name(&self) -> &'static str {
        "NoOp"
    }
    fn parameters(&self) -> Vec<f64> {
        Vec::new()
    }
    fn set_parameters(&mut self, _params: &[f64]) {}
    fn parameter_len(&self) -> usize {
        0
    }
}

// â”€â”€â”€ Evolution Strategy â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

pub struct EvolutionStrategy {
    pub model: Box<dyn Model>,
    pub population_size: usize,
    pub sigma: f64,
    pub learning_rate: f64,
    pub mean: Vec<f64>,
    pub noise: Vec<Vec<f64>>,
    pub population: Vec<Vec<f64>>,
    pub fitness: Vec<f64>,
    pub episode: usize,
}

impl EvolutionStrategy {
    pub fn new(mut model: Box<dyn Model>, population_size: usize, sigma: f64, learning_rate: f64) -> Self {
        let mean = model.parameters();
        let n = mean.len();
        let mut noise = Vec::new();
        let mut population = Vec::new();
        for _ in 0..population_size {
            let eps: Vec<f64> = (0..n).map(|_| rand::random::<f64>() * 2.0 - 1.0).collect();
            let p: Vec<f64> = mean.iter().zip(&eps).map(|(m, e)| m + sigma * e).collect();
            noise.push(eps);
            population.push(p);
        }
        if let Some(p0) = population.first() {
            model.set_parameters(p0);
        }
        EvolutionStrategy {
            model,
            population_size,
            sigma,
            learning_rate,
            mean,
            noise,
            population,
            fitness: vec![0.0; population_size],
            episode: 0,
        }
    }
}

impl Agent for EvolutionStrategy {
    fn act(&mut self, state: &[f64]) -> Action {
        let logits = self.model.forward(state);
        action_from_logits(&logits)
    }

    fn train_episode(&mut self, rewards: &[f64]) {
        let r: f64 = rewards.iter().sum();
        let idx = self.episode % self.population_size;
        self.fitness[idx] += r;
        self.episode += 1;
        if self.episode % self.population_size == 0 {
            let n = self.mean.len();
            let mut grad = vec![0.0; n];
            for i in 0..self.population_size {
                for j in 0..n {
                    grad[j] += self.fitness[i] * self.noise[i][j];
                }
            }
            let norm = (self.sigma * self.population_size as f64).max(1e-8);
            for j in 0..n {
                self.mean[j] += self.learning_rate * grad[j] / norm;
            }
            self.noise.clear();
            self.population.clear();
            for _ in 0..self.population_size {
                let eps: Vec<f64> = (0..n).map(|_| rand::random::<f64>() * 2.0 - 1.0).collect();
                let p: Vec<f64> = self.mean.iter().zip(&eps).map(|(m, e)| m + self.sigma * e).collect();
                self.noise.push(eps);
                self.population.push(p);
            }
            self.fitness.fill(0.0);
        }
        if let Some(p) = self.population.get(self.episode % self.population_size) {
            self.model.set_parameters(p);
        }
    }

    fn save(&self, path: &str) {
        let mut s = String::new();
        for v in &self.mean {
            s.push_str(&format!("{}\n", v));
        }
        let _ = std::fs::write(path, s);
    }

    fn load(&mut self, path: &str) {
        if let Ok(content) = std::fs::read_to_string(path) {
            let vals: Vec<f64> = content.lines().filter_map(|l| l.parse().ok()).collect();
            if vals.len() == self.mean.len() {
                self.mean = vals;
            }
        }
    }

    fn name(&self) -> &'static str {
        "ES"
    }

    fn model(&self) -> &dyn Model {
        &*self.model
    }

    fn model_mut(&mut self) -> &mut dyn Model {
        &mut *self.model
    }

    fn reset(&mut self) {
        self.model.reset();
        self.mean = self.model.parameters();
        self.episode = 0;
        self.fitness.fill(0.0);
    }
}

// â”€â”€â”€ Q-Learning family â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

pub struct QLearning {
    pub model: Box<dyn Model>,
    pub gamma: f64,
    pub state_size: usize,
    pub action_size: usize,
    pub epsilon: f64,
    pub learning_rate: f64,
    pub dueling: bool,
    pub double: bool,
    pub recurrent: bool,
    pub target_params: Vec<f64>,
    pub history: Vec<f64>,
    pub episode_count: usize,
    pub last_state: Vec<f64>,
    pub last_action: Action,
}

impl QLearning {
    pub fn new(model: Box<dyn Model>, state_size: usize, action_size: usize) -> Self {
        let target_params = model.parameters();
        QLearning {
            model,
            gamma: 0.99,
            state_size,
            action_size,
            epsilon: 0.2,
            learning_rate: 0.001,
            dueling: false,
            double: false,
            recurrent: false,
            target_params,
            history: Vec::new(),
            episode_count: 0,
            last_state: Vec::new(),
            last_action: Action::hold(),
        }
    }

    fn pick(&mut self, state: &[f64]) -> Action {
        let input = if self.recurrent {
            blended(state, &mut self.history)
        } else {
            state.to_vec()
        };
        let mut logits = self.model.forward(&input);
        if self.dueling && logits.len() > 1 {
            let mean = logits.iter().sum::<f64>() / logits.len() as f64;
            for l in logits.iter_mut() {
                *l -= mean;
            }
        }
        let eps = if self.double { self.epsilon * 0.5 } else { self.epsilon };
        if rand::random::<f64>() < eps {
            let r = rand::random::<f64>();
            if r < 0.5 {
                Action::market_buy((rand::random::<f64>() * 5.0) as u64 + 1)
            } else {
                Action::market_sell((rand::random::<f64>() * 5.0) as u64 + 1)
            }
        } else {
            action_from_logits(&logits)
        }
    }
}

impl Agent for QLearning {
    fn act(&mut self, state: &[f64]) -> Action {
        self.last_state = state.to_vec();
        self.last_action = self.pick(state);
        self.last_action.clone()
    }

    fn train_episode(&mut self, rewards: &[f64]) {
        let r: f64 = rewards.iter().sum();
        let scale = if r > 0.0 { 1.0 } else if r < 0.0 { -1.0 } else { 0.0 };
        self.model.update(self.learning_rate * scale);
        self.episode_count += 1;
        if self.episode_count % 20 == 0 {
            self.target_params = self.model.parameters();
        }
        self.epsilon = (self.epsilon * 0.995).max(0.05);
        self.history.clear();
    }

    fn save(&self, path: &str) {
        let p = self.model.parameters();
        let s: Vec<String> = p.iter().map(|v| v.to_string()).collect();
        let _ = std::fs::write(path, s.join("\n"));
    }

    fn load(&mut self, path: &str) {
        if let Ok(content) = std::fs::read_to_string(path) {
            let vals: Vec<f64> = content.lines().filter_map(|l| l.parse().ok()).collect();
            if vals.len() == self.model.parameter_len() {
                self.model.set_parameters(&vals);
            }
        }
    }

    fn name(&self) -> &'static str {
        "QLearning"
    }

    fn model(&self) -> &dyn Model {
        &*self.model
    }

    fn model_mut(&mut self) -> &mut dyn Model {
        &mut *self.model
    }

    fn reset(&mut self) {
        self.model.reset();
        self.episode_count = 0;
        self.history.clear();
    }
}

macro_rules! q_variant {
    ($name:ident, $cfg:expr) => {
        pub struct $name {
            pub base: QLearning,
        }
        impl $name {
            pub fn new(model: Box<dyn Model>, state_size: usize, action_size: usize) -> Self {
                let mut base = QLearning::new(model, state_size, action_size);
                $cfg(&mut base);
                $name { base }
            }
        }
        impl Agent for $name {
            fn act(&mut self, state: &[f64]) -> Action {
                self.base.act(state)
            }
            fn train_episode(&mut self, rewards: &[f64]) {
                self.base.train_episode(rewards)
            }
            fn save(&self, path: &str) {
                self.base.save(path)
            }
            fn load(&mut self, path: &str) {
                self.base.load(path)
            }
            fn name(&self) -> &'static str {
                stringify!($name)
            }
            fn model(&self) -> &dyn Model {
                self.base.model()
            }
            fn model_mut(&mut self) -> &mut dyn Model {
                self.base.model_mut()
            }
            fn reset(&mut self) {
                self.base.reset()
            }
        }
    };
}

q_variant!(DoubleQLearning, |b: &mut QLearning| b.double = true);
q_variant!(DuelingQLearning, |b: &mut QLearning| b.dueling = true);
q_variant!(RecurrentQLearning, |b: &mut QLearning| b.recurrent = true);
q_variant!(DoubleDuelingQLearning, |b: &mut QLearning| {
    b.double = true;
    b.dueling = true;
});
q_variant!(DoubleRecurrentQLearning, |b: &mut QLearning| {
    b.double = true;
    b.recurrent = true;
});
q_variant!(DuelingRecurrentQLearning, |b: &mut QLearning| {
    b.dueling = true;
    b.recurrent = true;
});
q_variant!(DoubleDuelingRecurrentQLearning, |b: &mut QLearning| {
    b.double = true;
    b.dueling = true;
    b.recurrent = true;
});

// â”€â”€â”€ Actor-Critic family â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

pub struct ActorCritic {
    pub actor: Box<dyn Model>,
    pub critic: Box<dyn Model>,
    pub gamma: f64,
    pub learning_rate: f64,
    pub recurrent: bool,
    pub duel: bool,
    pub history: Vec<f64>,
    pub last_action: Action,
}

impl ActorCritic {
    pub fn new(actor: Box<dyn Model>, critic: Box<dyn Model>, gamma: f64, learning_rate: f64) -> Self {
        ActorCritic {
            actor,
            critic,
            gamma,
            learning_rate,
            recurrent: false,
            duel: false,
            history: Vec::new(),
            last_action: Action::hold(),
        }
    }
}

impl Agent for ActorCritic {
    fn act(&mut self, state: &[f64]) -> Action {
        let input = if self.recurrent {
            blended(state, &mut self.history)
        } else {
            state.to_vec()
        };
        let mut logits = self.actor.forward(&input);
        if self.duel && logits.len() > 1 {
            let mean = logits.iter().sum::<f64>() / logits.len() as f64;
            for l in logits.iter_mut() {
                *l -= mean;
            }
        }
        let idx = softmax_index(&logits);
        self.last_action = action_from_index(idx, logits.len());
        self.last_action.clone()
    }

    fn train_episode(&mut self, rewards: &[f64]) {
        let r: f64 = rewards.iter().sum();
        let scale = if r > 0.0 { 1.0 } else if r < 0.0 { -1.0 } else { 0.0 };
        self.actor.update(self.learning_rate * scale);
        self.critic.update(self.learning_rate * scale * 0.5);
        self.history.clear();
    }

    fn save(&self, path: &str) {
        let p = self.actor.parameters();
        let s: Vec<String> = p.iter().map(|v| v.to_string()).collect();
        let _ = std::fs::write(path, s.join("\n"));
    }

    fn load(&mut self, path: &str) {
        if let Ok(content) = std::fs::read_to_string(path) {
            let vals: Vec<f64> = content.lines().filter_map(|l| l.parse().ok()).collect();
            if vals.len() == self.actor.parameter_len() {
                self.actor.set_parameters(&vals);
            }
        }
    }

    fn name(&self) -> &'static str {
        "ActorCritic"
    }

    fn model(&self) -> &dyn Model {
        &*self.actor
    }

    fn model_mut(&mut self) -> &mut dyn Model {
        &mut *self.actor
    }

    fn reset(&mut self) {
        self.actor.reset();
        self.critic.reset();
        self.history.clear();
    }
}

pub struct ActorCriticRecurrent {
    pub base: ActorCritic,
}

impl ActorCriticRecurrent {
    pub fn new(actor: Box<dyn Model>, critic: Box<dyn Model>, gamma: f64, lr: f64) -> Self {
        let mut base = ActorCritic::new(actor, critic, gamma, lr);
        base.recurrent = true;
        ActorCriticRecurrent { base }
    }
}

impl Agent for ActorCriticRecurrent {
    fn act(&mut self, state: &[f64]) -> Action {
        self.base.act(state)
    }
    fn train_episode(&mut self, rewards: &[f64]) {
        self.base.train_episode(rewards)
    }
    fn save(&self, path: &str) {
        self.base.save(path)
    }
    fn load(&mut self, path: &str) {
        self.base.load(path)
    }
    fn name(&self) -> &'static str {
        "ActorCriticRecurrent"
    }
    fn model(&self) -> &dyn Model {
        self.base.model()
    }
    fn model_mut(&mut self) -> &mut dyn Model {
        self.base.model_mut()
    }
    fn reset(&mut self) {
        self.base.reset()
    }
}

pub struct ActorCriticDuel {
    pub base: ActorCritic,
}

impl ActorCriticDuel {
    pub fn new(actor: Box<dyn Model>, critic: Box<dyn Model>, gamma: f64, lr: f64) -> Self {
        let mut base = ActorCritic::new(actor, critic, gamma, lr);
        base.duel = true;
        ActorCriticDuel { base }
    }
}

impl Agent for ActorCriticDuel {
    fn act(&mut self, state: &[f64]) -> Action {
        self.base.act(state)
    }
    fn train_episode(&mut self, rewards: &[f64]) {
        self.base.train_episode(rewards)
    }
    fn save(&self, path: &str) {
        self.base.save(path)
    }
    fn load(&mut self, path: &str) {
        self.base.load(path)
    }
    fn name(&self) -> &'static str {
        "ActorCriticDuel"
    }
    fn model(&self) -> &dyn Model {
        self.base.model()
    }
    fn model_mut(&mut self) -> &mut dyn Model {
        self.base.model_mut()
    }
    fn reset(&mut self) {
        self.base.reset()
    }
}

pub struct ActorCriticDuelRecurrent {
    pub base: ActorCritic,
}

impl ActorCriticDuelRecurrent {
    pub fn new(actor: Box<dyn Model>, critic: Box<dyn Model>, gamma: f64, lr: f64) -> Self {
        let mut base = ActorCritic::new(actor, critic, gamma, lr);
        base.duel = true;
        base.recurrent = true;
        ActorCriticDuelRecurrent { base }
    }
}

impl Agent for ActorCriticDuelRecurrent {
    fn act(&mut self, state: &[f64]) -> Action {
        self.base.act(state)
    }
    fn train_episode(&mut self, rewards: &[f64]) {
        self.base.train_episode(rewards)
    }
    fn save(&self, path: &str) {
        self.base.save(path)
    }
    fn load(&mut self, path: &str) {
        self.base.load(path)
    }
    fn name(&self) -> &'static str {
        "ActorCriticDuelRecurrent"
    }
    fn model(&self) -> &dyn Model {
        self.base.model()
    }
    fn model_mut(&mut self) -> &mut dyn Model {
        self.base.model_mut()
    }
    fn reset(&mut self) {
        self.base.reset()
    }
}

// â”€â”€â”€ Policy Gradient â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

pub struct PolicyGradient {
    pub model: Box<dyn Model>,
    pub learning_rate: f64,
    pub gamma: f64,
    pub last_action: Action,
}

impl PolicyGradient {
    pub fn new(model: Box<dyn Model>, learning_rate: f64, gamma: f64) -> Self {
        PolicyGradient {
            model,
            learning_rate,
            gamma,
            last_action: Action::hold(),
        }
    }
}

impl Agent for PolicyGradient {
    fn act(&mut self, state: &[f64]) -> Action {
        let logits = self.model.forward(state);
        let idx = softmax_index(&logits);
        self.last_action = action_from_index(idx, logits.len());
        self.last_action.clone()
    }

    fn train_episode(&mut self, rewards: &[f64]) {
        let r: f64 = rewards.iter().sum();
        let scale = r.max(-1.0).min(1.0);
        self.model.update(self.learning_rate * scale);
    }

    fn save(&self, path: &str) {
        let p = self.model.parameters();
        let s: Vec<String> = p.iter().map(|v| v.to_string()).collect();
        let _ = std::fs::write(path, s.join("\n"));
    }

    fn load(&mut self, path: &str) {
        if let Ok(content) = std::fs::read_to_string(path) {
            let vals: Vec<f64> = content.lines().filter_map(|l| l.parse().ok()).collect();
            if vals.len() == self.model.parameter_len() {
                self.model.set_parameters(&vals);
            }
        }
    }

    fn name(&self) -> &'static str {
        "PolicyGradient"
    }

    fn model(&self) -> &dyn Model {
        &*self.model
    }

    fn model_mut(&mut self) -> &mut dyn Model {
        &mut *self.model
    }

    fn reset(&mut self) {
        self.model.reset();
    }
}

// â”€â”€â”€ Curiosity family â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

pub struct CuriosityQLearning {
    pub base: QLearning,
    pub curiosity_weight: f64,
    pub novelty: Vec<Vec<f64>>,
}

impl CuriosityQLearning {
    pub fn new(model: Box<dyn Model>, state_size: usize, action_size: usize, curiosity_weight: f64) -> Self {
        CuriosityQLearning {
            base: QLearning::new(model, state_size, action_size),
            curiosity_weight,
            novelty: Vec::new(),
        }
    }

    fn novelty_bonus(&self, state: &[f64]) -> f64 {
        let mut best = f64::MAX;
        for prev in self.novelty.iter().rev().take(20) {
            let d: f64 = state.iter().zip(prev).map(|(a, b)| (a - b).abs()).sum();
            if d < best {
                best = d;
            }
        }
        if best == f64::MAX {
            0.0
        } else {
            best
        }
    }
}

impl Agent for CuriosityQLearning {
    fn act(&mut self, state: &[f64]) -> Action {
        self.novelty.push(state.to_vec());
        if self.novelty.len() > 100 {
            self.novelty.remove(0);
        }
        self.base.act(state)
    }

    fn train_episode(&mut self, rewards: &[f64]) {
        self.base.train_episode(rewards);
        self.novelty.clear();
    }

    fn save(&self, path: &str) {
        self.base.save(path)
    }

    fn load(&mut self, path: &str) {
        self.base.load(path)
    }

    fn name(&self) -> &'static str {
        "CuriosityQLearning"
    }

    fn model(&self) -> &dyn Model {
        self.base.model()
    }

    fn model_mut(&mut self) -> &mut dyn Model {
        self.base.model_mut()
    }

    fn reset(&mut self) {
        self.base.reset();
        self.novelty.clear();
    }
}

pub struct RecurrentCuriosityQLearning {
    pub base: QLearning,
    pub curiosity_weight: f64,
    pub novelty: Vec<Vec<f64>>,
}

impl RecurrentCuriosityQLearning {
    pub fn new(model: Box<dyn Model>, state_size: usize, action_size: usize) -> Self {
        let mut base = QLearning::new(model, state_size, action_size);
        base.recurrent = true;
        RecurrentCuriosityQLearning {
            base,
            curiosity_weight: 0.1,
            novelty: Vec::new(),
        }
    }
}

impl Agent for RecurrentCuriosityQLearning {
    fn act(&mut self, state: &[f64]) -> Action {
        self.novelty.push(state.to_vec());
        if self.novelty.len() > 100 {
            self.novelty.remove(0);
        }
        self.base.act(state)
    }

    fn train_episode(&mut self, rewards: &[f64]) {
        self.base.train_episode(rewards);
        self.novelty.clear();
    }

    fn save(&self, path: &str) {
        self.base.save(path)
    }

    fn load(&mut self, path: &str) {
        self.base.load(path)
    }

    fn name(&self) -> &'static str {
        "RecurrentCuriosityQLearning"
    }

    fn model(&self) -> &dyn Model {
        self.base.model()
    }

    fn model_mut(&mut self) -> &mut dyn Model {
        self.base.model_mut()
    }

    fn reset(&mut self) {
        self.base.reset();
        self.novelty.clear();
    }
}

pub struct DuelingCuriosityQLearning {
    pub base: QLearning,
    pub curiosity_weight: f64,
    pub novelty: Vec<Vec<f64>>,
}

impl DuelingCuriosityQLearning {
    pub fn new(model: Box<dyn Model>, state_size: usize, action_size: usize) -> Self {
        let mut base = QLearning::new(model, state_size, action_size);
        base.dueling = true;
        DuelingCuriosityQLearning {
            base,
            curiosity_weight: 0.1,
            novelty: Vec::new(),
        }
    }
}

impl Agent for DuelingCuriosityQLearning {
    fn act(&mut self, state: &[f64]) -> Action {
        self.novelty.push(state.to_vec());
        if self.novelty.len() > 100 {
            self.novelty.remove(0);
        }
        self.base.act(state)
    }

    fn train_episode(&mut self, rewards: &[f64]) {
        self.base.train_episode(rewards);
        self.novelty.clear();
    }

    fn save(&self, path: &str) {
        self.base.save(path)
    }

    fn load(&mut self, path: &str) {
        self.base.load(path)
    }

    fn name(&self) -> &'static str {
        "DuelingCuriosityQLearning"
    }

    fn model(&self) -> &dyn Model {
        self.base.model()
    }

    fn model_mut(&mut self) -> &mut dyn Model {
        self.base.model_mut()
    }

    fn reset(&mut self) {
        self.base.reset();
        self.novelty.clear();
    }
}

// â”€â”€â”€ NeuroEvolution â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

pub struct NeuroEvolution {
    pub population: Vec<Box<dyn Model>>,
    pub mutation_rate: f64,
    pub input_size: usize,
    pub output_size: usize,
    pub fitness: Vec<f64>,
    pub episode: usize,
    pub current: usize,
    model_factory: fn(usize, usize) -> Box<dyn Model>,
}

impl NeuroEvolution {
    pub fn new(
        pop_size: usize,
        mutation_rate: f64,
        input_size: usize,
        output_size: usize,
        model_factory: fn(usize, usize) -> Box<dyn Model>,
    ) -> Self {
        let population: Vec<Box<dyn Model>> =
            (0..pop_size).map(|_| model_factory(input_size, output_size)).collect();
        NeuroEvolution {
            population,
            mutation_rate,
            input_size,
            output_size,
            fitness: vec![0.0; pop_size],
            episode: 0,
            current: 0,
            model_factory,
        }
    }

    pub fn crossover(&self, p1: &dyn Model, p2: &dyn Model) -> Box<dyn Model> {
        let mut child = (self.model_factory)(self.input_size, self.output_size);
        let a = p1.parameters();
        let b = p2.parameters();
        let params: Vec<f64> = a
            .iter()
            .zip(&b)
            .map(|(x, y)| if rand::random::<f64>() < 0.5 { *x } else { *y })
            .collect();
        child.set_parameters(&params);
        child
    }

    pub fn mutate(&self, model: &mut dyn Model) {
        let mut params = model.parameters();
        for v in params.iter_mut() {
            if rand::random::<f64>() < self.mutation_rate {
                *v += rand::random::<f64>() * 2.0 - 1.0;
            }
        }
        model.set_parameters(&params);
    }

    fn evolve(&mut self) {
        let len = self.population.len();
        if len == 0 {
            return;
        }
        let best = (0..len)
            .max_by(|&a, &b| self.fitness[a].partial_cmp(&self.fitness[b]).unwrap())
            .unwrap_or(0);
        let best_params = self.population[best].parameters();
        let mut new_pop: Vec<Box<dyn Model>> = Vec::new();
        let mut champion = (self.model_factory)(self.input_size, self.output_size);
        champion.set_parameters(&best_params);
        new_pop.push(champion);
        for i in 1..len {
            let p1 = self.population[i % len].as_ref();
            let p2 = self.population[best].as_ref();
            let mut child = self.crossover(p1, p2);
            self.mutate(&mut *child);
            new_pop.push(child);
        }
        self.population = new_pop;
        self.current = 0;
    }
}

impl Agent for NeuroEvolution {
    fn act(&mut self, state: &[f64]) -> Action {
        let idx = self.episode % self.population.len();
        self.current = idx;
        let logits = self.population[idx].forward(state);
        action_from_logits(&logits)
    }

    fn train_episode(&mut self, rewards: &[f64]) {
        let r: f64 = rewards.iter().sum();
        let idx = self.episode % self.population.len();
        self.fitness[idx] += r;
        self.episode += 1;
        if self.episode % self.population.len() == 0 {
            self.evolve();
            self.fitness.fill(0.0);
        }
    }

    fn save(&self, path: &str) {
        let p = self.population[self.current].parameters();
        let s: Vec<String> = p.iter().map(|v| v.to_string()).collect();
        let _ = std::fs::write(path, s.join("\n"));
    }

    fn load(&mut self, path: &str) {
        if let Ok(content) = std::fs::read_to_string(path) {
            let vals: Vec<f64> = content.lines().filter_map(|l| l.parse().ok()).collect();
            if vals.len() == self.population[self.current].parameter_len() {
                self.population[self.current].set_parameters(&vals);
            }
        }
    }

    fn name(&self) -> &'static str {
        "NeuroEvolution"
    }

    fn model(&self) -> &dyn Model {
        &*self.population[self.current]
    }

    fn model_mut(&mut self) -> &mut dyn Model {
        &mut *self.population[self.current]
    }

    fn reset(&mut self) {
        self.episode = 0;
        self.current = 0;
        self.fitness.fill(0.0);
        for m in self.population.iter_mut() {
            m.reset();
        }
    }
}

// â”€â”€â”€ NeuroEvolution + Novelty Search â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

pub struct NeuroEvolutionNoveltySearch {
    pub base: NeuroEvolution,
    pub novelty_archive: Vec<Vec<f64>>,
}

impl NeuroEvolutionNoveltySearch {
    pub fn new(
        pop_size: usize,
        mutation_rate: f64,
        input_size: usize,
        output_size: usize,
        model_factory: fn(usize, usize) -> Box<dyn Model>,
    ) -> Self {
        NeuroEvolutionNoveltySearch {
            base: NeuroEvolution::new(pop_size, mutation_rate, input_size, output_size, model_factory),
            novelty_archive: Vec::new(),
        }
    }
}

impl Agent for NeuroEvolutionNoveltySearch {
    fn act(&mut self, state: &[f64]) -> Action {
        self.base.act(state)
    }

    fn train_episode(&mut self, rewards: &[f64]) {
        let r: f64 = rewards.iter().sum();
        let novelty = if let Some(prev) = self.novelty_archive.last() {
            let d: f64 = self.base.population[self.base.current].parameters().iter().zip(prev.iter()).map(|(a, b)| (a - b).abs()).sum();
            d
        } else {
            0.0
        };
        let bonus = rewards.iter().sum::<f64>() + novelty * 0.01;
        let _ = bonus;
        self.novelty_archive.push(self.base.population[self.base.current].parameters());
        if self.novelty_archive.len() > 200 {
            self.novelty_archive.remove(0);
        }
        let _ = r;
        self.base.train_episode(rewards);
    }

    fn save(&self, path: &str) {
        self.base.save(path)
    }

    fn load(&mut self, path: &str) {
        self.base.load(path)
    }

    fn name(&self) -> &'static str {
        "NeuroEvolutionNoveltySearch"
    }

    fn model(&self) -> &dyn Model {
        self.base.model()
    }

    fn model_mut(&mut self) -> &mut dyn Model {
        self.base.model_mut()
    }

    fn reset(&mut self) {
        self.base.reset();
        self.novelty_archive.clear();
    }
}

// â”€â”€â”€ NES (Natural Evolution Strategies) â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

pub struct NESAgent {
    pub model: Box<dyn Model>,
    pub sigma: f64,
    pub learning_rate: f64,
    pub mean: Vec<f64>,
    pub noise: Vec<f64>,
    pub grad: Vec<f64>,
    pub episode: usize,
}

impl NESAgent {
    pub fn new(mut model: Box<dyn Model>, sigma: f64, learning_rate: f64) -> Self {
        let mean = model.parameters();
        let n = mean.len();
        let noise: Vec<f64> = (0..n).map(|_| rand::random::<f64>() * 2.0 - 1.0).collect();
        let p: Vec<f64> = mean.iter().zip(&noise).map(|(m, e)| m + sigma * e).collect();
        model.set_parameters(&p);
        NESAgent {
            model,
            sigma,
            learning_rate,
            mean,
            noise,
            grad: vec![0.0; n],
            episode: 0,
        }
    }
}

impl Agent for NESAgent {
    fn act(&mut self, state: &[f64]) -> Action {
        let logits = self.model.forward(state);
        action_from_logits(&logits)
    }

    fn train_episode(&mut self, rewards: &[f64]) {
        let r: f64 = rewards.iter().sum();
        for (g, e) in self.grad.iter_mut().zip(&self.noise) {
            *g += r * e;
        }
        self.episode += 1;
        if self.episode % 10 == 0 {
            let norm = (self.sigma * 10.0).max(1e-8);
            for i in 0..self.mean.len() {
                self.mean[i] += self.learning_rate * self.grad[i] / norm;
            }
            self.grad.fill(0.0);
        }
        let noise: Vec<f64> = (0..self.mean.len()).map(|_| rand::random::<f64>() * 2.0 - 1.0).collect();
        let p: Vec<f64> = self.mean.iter().zip(&noise).map(|(m, e)| m + self.sigma * e).collect();
        self.noise = noise;
        self.model.set_parameters(&p);
    }

    fn save(&self, path: &str) {
        let s: Vec<String> = self.mean.iter().map(|v| v.to_string()).collect();
        let _ = std::fs::write(path, s.join("\n"));
    }

    fn load(&mut self, path: &str) {
        if let Ok(content) = std::fs::read_to_string(path) {
            let vals: Vec<f64> = content.lines().filter_map(|l| l.parse().ok()).collect();
            if vals.len() == self.mean.len() {
                self.mean = vals;
            }
        }
    }

    fn name(&self) -> &'static str {
        "NESAgent"
    }

    fn model(&self) -> &dyn Model {
        &*self.model
    }

    fn model_mut(&mut self) -> &mut dyn Model {
        &mut *self.model
    }

    fn reset(&mut self) {
        self.episode = 0;
        self.grad.fill(0.0);
        self.model.reset();
        self.mean = self.model.parameters();
    }
}

// â”€â”€â”€ ES + Bayesian â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

pub struct EvolutionStrategyBayesian {
    pub base: EvolutionStrategy,
    pub sigma_min: f64,
    pub sigma_max: f64,
    pub performance: Vec<f64>,
}

impl EvolutionStrategyBayesian {
    pub fn new(model: Box<dyn Model>, pop: usize, sigma: f64, lr: f64) -> Self {
        EvolutionStrategyBayesian {
            base: EvolutionStrategy::new(model, pop, sigma, lr),
            sigma_min: sigma * 0.3,
            sigma_max: sigma * 3.0,
            performance: Vec::new(),
        }
    }
}

impl Agent for EvolutionStrategyBayesian {
    fn act(&mut self, state: &[f64]) -> Action {
        self.base.act(state)
    }

    fn train_episode(&mut self, rewards: &[f64]) {
        let r: f64 = rewards.iter().sum();
        self.performance.push(r);
        if self.performance.len() > 100 {
            self.performance.remove(0);
        }
        self.base.train_episode(rewards);
    }

    fn save(&self, path: &str) {
        self.base.save(path)
    }

    fn load(&mut self, path: &str) {
        self.base.load(path)
    }

    fn name(&self) -> &'static str {
        "EvolutionStrategyBayesian"
    }

    fn model(&self) -> &dyn Model {
        self.base.model()
    }

    fn model_mut(&mut self) -> &mut dyn Model {
        self.base.model_mut()
    }

    fn reset(&mut self) {
        self.base.reset();
        self.performance.clear();
    }
}

// â”€â”€â”€ DNC Agent â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

pub struct DNBAgent {
    pub model: Box<dyn Model>,
    pub episode_count: usize,
}

impl DNBAgent {
    pub fn new(model: Box<dyn Model>) -> Self {
        DNBAgent {
            model,
            episode_count: 0,
        }
    }
}

impl Agent for DNBAgent {
    fn act(&mut self, state: &[f64]) -> Action {
        let logits = self.model.forward(state);
        action_from_logits(&logits)
    }

    fn train_episode(&mut self, rewards: &[f64]) {
        let r: f64 = rewards.iter().sum();
        let scale = if r > 0.0 { 1.0 } else if r < 0.0 { -1.0 } else { 0.0 };
        self.model.update(0.001 * scale);
        self.episode_count += 1;
    }

    fn save(&self, path: &str) {
        let p = self.model.parameters();
        let s: Vec<String> = p.iter().map(|v| v.to_string()).collect();
        let _ = std::fs::write(path, s.join("\n"));
    }

    fn load(&mut self, path: &str) {
        if let Ok(content) = std::fs::read_to_string(path) {
            let vals: Vec<f64> = content.lines().filter_map(|l| l.parse().ok()).collect();
            if vals.len() == self.model.parameter_len() {
                self.model.set_parameters(&vals);
            }
        }
    }

    fn name(&self) -> &'static str {
        "DNBAgent"
    }

    fn model(&self) -> &dyn Model {
        &*self.model
    }

    fn model_mut(&mut self) -> &mut dyn Model {
        &mut *self.model
    }

    fn reset(&mut self) {
        self.model.reset();
        self.episode_count = 0;
    }
}

// â”€â”€â”€ Turtle Agent â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

pub struct TurtleAgent {
    pub dummy: NoOpModel,
    pub prev: f64,
}

impl TurtleAgent {
    pub fn new() -> Self {
        TurtleAgent {
            dummy: NoOpModel,
            prev: 0.0,
        }
    }
}

impl Agent for TurtleAgent {
    fn act(&mut self, state: &[f64]) -> Action {
        let price = state.first().copied().unwrap_or(0.0);
        let mut action = Action::hold();
        if self.prev > 0.0 {
            if price > self.prev {
                action = Action::market_buy(1);
            } else if price < self.prev {
                action = Action::market_sell(1);
            }
        }
        self.prev = price;
        action
    }

    fn train_episode(&mut self, _rewards: &[f64]) {}

    fn save(&self, _path: &str) {}

    fn load(&mut self, _path: &str) {}

    fn name(&self) -> &'static str {
        "TurtleAgent"
    }

    fn model(&self) -> &dyn Model {
        &self.dummy
    }

    fn model_mut(&mut self) -> &mut dyn Model {
        &mut self.dummy
    }

    fn reset(&mut self) {
        self.prev = 0.0;
    }
}

// â”€â”€â”€ Signal Rolling Agent â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

pub struct SignalRollingAgent {
    pub dummy: NoOpModel,
    pub window: Vec<f64>,
    pub threshold: f64,
}

impl SignalRollingAgent {
    pub fn new(threshold: f64) -> Self {
        SignalRollingAgent {
            dummy: NoOpModel,
            window: Vec::new(),
            threshold,
        }
    }
}

impl Agent for SignalRollingAgent {
    fn act(&mut self, state: &[f64]) -> Action {
        let price = state.first().copied().unwrap_or(0.0);
        self.window.push(price);
        if self.window.len() > 20 {
            self.window.remove(0);
        }
        let mut action = Action::hold();
        if self.window.len() >= 10 {
            let mean = self.window.iter().sum::<f64>() / self.window.len() as f64;
            let mom = (price - mean) / mean.abs().max(1e-8);
            if mom > self.threshold {
                action = Action::market_buy(1);
            } else if mom < -self.threshold {
                action = Action::market_sell(1);
            }
        }
        action
    }

    fn train_episode(&mut self, _rewards: &[f64]) {
        self.window.clear();
    }

    fn save(&self, _path: &str) {}

    fn load(&mut self, _path: &str) {}

    fn name(&self) -> &'static str {
        "SignalRollingAgent"
    }

    fn model(&self) -> &dyn Model {
        &self.dummy
    }

    fn model_mut(&mut self) -> &mut dyn Model {
        &mut self.dummy
    }

    fn reset(&mut self) {
        self.window.clear();
    }
}

// â”€â”€â”€ Moving Average Agent â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

pub struct MovingAverageAgent {
    pub dummy: NoOpModel,
    pub short: usize,
    pub long: usize,
    pub prices: Vec<f64>,
}

impl MovingAverageAgent {
    pub fn new(short: usize, long: usize) -> Self {
        MovingAverageAgent {
            dummy: NoOpModel,
            short,
            long,
            prices: Vec::new(),
        }
    }
}

impl Agent for MovingAverageAgent {
    fn act(&mut self, state: &[f64]) -> Action {
        let price = state.first().copied().unwrap_or(0.0);
        self.prices.push(price);
        let cap = self.long * 2;
        if self.prices.len() > cap {
            let drop = self.prices.len() - cap;
            self.prices.drain(0..drop);
        }
        let mut action = Action::hold();
        if self.prices.len() >= self.long && self.short < self.prices.len() {
            let n = self.prices.len();
            let sma_s: f64 = self.prices[n - self.short..].iter().sum::<f64>() / self.short as f64;
            let sma_l: f64 = self.prices.iter().sum::<f64>() / n as f64;
            if sma_s > sma_l {
                action = Action::market_buy(1);
            } else if sma_s < sma_l {
                action = Action::market_sell(1);
            }
        }
        action
    }

    fn train_episode(&mut self, _rewards: &[f64]) {
        self.prices.clear();
    }

    fn save(&self, _path: &str) {}

    fn load(&mut self, _path: &str) {}

    fn name(&self) -> &'static str {
        "MovingAverageAgent"
    }

    fn model(&self) -> &dyn Model {
        &self.dummy
    }

    fn model_mut(&mut self) -> &mut dyn Model {
        &mut self.dummy
    }

    fn reset(&mut self) {
        self.prices.clear();
    }
}

// â”€â”€â”€ ABCD Pattern Agent â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

pub struct ABCDStrategyAgent {
    pub dummy: NoOpModel,
    pub phase: u8,
    pub prices: Vec<f64>,
}

impl ABCDStrategyAgent {
    pub fn new() -> Self {
        ABCDStrategyAgent {
            dummy: NoOpModel,
            phase: 0,
            prices: Vec::new(),
        }
    }
}

impl Agent for ABCDStrategyAgent {
    fn act(&mut self, state: &[f64]) -> Action {
        let price = state.first().copied().unwrap_or(0.0);
        self.prices.push(price);
        if self.prices.len() > 60 {
            self.prices.remove(0);
        }
        let mut action = Action::hold();
        if self.prices.len() >= 20 {
            let a = self.prices[0];
            let b = self.prices[5];
            let c = self.prices[10];
            let d = self.prices[self.prices.len() - 1];
            if a < b && b > c && c < d {
                self.phase = 1;
                action = Action::market_buy(1);
            } else if a > b && b < c && c > d {
                self.phase = 2;
                action = Action::market_sell(1);
            } else {
                self.phase = 0;
            }
        }
        action
    }

    fn train_episode(&mut self, _rewards: &[f64]) {
        self.prices.clear();
    }

    fn save(&self, _path: &str) {}

    fn load(&mut self, _path: &str) {}

    fn name(&self) -> &'static str {
        "ABCDStrategyAgent"
    }

    fn model(&self) -> &dyn Model {
        &self.dummy
    }

    fn model_mut(&mut self) -> &mut dyn Model {
        &mut self.dummy
    }

    fn reset(&mut self) {
        self.phase = 0;
        self.prices.clear();
    }
}

// â”€â”€â”€ Agent1 (mean reversion) â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

pub struct Agent1 {
    pub dummy: NoOpModel,
    pub prices: Vec<f64>,
}

impl Agent1 {
    pub fn new() -> Self {
        Agent1 {
            dummy: NoOpModel,
            prices: Vec::new(),
        }
    }
}

impl Agent for Agent1 {
    fn act(&mut self, state: &[f64]) -> Action {
        let price = state.first().copied().unwrap_or(0.0);
        self.prices.push(price);
        if self.prices.len() > 50 {
            self.prices.remove(0);
        }
        let mut action = Action::hold();
        if self.prices.len() >= 20 {
            let mean = self.prices.iter().sum::<f64>() / self.prices.len() as f64;
            let z = (price - mean) / (mean.abs() * 0.005 + 1e-8);
            if z < -1.0 {
                action = Action::market_buy(1);
            } else if z > 1.0 {
                action = Action::market_sell(1);
            }
        }
        action
    }

    fn train_episode(&mut self, _rewards: &[f64]) {
        self.prices.clear();
    }

    fn save(&self, _path: &str) {}

    fn load(&mut self, _path: &str) {}

    fn name(&self) -> &'static str {
        "Agent1"
    }

    fn model(&self) -> &dyn Model {
        &self.dummy
    }

    fn model_mut(&mut self) -> &mut dyn Model {
        &mut self.dummy
    }

    fn reset(&mut self) {
        self.prices.clear();
    }
}

// â”€â”€â”€ RealtimeAgent â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

pub struct RealtimeAgent {
    pub model: Box<dyn Model>,
    pub window_size: usize,
    pub initial_capital: f64,
    pub cash: f64,
    pub position: f64,
    pub prices: Vec<f64>,
}

impl RealtimeAgent {
    pub fn new(model: Box<dyn Model>, window_size: usize, initial_capital: f64) -> Self {
        RealtimeAgent {
            model,
            window_size,
            initial_capital,
            cash: initial_capital,
            position: 0.0,
            prices: Vec::new(),
        }
    }

    pub fn trade(&mut self, price: f64, _volume: f64) -> Action {
        self.prices.push(price);
        if self.prices.len() > self.window_size {
            self.prices.remove(0);
        }
        let logits = self.model.forward(&self.prices);
        action_from_logits(&logits)
    }
}
