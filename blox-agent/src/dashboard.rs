use crate::action::action_dim;
use crate::agent::Agent;
use crate::features::FeatureExtractor;
use crate::model::Model;
use crate::train::{BloxEnv, Metrics};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub struct TrainingRun {
    pub agent_name: String,
    pub feature_name: String,
    pub model_name: String,
    pub episode: usize,
    pub reward: f64,
    pub pnl: f64,
    pub sharpe: f64,
    pub completed: bool,
    pub equity_curve: Vec<f64>,
    pub ep_rewards: Vec<f64>,
}

pub struct AppState {
    pub runs: Vec<TrainingRun>,
    pub results: Vec<(String, String, String, Metrics)>,
    pub training: bool,
    pub prices: Vec<f64>,
    pub last_trade_price: f64,
    pub extractors: Vec<String>,
    pub agents: Vec<String>,
    pub models: Vec<String>,
    pub connected: bool,
    pub blox_server_addr: String,
    pub instruments: Vec<u16>,
    pub debug_buys: usize,
    pub debug_sells: usize,
    pub debug_holds: usize,
}

impl AppState {
    pub fn new() -> Self {
        AppState {
            runs: Vec::new(),
            results: Vec::new(),
            training: false,
            prices: Vec::new(),
            last_trade_price: 0.0,
            extractors: vec![
                "PriceVolume".into(), "Range".into(), "ATR".into(),
                "SMA".into(), "BookDepth".into(), "HFQ".into(),
                "Combined".into(),
            ],
            agents: vec![
                "ES".into(), "QLearning".into(),
                "DoubleQLearning".into(), "DuelingQLearning".into(),
                "RecurrentQLearning".into(), "DoubleDuelingQLearning".into(),
                "DoubleRecurrentQLearning".into(), "DuelingRecurrentQLearning".into(),
                "DoubleDuelingRecurrentQLearning".into(),
                "ActorCritic".into(), "ActorCriticRecurrent".into(),
                "ActorCriticDuel".into(), "ActorCriticDuelRecurrent".into(),
                "PolicyGradient".into(),
                "CuriosityQLearning".into(), "RecurrentCuriosityQLearning".into(),
                "DuelingCuriosityQLearning".into(),
                "NeuroEvolution".into(), "NeuroEvolutionNoveltySearch".into(),
                "NESAgent".into(), "EvolutionStrategyBayesian".into(),
                "DNBAgent".into(),
                "TurtleAgent".into(), "SignalRollingAgent".into(),
                "MovingAverageAgent".into(), "ABCDStrategyAgent".into(),
                "Agent1".into(),
            ],
            models: vec![
                "MLP".into(), "LSTM".into(), "GRU".into(),
                "Attention".into(), "DNC".into(), "Vanilla".into(),
            ],
            connected: false,
            blox_server_addr: "127.0.0.1:7070".into(),
            instruments: vec![1],
            debug_buys: 0, debug_sells: 0, debug_holds: 0,
        }
    }
}

// ─── Price streaming from blox-server ──────────────────────────────────────────

pub fn connect_to_server(state: &Arc<Mutex<AppState>>) {
    let s1 = Arc::clone(state);
    let s2 = Arc::clone(state);
    let s3 = Arc::clone(state);

    // Thread 1: subscribe to pushed events (EV TRADE, EV TOP)
    thread::spawn(move || loop {
        let addr = { let s = s1.lock().unwrap(); s.blox_server_addr.clone() };
        let insts = { let s = s1.lock().unwrap(); s.instruments.clone() };
        match TcpStream::connect(&addr) {
            Ok(mut stream) => {
                { let mut s = s1.lock().unwrap(); s.connected = true; }
                let mut sub = String::new();
                for inst in &insts { sub.push_str(&format!("SUB {}\n", inst)); }
                let _ = stream.write_all(sub.as_bytes());
                let mut buf = [0u8; 4096];
                let mut pending = String::new();
                loop {
                    match stream.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            pending.push_str(&String::from_utf8_lossy(&buf[..n]));
                            while let Some(pos) = pending.find('\n') {
                                let line = pending[..pos].trim().to_string();
                                pending = pending[pos + 1..].to_string();
                                if let Some(price) = ev_price(&line) {
                                    let mut s = s1.lock().unwrap();
                                    s.last_trade_price = price;
                                    s.prices.push(price);
                                    let plen = s.prices.len();
                                    if plen > 5000 {
                                        s.prices.drain(0..plen - 4000);
                                    }
                                }
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
            Err(_) => {
                { let mut s = s1.lock().unwrap(); s.connected = false; }
                thread::sleep(Duration::from_secs(1));
            }
        }
    });

    // Thread 2: poll BOOK every 50ms for fast quotes
    thread::spawn(move || loop {
        let addr = { let s = s2.lock().unwrap(); s.blox_server_addr.clone() };
        let insts = { let s = s2.lock().unwrap(); s.instruments.clone() };
        match TcpStream::connect(&addr) {
            Ok(mut stream) => {
                let _ = stream.set_nonblocking(true);
                let mut buf = [0u8; 4096];
                let mut pending = String::new();
                for inst in &insts {
                    let _ = stream.write_all(format!("QUIET 1\nBOOK {} 1\n", inst).as_bytes());
                }
                loop {
                    match stream.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            pending.push_str(&String::from_utf8_lossy(&buf[..n]));
                            while let Some(pos) = pending.find('\n') {
                                let line = pending[..pos].trim().to_string();
                                pending = pending[pos + 1..].to_string();
                                if let Some(price) = book_price(&line) {
                                    let mut s = s2.lock().unwrap();
                                    s.prices.push(price);
                                    let plen = s.prices.len();
                                    if plen > 5000 {
                                        s.prices.drain(0..plen - 4000);
                                    }
                                }
                            }
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                        Err(_) => break,
                    }
                    thread::sleep(Duration::from_millis(50));
                    for inst in &insts {
                        let _ = stream.write_all(format!("BOOK {} 1\n", inst).as_bytes());
                    }
                }
            }
            Err(_) => thread::sleep(Duration::from_secs(1)),
        }
    });

    // Thread 3: push last_trade_price every 200ms to keep prices flowing
    thread::spawn(move || loop {
        thread::sleep(Duration::from_millis(200));
        let mut s = s3.lock().unwrap();
        let tp = s.last_trade_price;
        if tp > 0.0 {
            let last = s.prices.last().copied().unwrap_or(0.0);
            if (tp - last).abs() > 0.0001 {
                s.prices.push(tp);
                let plen = s.prices.len();
                if plen > 5000 { s.prices.drain(0..plen - 4000); }
            }
        }
    });
}

fn ev_price(line: &str) -> Option<f64> {
    let p: Vec<&str> = line.split_whitespace().collect();
    if p.len() >= 7 && p[0] == "EV" && p[1] == "TRADE" {
        p[3].parse::<i64>().ok().map(|t| t as f64 / 100.0)
    } else if p.len() >= 6 && p[0] == "EV" && p[1] == "TOP" {
        if let Ok(b) = p[3].parse::<i64>() { if b > 0 { return Some(b as f64 / 100.0); } }
        if let Ok(a) = p[5].parse::<i64>() { if a > 0 { return Some(a as f64 / 100.0); } }
        None
    } else { None }
}

fn book_price(line: &str) -> Option<f64> {
    let p: Vec<&str> = line.split_whitespace().collect();
    if p.len() >= 3 && p[0] == "BOOK" && p[2] != "-" {
        p[2].split(',').next()?.split(':').next()?.parse::<i64>().ok().map(|t| t as f64 / 100.0)
    } else { None }
}

// ─── Training ───────────────────────────────────────────────────────────────────

pub fn start_training(state: &Arc<Mutex<AppState>>) {
    let state_clone = Arc::clone(state);
    thread::spawn(move || {
        for _ in 0..50 {
            if { let s = state_clone.lock().unwrap(); s.prices.len() >= 200 } { break; }
            thread::sleep(Duration::from_millis(200));
        }

        let prices = {
            let mut s = state_clone.lock().unwrap();
            s.training = true;
            s.runs.clear();
            s.results.clear();
            let p = s.prices.clone();
            if p.len() < 10000 {
                let last = p.last().copied().unwrap_or(100.0);
                let more = 10000 - p.len();
                let pad: Vec<f64> = (0..more).map(|_| last + (rand::random::<f64>() - 0.5) * 0.5).collect();
                let mut f = p; f.extend(pad); s.prices = f.clone(); f
            } else { p }
        };

        let extractors: Vec<(&str, Box<dyn Fn() -> Box<dyn FeatureExtractor>>)> = vec![
            ("PriceVolume", Box::new(|| -> Box<dyn FeatureExtractor> { Box::new(crate::features::PriceVolume::new()) }) as Box<dyn Fn() -> Box<dyn FeatureExtractor>>),
            ("Range", Box::new(|| -> Box<dyn FeatureExtractor> { Box::new(crate::features::Range::new()) })),
            ("ATR", Box::new(|| -> Box<dyn FeatureExtractor> { Box::new(crate::features::ATR::new(14)) })),
            ("SMA", Box::new(|| -> Box<dyn FeatureExtractor> { Box::new(crate::features::SMA::new(&[10, 30, 50])) })),
            ("BookDepth", Box::new(|| -> Box<dyn FeatureExtractor> { Box::new(crate::features::BookDepth::new(5)) })),
            ("HFQ", Box::new(|| -> Box<dyn FeatureExtractor> { Box::new(crate::features::HFQ::new()) })),
            ("Combined", Box::new(|| -> Box<dyn FeatureExtractor> { Box::new(crate::features::Combined::new(vec![
                Box::new(crate::features::PriceVolume::new()),
                Box::new(crate::features::Range::new()),
                Box::new(crate::features::ATR::new(14)),
                Box::new(crate::features::SMA::new(&[10, 30])),
            ])) })),
        ];

        let models: Vec<(&str, fn(usize, usize) -> Box<dyn Model>)> = vec![
            ("MLP", |i, o| Box::new(crate::model::MLP::new(&[i, 128, 64, o]))),
            ("LSTM", |i, o| Box::new(crate::model::LSTM::new(i, o))),
            ("GRU", |i, o| Box::new(crate::model::GRU::new(i, o))),
            ("Attention", |i, _| Box::new(crate::model::Attention::new(i))),
            ("DNC", |i, o| Box::new(crate::model::DNC::new(i, 4, o, 2))),
            ("Vanilla", |i, o| Box::new(crate::model::Vanilla::new(i, o))),
        ];

        let agents: Vec<(&str, fn(Box<dyn Model>, usize, usize) -> Box<dyn Agent>)> = vec![
            ("ES", |m,_,_| Box::new(crate::agent::EvolutionStrategy::new(m,10,0.1,0.03))),
            ("QLearning", |m,i,o| Box::new(crate::agent::QLearning::new(m,i,o))),
            ("DoubleQLearning", |m,i,o| Box::new(crate::agent::DoubleQLearning::new(m,i,o))),
            ("DuelingQLearning", |m,i,o| Box::new(crate::agent::DuelingQLearning::new(m,i,o))),
            ("RecurrentQLearning",|m,i,o|Box::new(crate::agent::RecurrentQLearning::new(m,i,o))),
            ("DoubleDuelingQLearning",|m,i,o|Box::new(crate::agent::DoubleDuelingQLearning::new(m,i,o))),
            ("DoubleRecurrentQLearning",|m,i,o|Box::new(crate::agent::DoubleRecurrentQLearning::new(m,i,o))),
            ("DuelingRecurrentQLearning",|m,i,o|Box::new(crate::agent::DuelingRecurrentQLearning::new(m,i,o))),
            ("DoubleDuelingRecurrentQLearning",|m,i,o|Box::new(crate::agent::DoubleDuelingRecurrentQLearning::new(m,i,o))),
            ("ActorCritic", |m,i,_| Box::new(crate::agent::ActorCritic::new(m,Box::new(crate::model::MLP::new(&[i,32,1])),0.99,0.001))),
            ("ActorCriticRecurrent",|m,i,_|Box::new(crate::agent::ActorCriticRecurrent::new(m,Box::new(crate::model::MLP::new(&[i,32,1])),0.99,0.001))),
            ("ActorCriticDuel",|m,i,_|Box::new(crate::agent::ActorCriticDuel::new(m,Box::new(crate::model::MLP::new(&[i,32,1])),0.99,0.001))),
            ("ActorCriticDuelRecurrent",|m,i,_|Box::new(crate::agent::ActorCriticDuelRecurrent::new(m,Box::new(crate::model::MLP::new(&[i,32,1])),0.99,0.001))),
            ("PolicyGradient", |m,_,_| Box::new(crate::agent::PolicyGradient::new(m,0.001,0.99))),
            ("CuriosityQLearning",|m,i,o|Box::new(crate::agent::CuriosityQLearning::new(m,i,o,0.1))),
            ("RecurrentCuriosityQLearning",|m,i,o|Box::new(crate::agent::RecurrentCuriosityQLearning::new(m,i,o))),
            ("DuelingCuriosityQLearning",|m,i,o|Box::new(crate::agent::DuelingCuriosityQLearning::new(m,i,o))),
            ("NeuroEvolution", |_,i,o| Box::new(crate::agent::NeuroEvolution::new(20,0.05,i,o,|a,b|Box::new(crate::model::MLP::new(&[a,64,32,b]))))),
            ("NeuroEvolutionNoveltySearch",|_,i,o|Box::new(crate::agent::NeuroEvolutionNoveltySearch::new(20,0.05,i,o,|a,b|Box::new(crate::model::MLP::new(&[a,64,32,b]))))),
            ("NESAgent", |m,_,_| Box::new(crate::agent::NESAgent::new(m,0.1,0.01))),
            ("EvolutionStrategyBayesian",|m,_,_|Box::new(crate::agent::EvolutionStrategyBayesian::new(m,10,0.1,0.03))),
            ("DNBAgent", |m,_,_| Box::new(crate::agent::DNBAgent::new(m))),
            ("TurtleAgent", |_,_,_| Box::new(crate::agent::TurtleAgent::new())),
            ("SignalRollingAgent",|_,_,_|Box::new(crate::agent::SignalRollingAgent::new(0.005))),
            ("MovingAverageAgent",|_,_,_|Box::new(crate::agent::MovingAverageAgent::new(5,20))),
            ("ABCDStrategyAgent",|_,_,_|Box::new(crate::agent::ABCDStrategyAgent::new())),
            ("Agent1", |_,_,_| Box::new(crate::agent::Agent1::new())),
        ];

        let out_dim = action_dim();

        // Build all combos as index triples, then shuffle
        let mut combo_indices: Vec<(usize, usize, usize)> = Vec::new();
        for ai in 0..agents.len() {
            for fi in 0..extractors.len() {
                for mi in 0..models.len() {
                    combo_indices.push((ai, fi, mi));
                }
            }
        }
        use rand::seq::SliceRandom;
        combo_indices.shuffle(&mut rand::thread_rng());

        // Pre-create all training run entries
        {
            let mut s = state_clone.lock().unwrap();
            for &(ai, fi, mi) in &combo_indices {
                s.runs.push(TrainingRun {
                    agent_name: agents[ai].0.to_string(),
                    feature_name: extractors[fi].0.to_string(),
                    model_name: models[mi].0.to_string(),
                    episode: 0, reward: 0.0, pnl: 10000.0,
                    sharpe: 0.0, completed: false,
                    equity_curve: Vec::new(),
                    ep_rewards: Vec::new(),
                });
            }
        }

        let total_combos = combo_indices.len();
        let batch_size = 4;

        for batch_start in (0..total_combos).step_by(batch_size) {
            if !{ let s = state_clone.lock().unwrap(); s.training } { break; }
            let batch_end = (batch_start + batch_size).min(total_combos);
            let mut handles = Vec::new();

            for ci in batch_start..batch_end {
                let state_ref = Arc::clone(&state_clone);
                let p = prices.clone();
                let (ai, fi, mi) = combo_indices[ci];
                let (aname, af) = &agents[ai];
                let (fname, ext_factory) = &extractors[fi];
                let (mname, mf) = &models[mi];

                let mut ext = (ext_factory)();
                let inp_dim = ext.dim();
                let vol: Vec<f64> = p.iter().map(|&v| v * 100.0).collect();
                let model = mf(inp_dim, out_dim);
                let mut agent = af(model, inp_dim, out_dim);
                let mut env = BloxEnv::new(p.clone(), vol, 10000.0);
                let mut all_rewards = Vec::new();
                let mut equity = Vec::new();

                let an = aname.to_string();
                let fn2 = fname.to_string();
                let mn = mname.to_string();

                let handle = thread::spawn(move || {
                    for ep in 0..200 {
                        env.reset();
                        let mut ep_rewards = Vec::new();

                        for step in 0..env.max_steps - 1 {
                            let obs = env.observe();
                            let state = ext.extract(&obs);
                            let mut action = agent.act(&state);
                            // force exploration: 10% random actions
                            if rand::random::<f64>() < 0.1 {
                                let r = rand::random::<f64>();
                                action = if r < 0.5 {
                                    crate::action::Action::market_buy((rand::random::<f64>() * 10.0) as u64 + 1)
                                } else {
                                    crate::action::Action::sell(0, (rand::random::<f64>() * 10.0) as u64 + 1)
                                };
                            }
                            let reward = env.step(&action);
                            ep_rewards.push(reward);
                            if step % 10 == 0 {
                                equity.push(env.pnl());
                            }
                        }

                        let ep_r: f64 = ep_rewards.iter().sum();
                        all_rewards.push(ep_r);
                        agent.train_episode(&ep_rewards);

                        let mean = all_rewards.iter().sum::<f64>() / all_rewards.len() as f64;
                        let std = (all_rewards.iter().map(|r| (r - mean).powi(2)).sum::<f64>()
                            / all_rewards.len() as f64).sqrt().max(1e-8);
                        let sharpe = mean / std * (all_rewards.len() as f64).sqrt();

                        let mut s = state_ref.lock().unwrap();
                        if let Some(run) = s.runs.get_mut(ci) {
                            run.episode = ep;
                            run.reward = ep_r;
                            run.pnl = env.pnl();
                            run.sharpe = sharpe;
                            run.equity_curve = equity.clone();
                            run.ep_rewards = ep_rewards.clone();
                        }
                    }

                    let pnl = env.pnl();
                    let mean = all_rewards.iter().sum::<f64>() / all_rewards.len() as f64;
                    let std = (all_rewards.iter().map(|r| (r - mean).powi(2)).sum::<f64>()
                        / all_rewards.len() as f64).sqrt().max(1e-8);
                    let sharpe = mean / std * (all_rewards.len() as f64).sqrt();
                    let w: f64 = all_rewards.iter().filter(|&&r| r > 0.0).count() as f64
                        / all_rewards.len() as f64;

                    let mut s = state_ref.lock().unwrap();
                    if let Some(run) = s.runs.get_mut(ci) { run.completed = true; }
                    s.results.push((an, fn2, mn,
                        Metrics { episodes: 200, total_pnl: pnl - 10000.0, sharpe,
                            max_drawdown: 0.0, win_rate: w, trade_count: all_rewards.len() }));
                });
                handles.push(handle);
            }
            for h in handles { h.join().unwrap_or(()); }
        }
        { let mut s = state_clone.lock().unwrap(); s.training = false; }
    });
}
