//! Neural network models used as the backbone of agents.

pub fn xavier_init(size: usize) -> Vec<f64> {
    let limit = if size > 0 { (6.0 / size as f64).sqrt() } else { 1.0 };
    (0..size).map(|_| (rand::random::<f64>() * 2.0 - 1.0) * limit).collect()
}

pub trait Model: Send + Sync {
    fn forward(&mut self, input: &[f64]) -> Vec<f64>;
    fn backward(&mut self, input: &[f64], grads: &[f64]) -> Vec<f64>;
    fn update(&mut self, learning_rate: f64);
    fn reset(&mut self);
    fn name(&self) -> &'static str;
    fn parameters(&self) -> Vec<f64>;
    fn set_parameters(&mut self, params: &[f64]);
    fn parameter_len(&self) -> usize;
}

fn linear(weights: &[f64], input: &[f64], out: usize) -> Vec<f64> {
    let mut result = vec![0.0; out];
    for i in 0..out {
        let mut sum = 0.0;
        for (j, &x) in input.iter().enumerate() {
            sum += x * weights[i * input.len() + j];
        }
        result[i] = sum;
    }
    result
}

fn hash_int(x: usize) -> usize {
    let mut k = x;
    k ^= k >> 33;
    k = k.wrapping_mul(0xff51afd7ed558ccd);
    k ^= k >> 33;
    k = k.wrapping_mul(0xc4ceb9fe1a85ec53);
    k ^= k >> 33;
    (k & 0x1_ffff) as usize
}

// ─── MLP ────────────────────────────────────────────────────────────────────────

pub struct MLP {
    pub size: Vec<usize>,
    pub weights: Vec<Vec<f64>>,
    pub biases: Vec<Vec<f64>>,
}

impl MLP {
    pub fn new(size: &[usize]) -> Self {
        let mut weights = Vec::new();
        let mut biases = Vec::new();
        for w in size.windows(2) {
            weights.push(xavier_init(w[0] * w[1]));
            biases.push(vec![0.0; w[1]]);
        }
        MLP { size: size.to_vec(), weights, biases }
    }
}

impl Model for MLP {
    fn forward(&mut self, input: &[f64]) -> Vec<f64> {
        let mut act = input.to_vec();
        let last = self.size.len() - 1;
        for (i, w) in self.weights.iter().enumerate() {
            let next = linear(w, &act, self.size[i + 1]);
            act = if i == last {
                next
            } else {
                next.iter().map(|x| x.max(0.0)).collect()
            };
        }
        act
    }

    fn backward(&mut self, input: &[f64], grads: &[f64]) -> Vec<f64> {
        // Copy-gradient placeholder; sufficient for episode-level perturbations.
        grads.to_vec()
    }

    fn update(&mut self, learning_rate: f64) {
        for w in self.weights.iter_mut() {
            for v in w.iter_mut() {
                *v += (rand::random::<f64>() * 2.0 - 1.0) * learning_rate;
            }
        }
    }

    fn reset(&mut self) {
        for (i, w) in self.weights.iter_mut().enumerate() {
            *w = xavier_init(self.size[i] * self.size[i + 1]);
        }
        for b in self.biases.iter_mut() {
            b.fill(0.0);
        }
    }

    fn name(&self) -> &'static str {
        "MLP"
    }

    fn parameters(&self) -> Vec<f64> {
        let mut p = Vec::new();
        for w in &self.weights {
            p.extend_from_slice(w);
        }
        for b in &self.biases {
            p.extend_from_slice(b);
        }
        p
    }

    fn set_parameters(&mut self, params: &[f64]) {
        let mut idx = 0;
        for w in self.weights.iter_mut() {
            let n = w.len();
            w.copy_from_slice(&params[idx..idx + n]);
            idx += n;
        }
        for b in self.biases.iter_mut() {
            let n = b.len();
            b.copy_from_slice(&params[idx..idx + n]);
            idx += n;
        }
    }

    fn parameter_len(&self) -> usize {
        self.weights.iter().map(|w| w.len()).sum::<usize>() + self.biases.iter().map(|b| b.len()).sum::<usize>()
    }
}

// ─── Vanilla (single linear layer) ──────────────────────────────────────────────

pub struct Vanilla {
    pub input_size: usize,
    pub output_size: usize,
    pub weights: Vec<f64>,
    pub biases: Vec<f64>,
}

impl Vanilla {
    pub fn new(input_size: usize, output_size: usize) -> Self {
        Vanilla {
            input_size,
            output_size,
            weights: xavier_init(input_size * output_size),
            biases: vec![0.0; output_size],
        }
    }
}

impl Model for Vanilla {
    fn forward(&mut self, input: &[f64]) -> Vec<f64> {
        let mut out = linear(&self.weights, input, self.output_size);
        for i in 0..self.output_size {
            out[i] += self.biases[i];
        }
        out
    }

    fn backward(&mut self, _input: &[f64], grads: &[f64]) -> Vec<f64> {
        grads.to_vec()
    }

    fn update(&mut self, learning_rate: f64) {
        for v in self.weights.iter_mut() {
            *v += (rand::random::<f64>() * 2.0 - 1.0) * learning_rate;
        }
    }

    fn reset(&mut self) {
        self.weights = xavier_init(self.input_size * self.output_size);
    }

    fn name(&self) -> &'static str {
        "Vanilla"
    }

    fn parameters(&self) -> Vec<f64> {
        let mut p = self.weights.clone();
        p.extend_from_slice(&self.biases);
        p
    }

    fn set_parameters(&mut self, params: &[f64]) {
        let n = self.weights.len();
        self.weights.copy_from_slice(&params[..n]);
        self.biases.copy_from_slice(&params[n..]);
    }

    fn parameter_len(&self) -> usize {
        self.weights.len() + self.biases.len()
    }
}

// ─── LSTM ───────────────────────────────────────────────────────────────────────

pub struct LSTM {
    pub w_ih: Vec<f64>,
    pub w_hh: Vec<f64>,
    pub b: Vec<f64>,
    pub input_size: usize,
    pub hidden_size: usize,
    pub h: Vec<f64>,
    pub c: Vec<f64>,
}

impl LSTM {
    pub fn new(input_size: usize, hidden_size: usize) -> Self {
        let gates = 4;
        LSTM {
            w_ih: xavier_init(input_size * hidden_size * gates),
            w_hh: xavier_init(hidden_size * hidden_size * gates),
            b: vec![0.0; hidden_size * gates],
            input_size,
            hidden_size,
            h: vec![0.0; hidden_size],
            c: vec![0.0; hidden_size],
        }
    }

    pub fn reset_state(&mut self) {
        self.h.fill(0.0);
        self.c.fill(0.0);
    }
}

impl Model for LSTM {
    fn forward(&mut self, input: &[f64]) -> Vec<f64> {
        let n = self.hidden_size;
        let g = 4 * n;
        // Use input.len() to index gates (parameters sized for the actual input).
        let seg = self.w_ih.len() / if input.len() > 0 { input.len() } else { 1 };
        let mut thetas = vec![0.0; g];
        for i in 0..g {
            let mut sum = 0.0;
            for (j, &x) in input.iter().enumerate() {
                sum += x * self.w_ih[i * seg + j];
            }
            thetas[i] = sum;
        }
        for k in 0..n {
            for l in 0..n {
                thetas[k] += self.w_hh[k * n + l] * self.h[l];
                thetas[n + k] += self.w_hh[(n + k) * n + l] * self.h[l];
                thetas[2 * n + k] += self.w_hh[(2 * n + k) * n + l] * self.h[l];
                thetas[3 * n + k] += self.w_hh[(3 * n + k) * n + l] * self.h[l];
            }
        }
        for i in 0..g {
            thetas[i] += self.b[i];
        }
        let mut new_h = vec![0.0; n];
        let mut new_c = vec![0.0; n];
        for k in 0..n {
            let i = sigmoid(thetas[k]);
            let f = sigmoid(thetas[n + k]);
            let og = sigmoid(thetas[2 * n + k]);
            let gg = thetas[3 * n + k].tanh();
            new_c[k] = f * self.c[k] + i * gg;
            new_h[k] = og * new_c[k].tanh();
        }
        let h = new_h.clone();
        self.h = new_h;
        self.c = new_c;
        h
    }

    fn backward(&mut self, _input: &[f64], grads: &[f64]) -> Vec<f64> {
        grads.to_vec()
    }

    fn update(&mut self, learning_rate: f64) {
        for w in self.w_ih.iter_mut() {
            *w += (rand::random::<f64>() * 2.0 - 1.0) * learning_rate;
        }
    }

    fn reset(&mut self) {
        self.reset_state();
        self.w_ih = xavier_init(self.input_size * self.hidden_size * 4);
    }

    fn name(&self) -> &'static str {
        "LSTM"
    }

    fn parameters(&self) -> Vec<f64> {
        let mut p = self.w_ih.clone();
        p.extend_from_slice(&self.w_hh);
        p.extend_from_slice(&self.b);
        p
    }

    fn set_parameters(&mut self, params: &[f64]) {
        let n1 = self.w_ih.len();
        let n2 = self.w_hh.len();
        self.w_ih.copy_from_slice(&params[..n1]);
        self.w_hh.copy_from_slice(&params[n1..n1 + n2]);
        self.b.copy_from_slice(&params[n1 + n2..]);
    }

    fn parameter_len(&self) -> usize {
        self.w_ih.len() + self.w_hh.len() + self.b.len()
    }
}

// ─── GRU ────────────────────────────────────────────────────────────────────────

pub struct GRU {
    pub w_ih: Vec<f64>,
    pub w_hh: Vec<f64>,
    pub b: Vec<f64>,
    pub input_size: usize,
    pub hidden_size: usize,
    pub h: Vec<f64>,
}

impl GRU {
    pub fn new(input_size: usize, hidden_size: usize) -> Self {
        GRU {
            w_ih: xavier_init(input_size * hidden_size * 3),
            w_hh: xavier_init(hidden_size * hidden_size * 3),
            b: vec![0.0; hidden_size * 3],
            input_size,
            hidden_size,
            h: vec![0.0; hidden_size],
        }
    }

    pub fn reset_state(&mut self) {
        self.h.fill(0.0);
    }
}

impl Model for GRU {
    fn forward(&mut self, input: &[f64]) -> Vec<f64> {
        let n = self.hidden_size;
        let seg = self.w_ih.len() / if input.len() > 0 { input.len() } else { 1 };
        let mut z = vec![0.0; n];
        let mut r = vec![0.0; n];
        let mut g = vec![0.0; n];
        for k in 0..n {
            let mut sz = 0.0;
            let mut sr = 0.0;
            let mut sg = 0.0;
            for (j, &x) in input.iter().enumerate() {
                sz += x * self.w_ih[k * seg + j];
                sr += x * self.w_ih[(n + k) * seg + j];
                sg += x * self.w_ih[(2 * n + k) * seg + j];
            }
            for l in 0..n {
                sz += self.w_hh[k * n + l] * self.h[l];
                sr += self.w_hh[(n + k) * n + l] * self.h[l];
                sg += self.w_hh[(2 * n + k) * n + l] * self.h[l];
            }
            z[k] = sigmoid(sz + self.b[k]);
            r[k] = sigmoid(sr + self.b[n + k]);
            g[k] = sg + self.b[2 * n + k];
        }
        let mut new_h = vec![0.0; n];
        for k in 0..n {
            let candidate = r[k] * self.h[k] + g[k];
            new_h[k] = z[k] * self.h[k] + (1.0 - z[k]) * candidate.tanh();
        }
        let h = new_h.clone();
        self.h = new_h;
        h
    }

    fn backward(&mut self, _input: &[f64], grads: &[f64]) -> Vec<f64> {
        grads.to_vec()
    }

    fn update(&mut self, learning_rate: f64) {
        for w in self.w_ih.iter_mut() {
            *w += (rand::random::<f64>() * 2.0 - 1.0) * learning_rate;
        }
    }

    fn reset(&mut self) {
        self.reset_state();
        self.w_ih = xavier_init(self.input_size * self.hidden_size * 3);
    }

    fn name(&self) -> &'static str {
        "GRU"
    }

    fn parameters(&self) -> Vec<f64> {
        let mut p = self.w_ih.clone();
        p.extend_from_slice(&self.w_hh);
        p.extend_from_slice(&self.b);
        p
    }

    fn set_parameters(&mut self, params: &[f64]) {
        let n1 = self.w_ih.len();
        let n2 = self.w_hh.len();
        self.w_ih.copy_from_slice(&params[..n1]);
        self.w_hh.copy_from_slice(&params[n1..n1 + n2]);
        self.b.copy_from_slice(&params[n1 + n2..]);
    }

    fn parameter_len(&self) -> usize {
        self.w_ih.len() + self.w_hh.len() + self.b.len()
    }
}

// ─── Attention ──────────────────────────────────────────────────────────────────

pub struct Attention {
    pub w_q: Vec<f64>,
    pub w_k: Vec<f64>,
    pub w_v: Vec<f64>,
    pub dim: usize,
    pub out: Vec<f64>,
}

impl Attention {
    pub fn new(input_size: usize) -> Self {
        Attention {
            w_q: xavier_init(input_size * input_size),
            w_k: xavier_init(input_size * input_size),
            w_v: xavier_init(input_size * input_size),
            dim: input_size,
            out: vec![0.0; input_size],
        }
    }
}

impl Model for Attention {
    fn forward(&mut self, input: &[f64]) -> Vec<f64> {
        let d = self.dim;
        let q: Vec<f64> = (0..d).map(|i| (0..d).map(|j| input[j] * self.w_q[i * d + j]).sum()).collect();
        let k: Vec<f64> = (0..d).map(|i| (0..d).map(|j| input[j] * self.w_k[i * d + j]).sum()).collect();
        let v: Vec<f64> = (0..d).map(|i| (0..d).map(|j| input[j] * self.w_v[i * d + j]).sum()).collect();
        let mut scale = 1.0 / (d as f64).sqrt();
        if !scale.is_finite() || scale <= 0.0 {
            scale = 1.0;
        }
        let mut scores = vec![0.0; d];
        for i in 0..d {
            let mut s = 0.0;
            for j in 0..d {
                s += q[i] * k[j] * scale;
            }
            scores[i] = s;
        }
        let maxs = scores.iter().cloned().fold(f64::MIN, f64::max);
        let mut exps: Vec<f64> = scores.iter().map(|&s| (s - maxs).exp()).collect();
        let sum: f64 = exps.iter().sum();
        for e in exps.iter_mut() {
            *e /= if sum > 0.0 { sum } else { 1.0 };
        }
        let mut out = vec![0.0; d];
        for i in 0..d {
            for j in 0..d {
                out[i] += exps[j] * v[i];
            }
        }
        let r = out.clone();
        self.out = out;
        r
    }

    fn backward(&mut self, _input: &[f64], grads: &[f64]) -> Vec<f64> {
        grads.to_vec()
    }

    fn update(&mut self, learning_rate: f64) {
        for w in self.w_q.iter_mut() {
            *w += (rand::random::<f64>() * 2.0 - 1.0) * learning_rate;
        }
    }

    fn reset(&mut self) {
        self.w_q = xavier_init(self.dim * self.dim);
        self.w_k = xavier_init(self.dim * self.dim);
        self.w_v = xavier_init(self.dim * self.dim);
    }

    fn name(&self) -> &'static str {
        "Attention"
    }

    fn parameters(&self) -> Vec<f64> {
        let mut p = self.w_q.clone();
        p.extend_from_slice(&self.w_k);
        p.extend_from_slice(&self.w_v);
        p
    }

    fn set_parameters(&mut self, params: &[f64]) {
        let n1 = self.w_q.len();
        let n2 = self.w_k.len();
        self.w_q.copy_from_slice(&params[..n1]);
        self.w_k.copy_from_slice(&params[n1..n1 + n2]);
        self.w_v.copy_from_slice(&params[n1 + n2..]);
    }

    fn parameter_len(&self) -> usize {
        self.w_q.len() + self.w_k.len() + self.w_v.len()
    }
}

// ─── DNC ────────────────────────────────────────────────────────────────────────

pub struct DNC {
    pub controller_w: Vec<f64>,
    pub memory: Vec<f64>,
    pub num_slots: usize,
    pub slot_size: usize,
    pub num_heads: usize,
    pub input_size: usize,
    pub output_size: usize,
}

impl DNC {
    pub fn new(input_size: usize, num_slots: usize, output_size: usize, num_heads: usize) -> Self {
        let slot_size = input_size.max(4);
        DNC {
            controller_w: xavier_init(input_size * output_size),
            memory: vec![0.0; num_slots * slot_size],
            num_slots,
            slot_size,
            num_heads,
            input_size,
            output_size,
        }
    }
}

impl Model for DNC {
    fn forward(&mut self, input: &[f64]) -> Vec<f64> {
        // Read from memory, combine with the controller, write back.
        let mut read = vec![0.0; self.slot_size];
        let slots = self.num_slots;
        if self.memory.len() >= slots * self.slot_size && self.slot_size > 0 {
            let mut idx = hash_int(input.len());
            for s in 0..slots {
                let base = s * self.slot_size;
                for k in 0..self.slot_size {
                    read[k] += self.memory[base + k];
                }
                idx = hash_int(idx + s);
                if idx % 7 == 0 {
                    let target = idx % slots;
                    let tbase = target * self.slot_size;
                    for k in 0..self.slot_size {
                        self.memory[tbase + k] = input[k % input.len()];
                    }
                }
            }
            for k in 0..self.slot_size {
                read[k] /= slots as f64;
            }
        }
        let ctrl = linear(&self.controller_w, input, self.output_size);
        let mut out = vec![0.0; self.output_size];
        for i in 0..self.output_size {
            out[i] = ctrl[i] + if i < read.len() { read[i] } else { 0.0 };
        }
        out
    }

    fn backward(&mut self, _input: &[f64], grads: &[f64]) -> Vec<f64> {
        grads.to_vec()
    }

    fn update(&mut self, learning_rate: f64) {
        for w in self.controller_w.iter_mut() {
            *w += (rand::random::<f64>() * 2.0 - 1.0) * learning_rate;
        }
    }

    fn reset(&mut self) {
        self.memory.fill(0.0);
        self.controller_w = xavier_init(self.input_size * self.output_size);
    }

    fn name(&self) -> &'static str {
        "DNC"
    }

    fn parameters(&self) -> Vec<f64> {
        let mut p = self.controller_w.clone();
        p.extend_from_slice(&self.memory);
        p
    }

    fn set_parameters(&mut self, params: &[f64]) {
        let n = self.controller_w.len();
        self.controller_w.copy_from_slice(&params[..n]);
        self.memory.copy_from_slice(&params[n..]);
    }

    fn parameter_len(&self) -> usize {
        self.controller_w.len() + self.memory.len()
    }
}

fn sigmoid(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}