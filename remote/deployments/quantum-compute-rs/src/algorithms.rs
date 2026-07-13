//! Textbook quantum algorithms, run end-to-end on the state-vector simulator.
//!
//!   * Grover — amplitude amplification over a phase oracle.
//!   * QAOA   — the Quantum Approximate Optimization Algorithm for weighted
//!              MaxCut, with a classical gradient-free outer optimiser.
//!   * VQE    — the Variational Quantum Eigensolver: a hardware-efficient ansatz
//!              whose parameters are optimised to minimise ⟨H⟩ for a Pauli-sum
//!              Hamiltonian, with the exact ground energy computed by shifted
//!              power iteration for reference on small registers.

use std::f64::consts::PI;
use std::time::{Duration, Instant};

use crate::complex::Complex;
use crate::gates::{rx_matrix, rz_matrix, ry_matrix, x_matrix};
use crate::rng::Rng;
use crate::state::State;

/// Cooperative wall-clock budget. The variational optimisers check it between
/// evaluations so no single request can pin a core regardless of the requested
/// layer/iteration counts.
#[derive(Clone, Copy)]
pub struct Deadline {
    stop: Option<Instant>,
}

impl Deadline {
    pub fn after_ms(ms: u64) -> Self {
        Deadline {
            stop: Some(Instant::now() + Duration::from_millis(ms)),
        }
    }

    pub fn expired(&self) -> bool {
        self.stop.map_or(false, |s| Instant::now() >= s)
    }
}

// ---------------------------------------------------------------------------
// Grover search
// ---------------------------------------------------------------------------

pub struct GroverResult {
    pub iterations: usize,
    pub success_probability: f64,
    pub state: State,
}

/// Grover amplitude amplification of the `marked` basis states. With `iterations`
/// unset, uses the analytically optimal ⌊(π/4)·√(N/M)⌋ rounds.
pub fn grover(n: usize, marked: &[usize], iterations: Option<usize>) -> Result<GroverResult, String> {
    let dim = 1usize << n;
    let mut unique: Vec<usize> = marked.to_vec();
    unique.sort_unstable();
    unique.dedup();
    if unique.is_empty() {
        return Err("grover requires at least one marked state".into());
    }
    if let Some(&max) = unique.last() {
        if max >= dim {
            return Err(format!("marked state {max} out of range for {n} qubits (max {})", dim - 1));
        }
    }

    let m = unique.len() as f64;
    let optimal = ((PI / 4.0) * (dim as f64 / m).sqrt() - 0.5).round().max(0.0) as usize;
    let rounds = iterations.unwrap_or(optimal).min(100_000);

    // Uniform superposition.
    let mut state = State::zero(n);
    let h = crate::gates::GateSpec {
        gate: "h".into(),
        target: None,
        targets: (0..n).collect(),
        control: None,
        controls: vec![],
        qubits: vec![],
        theta: None,
        angle: None,
        param: None,
        params: vec![],
    };
    crate::gates::apply_gate(&mut state, &h)?;

    for _ in 0..rounds {
        state.phase_flip(&unique);
        state.reflect_about_mean();
    }

    let probs = state.probabilities();
    let success = unique.iter().map(|&i| probs[i]).sum();
    Ok(GroverResult {
        iterations: rounds,
        success_probability: success,
        state,
    })
}

// ---------------------------------------------------------------------------
// QAOA — weighted MaxCut
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub struct Edge {
    pub u: usize,
    pub v: usize,
    pub weight: f64,
}

pub struct QaoaResult {
    pub params: Vec<f64>,
    pub expected_cut: f64,
    pub best_bitstring: usize,
    pub best_cut: f64,
    pub optimal_cut: Option<f64>,
    pub state: State,
}

/// Apply RZZ(θ) = exp(−i·θ/2·Z⊗Z) to (u, v) via CX · RZ · CX.
fn apply_rzz(state: &mut State, u: usize, v: usize, theta: f64) {
    state.apply_controlled_1q(&[u], v, &x_matrix());
    state.apply_1q(v, &rz_matrix(theta));
    state.apply_controlled_1q(&[u], v, &x_matrix());
}

/// Build the QAOA ansatz state for the given (γ, β) parameters.
/// `params` is `[γ_0..γ_{p-1}, β_0..β_{p-1}]`.
fn qaoa_state(n: usize, edges: &[Edge], layers: usize, params: &[f64]) -> State {
    let mut state = State::zero(n);
    let hadamard = [
        [
            Complex::real(std::f64::consts::FRAC_1_SQRT_2),
            Complex::real(std::f64::consts::FRAC_1_SQRT_2),
        ],
        [
            Complex::real(std::f64::consts::FRAC_1_SQRT_2),
            Complex::real(-std::f64::consts::FRAC_1_SQRT_2),
        ],
    ];
    for q in 0..n {
        state.apply_1q(q, &hadamard);
    }
    for l in 0..layers {
        let gamma = params[l];
        let beta = params[layers + l];
        for e in edges {
            apply_rzz(&mut state, e.u, e.v, 2.0 * gamma * e.weight);
        }
        let mixer = rx_matrix(2.0 * beta);
        for q in 0..n {
            state.apply_1q(q, &mixer);
        }
    }
    state
}

/// Expected cut value ⟨C⟩ = Σ_e w_e·(1 − ⟨Z_u Z_v⟩)/2 of a state.
fn expected_cut(state: &State, edges: &[Edge]) -> f64 {
    edges
        .iter()
        .map(|e| e.weight * (1.0 - state.expectation_zz(e.u, e.v)) / 2.0)
        .sum()
}

/// Cut value of a concrete assignment (basis-state index).
fn cut_value(assignment: usize, edges: &[Edge]) -> f64 {
    edges
        .iter()
        .filter(|e| ((assignment >> e.u) & 1) != ((assignment >> e.v) & 1))
        .map(|e| e.weight)
        .sum()
}

pub fn qaoa_maxcut(
    n: usize,
    edges: &[Edge],
    layers: usize,
    max_evals: usize,
    rng: &mut Rng,
    deadline: Deadline,
) -> Result<QaoaResult, String> {
    if n == 0 {
        return Err("qaoa requires at least one node".into());
    }
    for e in edges {
        if e.u >= n || e.v >= n {
            return Err(format!("edge ({}, {}) references a node outside 0..{n}", e.u, e.v));
        }
    }
    let layers = layers.max(1);
    // Maximise ⟨C⟩ ⇒ minimise −⟨C⟩.
    let objective = |p: &[f64]| -expected_cut(&qaoa_state(n, edges, layers, p), edges);
    let (params, neg_cut) = minimize(2 * layers, 0.0, PI, objective, rng, max_evals, deadline);
    let expected = -neg_cut;

    let state = qaoa_state(n, edges, layers, &params);
    // Read out: sample, then keep the highest-cut bitstring observed.
    let samples = state.sample(rng, 4096);
    let mut best_bitstring = 0usize;
    let mut best_cut = f64::NEG_INFINITY;
    for (idx, _) in &samples {
        let c = cut_value(*idx, edges);
        if c > best_cut {
            best_cut = c;
            best_bitstring = *idx;
        }
    }

    // Exact optimum by brute force on small graphs, for a quality reference.
    let optimal_cut = if n <= 20 {
        let mut best = 0.0f64;
        for a in 0..(1usize << n) {
            let c = cut_value(a, edges);
            if c > best {
                best = c;
            }
        }
        Some(best)
    } else {
        None
    };

    Ok(QaoaResult {
        params,
        expected_cut: expected,
        best_bitstring,
        best_cut,
        optimal_cut,
        state,
    })
}

// ---------------------------------------------------------------------------
// VQE — variational quantum eigensolver
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct PauliTerm {
    pub coeff: f64,
    /// (qubit, Pauli letter). Identity factors are omitted.
    pub ops: Vec<(usize, char)>,
}

pub struct VqeResult {
    pub params: Vec<f64>,
    pub energy: f64,
    pub exact_ground: Option<f64>,
    pub best_bitstring: usize,
    pub state: State,
}

/// ⟨ψ| P |ψ⟩ for a single Pauli string P (real, since P is Hermitian).
fn pauli_expectation(state: &State, term: &PauliTerm) -> f64 {
    let mut work = state.clone();
    for &(q, p) in &term.ops {
        work.apply_pauli(q, p);
    }
    let mut acc = 0.0;
    for (a, b) in state.amps.iter().zip(work.amps.iter()) {
        acc += (a.conj() * *b).re;
    }
    acc
}

/// ⟨ψ| H |ψ⟩ for the full Pauli-sum Hamiltonian.
fn energy(state: &State, terms: &[PauliTerm]) -> f64 {
    terms
        .iter()
        .map(|t| t.coeff * pauli_expectation(state, t))
        .sum()
}

/// H|ψ⟩ as a raw amplitude vector (reused by the reference power iteration).
fn apply_hamiltonian(terms: &[PauliTerm], state: &State) -> Vec<Complex> {
    let mut out = vec![Complex::ZERO; state.dim()];
    for term in terms {
        let mut work = state.clone();
        for &(q, p) in &term.ops {
            work.apply_pauli(q, p);
        }
        for (o, c) in out.iter_mut().zip(work.amps.iter()) {
            *o = *o + c.scale(term.coeff);
        }
    }
    out
}

/// Hardware-efficient ansatz: alternating RY layers and a linear CX entangler.
/// `params` has length `n·(layers + 1)`.
fn vqe_state(n: usize, layers: usize, params: &[f64]) -> State {
    let mut state = State::zero(n);
    let mut k = 0usize;
    for layer in 0..=layers {
        for q in 0..n {
            state.apply_1q(q, &ry_matrix(params[k]));
            k += 1;
        }
        if layer < layers {
            for q in 0..n.saturating_sub(1) {
                state.apply_controlled_1q(&[q], q + 1, &x_matrix());
            }
        }
    }
    state
}

/// Exact lowest eigenvalue of H via shifted power iteration on M = c·I − H,
/// whose top eigenvalue is c − E_min. Only for small registers.
fn exact_ground_energy(n: usize, terms: &[PauliTerm], deadline: Deadline) -> Option<f64> {
    let dim = 1usize << n;
    if dim > 4096 {
        return None;
    }
    let shift = terms.iter().map(|t| t.coeff.abs()).sum::<f64>() + 1.0;
    let mut rng = Rng::new(0x5151_2727);
    let mut v = State {
        n,
        amps: (0..dim)
            .map(|_| Complex::new(rng.normal(), rng.normal()))
            .collect(),
    };
    v.normalize();
    let mut lambda = 0.0;
    for _ in 0..1024 {
        if deadline.expired() {
            break;
        }
        let hv = apply_hamiltonian(terms, &v);
        let mut w = State {
            n,
            amps: v
                .amps
                .iter()
                .zip(hv.iter())
                .map(|(vi, hvi)| vi.scale(shift) - *hvi)
                .collect(),
        };
        let norm = w.norm().sqrt();
        if norm < 1e-15 {
            break;
        }
        w.normalize();
        // Rayleigh quotient ⟨w|M|w⟩ for the converged estimate.
        let mw = apply_hamiltonian(terms, &w);
        let mut quot = 0.0;
        for (i, wi) in w.amps.iter().enumerate() {
            let m_wi = wi.scale(shift) - mw[i];
            quot += (wi.conj() * m_wi).re;
        }
        let converged = (quot - lambda).abs() < 1e-12;
        lambda = quot;
        if converged {
            break;
        }
        v = w;
    }
    Some(shift - lambda)
}

pub fn vqe(
    n: usize,
    terms: &[PauliTerm],
    layers: usize,
    max_evals: usize,
    rng: &mut Rng,
    deadline: Deadline,
) -> Result<VqeResult, String> {
    if n == 0 {
        return Err("vqe requires at least one qubit".into());
    }
    if terms.is_empty() {
        return Err("vqe requires a non-empty Hamiltonian".into());
    }
    for t in terms {
        for &(q, _) in &t.ops {
            if q >= n {
                return Err(format!("Hamiltonian term references qubit {q} outside 0..{n}"));
            }
        }
    }
    let layers = layers.max(1);
    let n_params = n * (layers + 1);
    let objective = |p: &[f64]| energy(&vqe_state(n, layers, p), terms);
    let (params, best_energy) = minimize(n_params, 0.0, 2.0 * PI, objective, rng, max_evals, deadline);

    let state = vqe_state(n, layers, &params);
    let probs = state.probabilities();
    let best_bitstring = probs
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .map(|(i, _)| i)
        .unwrap_or(0);
    let exact_ground = exact_ground_energy(n, terms, deadline);

    Ok(VqeResult {
        params,
        energy: best_energy,
        exact_ground,
        best_bitstring,
        state,
    })
}

// ---------------------------------------------------------------------------
// Gradient-free optimiser
// ---------------------------------------------------------------------------

/// Minimise `f` over `n_params` values bounded to `[lo, hi]` using random
/// restarts refined by coordinate descent with a geometrically shrinking step.
/// Bounded by `max_evals` objective evaluations and the `deadline`; both the
/// number of restarts and the precision adapt to whatever budget remains.
fn minimize<F: Fn(&[f64]) -> f64>(
    n_params: usize,
    lo: f64,
    hi: f64,
    f: F,
    rng: &mut Rng,
    max_evals: usize,
    deadline: Deadline,
) -> (Vec<f64>, f64) {
    let max_evals = max_evals.max(1);
    let mut evals = 0usize;
    let mut best: Vec<f64> = (0..n_params).map(|_| rng.range(lo, hi)).collect();
    let mut best_val = f(&best);
    evals += 1;

    while evals < max_evals && !deadline.expired() {
        // Fresh random start (the incumbent best is preserved separately).
        let mut x: Vec<f64> = (0..n_params).map(|_| rng.range(lo, hi)).collect();
        let mut fx = f(&x);
        evals += 1;

        let mut step = (hi - lo) * 0.5;
        while step > (hi - lo) * 1e-3 && evals < max_evals && !deadline.expired() {
            let mut improved = false;
            for c in 0..n_params {
                for &dir in &[step, -step] {
                    if evals >= max_evals || deadline.expired() {
                        break;
                    }
                    let mut trial = x.clone();
                    trial[c] = (trial[c] + dir).clamp(lo, hi);
                    let ft = f(&trial);
                    evals += 1;
                    if ft < fx {
                        fx = ft;
                        x = trial;
                        improved = true;
                    }
                }
            }
            if !improved {
                step *= 0.5;
            }
        }

        if fx < best_val {
            best_val = fx;
            best = x;
        }
    }

    (best, best_val)
}
