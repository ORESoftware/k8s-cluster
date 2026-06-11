//! A small multilayer perceptron trained by backpropagation.
//!
//! This is the *numeric* learner with *analytic gradients*: the chain rule is
//! applied exactly through each layer and the parameters are stepped with Adam.
//! The network always operates on standardised inputs/target (see `data.rs`);
//! the caller owns the scalers and inverts them for prediction. A single scalar
//! output makes it a regression head.

// Dense layer math indexes parallel weight/activation arrays by position; the
// range loops are the natural, clearest idiom here.
#![allow(clippy::needless_range_loop)]

use std::time::Instant;

use serde::Serialize;

use crate::rng::Rng;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Activation {
    Tanh,
    Relu,
    Sigmoid,
}

impl Activation {
    pub fn parse(name: &str) -> Activation {
        match name.trim().to_ascii_lowercase().as_str() {
            "relu" => Activation::Relu,
            "sigmoid" | "logistic" => Activation::Sigmoid,
            _ => Activation::Tanh,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Activation::Tanh => "tanh",
            Activation::Relu => "relu",
            Activation::Sigmoid => "sigmoid",
        }
    }

    fn apply(self, z: f64) -> f64 {
        match self {
            Activation::Tanh => z.tanh(),
            Activation::Relu => z.max(0.0),
            Activation::Sigmoid => 1.0 / (1.0 + (-z).exp()),
        }
    }

    /// Derivative dactivation/dz expressed from the pre-activation `z`.
    fn derivative(self, z: f64) -> f64 {
        match self {
            Activation::Tanh => {
                let t = z.tanh();
                1.0 - t * t
            }
            Activation::Relu => {
                if z > 0.0 {
                    1.0
                } else {
                    0.0
                }
            }
            Activation::Sigmoid => {
                let s = 1.0 / (1.0 + (-z).exp());
                s * (1.0 - s)
            }
        }
    }
}

/// One fully-connected layer; `weights` is `out × in`, `bias` is `out`.
#[derive(Clone, Serialize)]
pub struct Layer {
    pub weights: Vec<Vec<f64>>,
    pub bias: Vec<f64>,
}

impl Layer {
    fn zeros(inputs: usize, outputs: usize) -> Layer {
        Layer {
            weights: vec![vec![0.0; inputs]; outputs],
            bias: vec![0.0; outputs],
        }
    }
}

#[derive(Clone)]
pub struct Mlp {
    pub layers: Vec<Layer>,
    pub activation: Activation,
}

impl Mlp {
    /// Build a fresh network `d → hidden... → 1` with Glorot-uniform weights.
    pub fn new(d: usize, hidden: &[usize], activation: Activation, rng: &mut Rng) -> Mlp {
        let mut sizes = vec![d];
        sizes.extend_from_slice(hidden);
        sizes.push(1);
        let mut layers = Vec::with_capacity(sizes.len() - 1);
        for w in sizes.windows(2) {
            let (fan_in, fan_out) = (w[0], w[1]);
            let limit = (6.0 / (fan_in + fan_out).max(1) as f64).sqrt();
            let mut layer = Layer::zeros(fan_in, fan_out);
            for o in 0..fan_out {
                for i in 0..fan_in {
                    layer.weights[o][i] = rng.range(-limit, limit);
                }
            }
            layers.push(layer);
        }
        Mlp { layers, activation }
    }

    /// Number of trainable parameters (weights + biases) across all layers.
    pub fn parameter_count(&self) -> usize {
        self.layers
            .iter()
            .map(|l| l.bias.len() + l.weights.iter().map(|r| r.len()).sum::<usize>())
            .sum()
    }

    /// Flatten every parameter into one vector (weights row-major, then bias,
    /// per layer). Pairs with `set_flat` for the evolution-strategy learner.
    pub fn to_flat(&self) -> Vec<f64> {
        let mut out = Vec::with_capacity(self.parameter_count());
        for layer in &self.layers {
            for row in &layer.weights {
                out.extend_from_slice(row);
            }
            out.extend_from_slice(&layer.bias);
        }
        out
    }

    /// Overwrite every parameter from a flat vector produced by `to_flat`.
    pub fn set_flat(&mut self, flat: &[f64]) {
        let mut k = 0;
        for layer in &mut self.layers {
            for row in &mut layer.weights {
                for w in row.iter_mut() {
                    *w = flat.get(k).copied().unwrap_or(0.0);
                    k += 1;
                }
            }
            for b in layer.bias.iter_mut() {
                *b = flat.get(k).copied().unwrap_or(0.0);
                k += 1;
            }
        }
    }

    /// Forward pass on a standardised input, returning the scalar output.
    pub fn forward(&self, x: &[f64]) -> f64 {
        let mut activation = x.to_vec();
        let last = self.layers.len().saturating_sub(1);
        for (idx, layer) in self.layers.iter().enumerate() {
            let mut next = vec![0.0; layer.bias.len()];
            for o in 0..layer.bias.len() {
                let mut z = layer.bias[o];
                for i in 0..activation.len() {
                    z += layer.weights[o][i] * activation[i];
                }
                next[o] = if idx == last {
                    z // linear regression head
                } else {
                    self.activation.apply(z)
                };
            }
            activation = next;
        }
        activation.first().copied().unwrap_or(0.0)
    }

    /// One backprop pass over a batch, returning summed parameter gradients in
    /// the same layout as `to_flat`, plus the summed squared error. Loss is
    /// `(pred - target)²`; gradients use the exact derivative `2·(pred-target)`.
    fn batch_gradient(&self, xs: &[Vec<f64>], ys: &[f64]) -> (Vec<f64>, f64) {
        let n_params = self.parameter_count();
        let mut grad = vec![0.0; n_params];
        let mut sse = 0.0;
        let last = self.layers.len().saturating_sub(1);

        for (x, &target) in xs.iter().zip(ys.iter()) {
            // Forward pass, caching pre-activations (z) and activations (a).
            let mut activations: Vec<Vec<f64>> = vec![x.clone()];
            let mut pre: Vec<Vec<f64>> = Vec::with_capacity(self.layers.len());
            for (idx, layer) in self.layers.iter().enumerate() {
                let prev = activations.last().unwrap();
                let mut z = vec![0.0; layer.bias.len()];
                let mut a = vec![0.0; layer.bias.len()];
                for o in 0..layer.bias.len() {
                    let mut acc = layer.bias[o];
                    for i in 0..prev.len() {
                        acc += layer.weights[o][i] * prev[i];
                    }
                    z[o] = acc;
                    a[o] = if idx == last {
                        acc
                    } else {
                        self.activation.apply(acc)
                    };
                }
                pre.push(z);
                activations.push(a);
            }

            let pred = activations.last().unwrap()[0];
            let err = pred - target;
            sse += err * err;

            // Backward pass. delta = dL/dz for each layer.
            let mut delta = vec![2.0 * err]; // output layer (linear): dL/dz = 2(pred-t)
            let mut k_end = n_params;
            for idx in (0..self.layers.len()).rev() {
                let layer = &self.layers[idx];
                let prev = &activations[idx];
                let outs = layer.bias.len();
                let ins = prev.len();
                // Accumulate gradient for this layer (weights then bias),
                // indexing into the flat grad vector at the layer's slot.
                let layer_params = outs * ins + outs;
                let base = k_end - layer_params;
                for o in 0..outs {
                    let d = delta[o];
                    let wbase = base + o * ins;
                    for i in 0..ins {
                        grad[wbase + i] += d * prev[i];
                    }
                    grad[base + outs * ins + o] += d;
                }
                k_end = base;

                // Propagate delta to the previous layer (skip when at input).
                if idx > 0 {
                    let prev_z = &pre[idx - 1];
                    let mut new_delta = vec![0.0; ins];
                    for i in 0..ins {
                        let mut acc = 0.0;
                        for o in 0..outs {
                            acc += layer.weights[o][i] * delta[o];
                        }
                        new_delta[i] = acc * self.activation.derivative(prev_z[i]);
                    }
                    delta = new_delta;
                }
            }
        }
        (grad, sse)
    }

    /// Train in place with mini-batch Adam, keeping the parameters that gave the
    /// best validation MSE (early-stopping style). Returns the number of epochs
    /// actually run. Inputs/targets must already be standardised.
    #[allow(clippy::too_many_arguments)]
    pub fn train(
        &mut self,
        train_x: &[Vec<f64>],
        train_y: &[f64],
        val_x: &[Vec<f64>],
        val_y: &[f64],
        epochs: usize,
        learning_rate: f64,
        batch_size: usize,
        rng: &mut Rng,
        deadline: Instant,
    ) -> usize {
        let n = train_x.len();
        if n == 0 {
            return 0;
        }
        let n_params = self.parameter_count();
        let mut m = vec![0.0; n_params];
        let mut v = vec![0.0; n_params];
        let (beta1, beta2, eps): (f64, f64, f64) = (0.9, 0.999, 1e-8);
        let batch = batch_size.clamp(1, n);

        let mut best_params = self.to_flat();
        let mut best_val = f64::INFINITY;
        let mut order: Vec<usize> = (0..n).collect();
        let mut t = 0u64;

        let mut ran = 0;
        for epoch in 0..epochs {
            // Cooperative time budget: stop cleanly and keep the best weights.
            if Instant::now() >= deadline {
                break;
            }
            // Shuffle the row order each epoch for stochastic batches.
            for i in (1..n).rev() {
                order.swap(i, rng.below(i + 1));
            }
            for chunk in order.chunks(batch) {
                let xs: Vec<Vec<f64>> = chunk.iter().map(|&i| train_x[i].clone()).collect();
                let ys: Vec<f64> = chunk.iter().map(|&i| train_y[i]).collect();
                let (grad, _) = self.batch_gradient(&xs, &ys);
                let scale = 1.0 / chunk.len() as f64;
                t += 1;
                // Cap the exponent so the bias correction can never overflow i32,
                // regardless of epoch/batch counts (beta^big is ~0 anyway).
                let exp = t.min(1_000_000) as i32;
                let bc1 = 1.0 - beta1.powi(exp);
                let bc2 = 1.0 - beta2.powi(exp);
                let mut flat = self.to_flat();
                for p in 0..n_params {
                    let g = grad[p] * scale;
                    m[p] = beta1 * m[p] + (1.0 - beta1) * g;
                    v[p] = beta2 * v[p] + (1.0 - beta2) * g * g;
                    let m_hat = m[p] / bc1;
                    let v_hat = v[p] / bc2;
                    flat[p] -= learning_rate * m_hat / (v_hat.sqrt() + eps);
                }
                self.set_flat(&flat);
            }

            // Track the best parameters by validation MSE.
            let val_mse = self.mse(val_x, val_y);
            if val_mse < best_val {
                best_val = val_mse;
                best_params = self.to_flat();
            }
            ran = epoch + 1;
            // Cheap divergence guard: bail if the net blew up.
            if !val_mse.is_finite() && epoch > 2 {
                break;
            }
        }
        self.set_flat(&best_params);
        ran
    }

    /// Mean squared error over a standardised set (used for early stopping).
    pub fn mse(&self, xs: &[Vec<f64>], ys: &[f64]) -> f64 {
        if xs.is_empty() {
            return f64::INFINITY;
        }
        let mut sse = 0.0;
        for (x, &t) in xs.iter().zip(ys.iter()) {
            let e = self.forward(x) - t;
            sse += e * e;
        }
        sse / xs.len() as f64
    }
}
