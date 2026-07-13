//! Dense state-vector quantum simulator.
//!
//! A register of `n` qubits is a unit vector of `2^n` complex amplitudes. Qubit
//! `q` corresponds to bit `q` of the basis-state index (little-endian: qubit 0
//! is the least-significant bit). Gates are applied in place by pairing the
//! amplitude indices that differ only in the affected bit(s), which is the
//! standard O(2^n)-per-gate kernel.

use crate::complex::Complex;
use crate::rng::Rng;

/// A single-qubit unitary, row-major: `[[a, b], [c, d]]`.
pub type Matrix2 = [[Complex; 2]; 2];

#[derive(Clone, Debug)]
pub struct State {
    pub n: usize,
    pub amps: Vec<Complex>,
}

impl State {
    /// The computational-basis state |0…0⟩.
    pub fn zero(n: usize) -> Self {
        let mut amps = vec![Complex::ZERO; 1usize << n];
        amps[0] = Complex::ONE;
        State { n, amps }
    }

    pub fn dim(&self) -> usize {
        self.amps.len()
    }

    /// Apply a single-qubit gate `m` to `target`.
    pub fn apply_1q(&mut self, target: usize, m: &Matrix2) {
        let bit = 1usize << target;
        let mut i = 0usize;
        while i < self.amps.len() {
            if i & bit == 0 {
                let j = i | bit;
                let a = self.amps[i];
                let b = self.amps[j];
                self.amps[i] = m[0][0] * a + m[0][1] * b;
                self.amps[j] = m[1][0] * a + m[1][1] * b;
            }
            i += 1;
        }
    }

    /// Apply `m` to `target`, conditioned on every qubit in `controls` being |1⟩.
    /// `controls` must not contain `target`.
    pub fn apply_controlled_1q(&mut self, controls: &[usize], target: usize, m: &Matrix2) {
        if controls.is_empty() {
            self.apply_1q(target, m);
            return;
        }
        let bit = 1usize << target;
        let cmask = controls.iter().fold(0usize, |acc, &c| acc | (1usize << c));
        let mut i = 0usize;
        while i < self.amps.len() {
            // Both i and j = i|bit share the same control bits (controls != target),
            // so testing the mask once on the bit-0 partner is sufficient.
            if i & bit == 0 && (i & cmask) == cmask {
                let j = i | bit;
                let a = self.amps[i];
                let b = self.amps[j];
                self.amps[i] = m[0][0] * a + m[0][1] * b;
                self.amps[j] = m[1][0] * a + m[1][1] * b;
            }
            i += 1;
        }
    }

    /// Swap two qubits' amplitudes.
    pub fn swap(&mut self, a: usize, b: usize) {
        if a == b {
            return;
        }
        let (ba, bb) = (1usize << a, 1usize << b);
        let mut i = 0usize;
        while i < self.amps.len() {
            // Visit each differing pair once: bit a clear, bit b set.
            if i & ba == 0 && i & bb != 0 {
                let j = (i | ba) & !bb;
                self.amps.swap(i, j);
            }
            i += 1;
        }
    }

    /// Flip the sign of every marked basis state's amplitude (a phase oracle).
    pub fn phase_flip(&mut self, marked: &[usize]) {
        for &idx in marked {
            if idx < self.amps.len() {
                self.amps[idx] = -self.amps[idx];
            }
        }
    }

    /// Grover diffusion: reflect every amplitude about their mean
    /// (`2|s⟩⟨s| − I`, the inversion-about-the-average operator).
    pub fn reflect_about_mean(&mut self) {
        let n = self.amps.len() as f64;
        let mut sum = Complex::ZERO;
        for a in &self.amps {
            sum = sum + *a;
        }
        let mean = sum.scale(1.0 / n);
        for a in &mut self.amps {
            *a = mean.scale(2.0) - *a;
        }
    }

    /// Apply a Pauli operator (`X`/`Y`/`Z`, `I` is a no-op) to one qubit.
    pub fn apply_pauli(&mut self, target: usize, pauli: char) {
        match pauli {
            'I' | 'i' => {}
            'X' | 'x' => {
                let m = [
                    [Complex::ZERO, Complex::ONE],
                    [Complex::ONE, Complex::ZERO],
                ];
                self.apply_1q(target, &m);
            }
            'Y' | 'y' => {
                let m = [
                    [Complex::ZERO, Complex::new(0.0, -1.0)],
                    [Complex::new(0.0, 1.0), Complex::ZERO],
                ];
                self.apply_1q(target, &m);
            }
            'Z' | 'z' => {
                let bit = 1usize << target;
                let mut i = 0usize;
                while i < self.amps.len() {
                    if i & bit != 0 {
                        self.amps[i] = -self.amps[i];
                    }
                    i += 1;
                }
            }
            _ => {}
        }
    }

    /// Per-basis-state probabilities |amp|².
    pub fn probabilities(&self) -> Vec<f64> {
        self.amps.iter().map(|a| a.norm_sqr()).collect()
    }

    /// Total probability mass — 1.0 for a normalised state; used as a sanity check.
    pub fn norm(&self) -> f64 {
        self.amps.iter().map(|a| a.norm_sqr()).sum()
    }

    /// Renormalise to unit length (guards against accumulated floating-point drift).
    pub fn normalize(&mut self) {
        let norm = self.norm().sqrt();
        if norm > 0.0 && (norm - 1.0).abs() > 1e-12 {
            let inv = 1.0 / norm;
            for a in &mut self.amps {
                *a = a.scale(inv);
            }
        }
    }

    /// ⟨Z_a Z_b⟩ — correlation of two qubits, in [−1, 1] (used by QAOA cuts).
    pub fn expectation_zz(&self, a: usize, b: usize) -> f64 {
        let (ba, bb) = (1usize << a, 1usize << b);
        let mut acc = 0.0;
        for (i, amp) in self.amps.iter().enumerate() {
            let parity = ((i & ba != 0) ^ (i & bb != 0)) as i32;
            let sign = if parity == 0 { 1.0 } else { -1.0 };
            acc += sign * amp.norm_sqr();
        }
        acc
    }

    /// Sample `shots` measurements in the computational basis, returning a
    /// histogram of basis-state index → count. Builds the CDF once (O(dim)) then
    /// binary-searches per shot (O(log dim)).
    pub fn sample(&self, rng: &mut Rng, shots: usize) -> Vec<(usize, u64)> {
        use std::collections::HashMap;
        let mut cdf = Vec::with_capacity(self.amps.len());
        let mut running = 0.0;
        for a in &self.amps {
            running += a.norm_sqr();
            cdf.push(running);
        }
        let total = *cdf.last().unwrap_or(&0.0);
        let mut counts: HashMap<usize, u64> = HashMap::new();
        for _ in 0..shots {
            let r = rng.uniform() * total;
            // First index whose cumulative probability exceeds r.
            let idx = match cdf.binary_search_by(|p| p.partial_cmp(&r).unwrap()) {
                Ok(i) => i,
                Err(i) => i,
            }
            .min(self.amps.len().saturating_sub(1));
            *counts.entry(idx).or_insert(0) += 1;
        }
        let mut out: Vec<(usize, u64)> = counts.into_iter().collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        out
    }
}

/// Render a basis-state index as a qubit bitstring, qubit 0 on the right
/// (most-significant qubit first), matching the usual `|q_{n-1}…q_0⟩` notation.
pub fn bitstring(index: usize, n: usize) -> String {
    let mut s = String::with_capacity(n);
    for q in (0..n).rev() {
        s.push(if index & (1usize << q) != 0 { '1' } else { '0' });
    }
    s
}
