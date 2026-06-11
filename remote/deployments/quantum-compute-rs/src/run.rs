//! Request/response contract and mode dispatch.
//!
//! One endpoint accepts a JSON `SolveRequest`; the `mode` field (or, if absent,
//! the shape of the payload) selects one of: `circuit`, `grover`, `qaoa`, `vqe`.
//! Every mode returns the same `SolveResponse` envelope — a measurement
//! distribution plus the per-mode answer — so downstream consumers parse one
//! shape. All sizes are bounded and the variational modes run under a
//! cooperative wall-clock deadline.

use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::algorithms::{grover, qaoa_maxcut, vqe, Deadline, Edge, PauliTerm};
use crate::gates::{run_circuit, GateSpec};
use crate::rng::Rng;
use crate::state::{bitstring, State};

const MAX_QUBITS: usize = 20;
const MAX_QAOA_NODES: usize = 16;
const MAX_VQE_QUBITS: usize = 14;
const MAX_GATES: usize = 100_000;
const MAX_SHOTS: usize = 1_000_000;
const DEFAULT_SHOTS: usize = 1024;
const MAX_TOP_OUTCOMES: usize = 64;
/// Include the full amplitude vector only for registers no larger than this.
const MAX_AMPLITUDES_DIM: usize = 512;
const DEFAULT_MAX_EVALS: usize = 2000;
const MAX_EVALS_CAP: usize = 200_000;
const DEFAULT_MAX_SOLVE_MS: u64 = 20_000;
const MIN_SOLVE_MS: u64 = 500;
const MAX_SOLVE_MS: u64 = 120_000;

// --- request --------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SolveRequest {
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub seed: Option<u64>,
    #[serde(default)]
    pub shots: Option<usize>,
    #[serde(default)]
    pub qubits: Option<usize>,
    #[serde(default)]
    pub gates: Vec<GateSpec>,
    // Grover
    #[serde(default)]
    pub marked: Vec<usize>,
    #[serde(default)]
    pub iterations: Option<usize>,
    // QAOA
    #[serde(default)]
    pub graph: Option<GraphSpec>,
    #[serde(default)]
    pub layers: Option<usize>,
    // VQE
    #[serde(default)]
    pub hamiltonian: Vec<HamiltonianTermSpec>,
    // optimiser / limits
    #[serde(default)]
    pub max_evals: Option<usize>,
    #[serde(default)]
    pub max_solve_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphSpec {
    #[serde(default)]
    pub nodes: Option<usize>,
    #[serde(default)]
    pub edges: Vec<EdgeSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum EdgeSpec {
    Obj { u: usize, v: usize, weight: Option<f64> },
    Weighted([f64; 3]),
    Pair([usize; 2]),
}

impl EdgeSpec {
    fn to_edge(&self) -> Edge {
        match self {
            EdgeSpec::Obj { u, v, weight } => Edge {
                u: *u,
                v: *v,
                weight: weight.unwrap_or(1.0),
            },
            EdgeSpec::Weighted([u, v, w]) => Edge {
                u: *u as usize,
                v: *v as usize,
                weight: *w,
            },
            EdgeSpec::Pair([u, v]) => Edge {
                u: *u,
                v: *v,
                weight: 1.0,
            },
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HamiltonianTermSpec {
    #[serde(default = "one")]
    pub coeff: f64,
    /// Dense Pauli string; position `i` (left to right) acts on qubit `i`.
    #[serde(default)]
    pub pauli: Option<String>,
    /// Sparse form: explicit (qubit, Pauli) factors.
    #[serde(default)]
    pub ops: Vec<OpSpec>,
}

fn one() -> f64 {
    1.0
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum OpSpec {
    Obj { qubit: usize, pauli: String },
    Tuple(usize, String),
}

// --- response -------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub bitstring: String,
    pub index: usize,
    pub probability: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct Amplitude {
    pub re: f64,
    pub im: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SolveResponse {
    pub ok: bool,
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    pub qubits: usize,
    pub shots: usize,
    pub top_outcomes: Vec<Outcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amplitudes: Option<Vec<Amplitude>>,
    // Grover
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iterations: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub success_probability: Option<f64>,
    // QAOA / VQE
    #[serde(skip_serializing_if = "Option::is_none")]
    pub objective_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub objective: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_objective: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub optimal_objective: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approximation_ratio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exact_ground_energy: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub best_bitstring: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Vec<f64>>,
    pub state_norm: f64,
    pub duration_ms: u128,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

// --- helpers --------------------------------------------------------------

fn sanitize(x: f64) -> f64 {
    if x.is_finite() {
        x
    } else {
        0.0
    }
}

/// Highest-probability basis states (top-K by exact probability), with sampled
/// shot counts attached when `shots > 0`. Uses a bounded selection pass so it
/// stays cheap even for a million-amplitude register.
fn build_outcomes(state: &State, shots: usize, rng: &mut Rng) -> (Vec<Outcome>, usize) {
    let probs = state.probabilities();
    // Bounded top-K selection: keep the K largest (idx, prob) seen so far.
    let mut top: Vec<(f64, usize)> = Vec::with_capacity(MAX_TOP_OUTCOMES + 1);
    for (i, &p) in probs.iter().enumerate() {
        if p <= 1e-12 {
            continue;
        }
        if top.len() < MAX_TOP_OUTCOMES {
            top.push((p, i));
            if top.len() == MAX_TOP_OUTCOMES {
                top.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            }
        } else if p > top[0].0 {
            top[0] = (p, i);
            // Restore the smallest-at-front invariant.
            top.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        }
    }
    top.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());

    let counts = if shots > 0 {
        state
            .sample(rng, shots)
            .into_iter()
            .collect::<std::collections::HashMap<usize, u64>>()
    } else {
        std::collections::HashMap::new()
    };

    let outcomes = top
        .into_iter()
        .map(|(p, i)| Outcome {
            bitstring: bitstring(i, state.n),
            index: i,
            probability: sanitize(p),
            count: counts.get(&i).copied(),
        })
        .collect();
    (outcomes, shots)
}

fn maybe_amplitudes(state: &State) -> Option<Vec<Amplitude>> {
    if state.dim() > MAX_AMPLITUDES_DIM {
        return None;
    }
    Some(
        state
            .amps
            .iter()
            .map(|a| Amplitude {
                re: sanitize(a.re),
                im: sanitize(a.im),
            })
            .collect(),
    )
}

fn infer_circuit_qubits(gates: &[GateSpec]) -> usize {
    let mut max = 0usize;
    let mut seen = false;
    let mut consider = |q: usize| {
        seen = true;
        if q > max {
            max = q;
        }
    };
    for g in gates {
        if let Some(t) = g.target {
            consider(t);
        }
        for &t in &g.targets {
            consider(t);
        }
        if let Some(c) = g.control {
            consider(c);
        }
        for &c in &g.controls {
            consider(c);
        }
        for &q in &g.qubits {
            consider(q);
        }
    }
    if seen {
        max + 1
    } else {
        1
    }
}

fn parse_hamiltonian(specs: &[HamiltonianTermSpec]) -> Result<(Vec<PauliTerm>, usize), String> {
    let mut terms = Vec::with_capacity(specs.len());
    let mut max_qubit = 0usize;
    for spec in specs {
        if !spec.coeff.is_finite() {
            return Err("Hamiltonian coefficient must be finite".into());
        }
        let mut ops: Vec<(usize, char)> = Vec::new();
        if let Some(pauli) = &spec.pauli {
            for (i, ch) in pauli.chars().enumerate() {
                let up = ch.to_ascii_uppercase();
                if !matches!(up, 'I' | 'X' | 'Y' | 'Z') {
                    return Err(format!("invalid Pauli letter '{ch}' in \"{pauli}\""));
                }
                if up != 'I' {
                    ops.push((i, up));
                    max_qubit = max_qubit.max(i);
                }
            }
        }
        for op in &spec.ops {
            let (qubit, letter) = match op {
                OpSpec::Obj { qubit, pauli } => (*qubit, pauli.chars().next().unwrap_or('I')),
                OpSpec::Tuple(qubit, pauli) => (*qubit, pauli.chars().next().unwrap_or('I')),
            };
            let up = letter.to_ascii_uppercase();
            if !matches!(up, 'I' | 'X' | 'Y' | 'Z') {
                return Err(format!("invalid Pauli letter '{letter}'"));
            }
            if up != 'I' {
                ops.push((qubit, up));
                max_qubit = max_qubit.max(qubit);
            }
        }
        terms.push(PauliTerm {
            coeff: spec.coeff,
            ops,
        });
    }
    Ok((terms, max_qubit + 1))
}

fn select_mode(req: &SolveRequest) -> String {
    if let Some(m) = &req.mode {
        let m = m.trim().to_lowercase();
        if !m.is_empty() && m != "auto" {
            return m;
        }
    }
    if !req.hamiltonian.is_empty() {
        "vqe".into()
    } else if req.graph.is_some() {
        "qaoa".into()
    } else if !req.marked.is_empty() {
        "grover".into()
    } else {
        "circuit".into()
    }
}

// --- entry point ----------------------------------------------------------

/// Run one solve request to completion. CPU-bound; callers run it on a blocking
/// thread. Returns a descriptive error string on bad input.
pub fn solve(req: SolveRequest) -> Result<SolveResponse, String> {
    let started = Instant::now();
    let mode = select_mode(&req);
    let mut warnings: Vec<String> = Vec::new();

    let seed = req.seed.unwrap_or(0x00C0_FFEE);
    let mut rng = Rng::new(seed);

    let mut shots = req.shots.unwrap_or(DEFAULT_SHOTS);
    if shots > MAX_SHOTS {
        warnings.push(format!("shots clamped from {shots} to {MAX_SHOTS}"));
        shots = MAX_SHOTS;
    }

    let max_evals = req
        .max_evals
        .unwrap_or(DEFAULT_MAX_EVALS)
        .clamp(1, MAX_EVALS_CAP);
    let solve_ms = req
        .max_solve_ms
        .unwrap_or(DEFAULT_MAX_SOLVE_MS)
        .clamp(MIN_SOLVE_MS, MAX_SOLVE_MS);
    let deadline = Deadline::after_ms(solve_ms);

    let request_id = req.request_id.clone().map(|id| {
        let mut id = id;
        id.truncate(200);
        id
    });

    let mut response = match mode.as_str() {
        "circuit" | "statevector" | "sample" => {
            if req.gates.len() > MAX_GATES {
                return Err(format!("too many gates: {} (max {MAX_GATES})", req.gates.len()));
            }
            let n = req.qubits.unwrap_or_else(|| infer_circuit_qubits(&req.gates));
            guard_qubits(n, MAX_QUBITS, "circuit")?;
            let state = run_circuit(n, &req.gates)?;
            let (top_outcomes, shots) = build_outcomes(&state, shots, &mut rng);
            SolveResponse {
                ok: true,
                mode: "circuit".into(),
                request_id,
                qubits: n,
                shots,
                amplitudes: maybe_amplitudes(&state),
                top_outcomes,
                iterations: None,
                success_probability: None,
                objective_kind: None,
                objective: None,
                expected_objective: None,
                optimal_objective: None,
                approximation_ratio: None,
                exact_ground_energy: None,
                best_bitstring: None,
                parameters: None,
                state_norm: state.norm(),
                duration_ms: 0,
                warnings: Vec::new(),
            }
        }

        "grover" | "search" => {
            if req.marked.is_empty() {
                return Err("grover mode requires a non-empty `marked` list".into());
            }
            let inferred = req
                .marked
                .iter()
                .copied()
                .max()
                .map(|m| (usize::BITS - m.leading_zeros()).max(1) as usize)
                .unwrap_or(1);
            let n = req.qubits.unwrap_or(inferred);
            guard_qubits(n, MAX_QUBITS, "grover")?;
            let result = grover(n, &req.marked, req.iterations)?;
            let (top_outcomes, shots) = build_outcomes(&result.state, shots, &mut rng);
            let best = top_outcomes.first().map(|o| o.bitstring.clone());
            SolveResponse {
                ok: true,
                mode: "grover".into(),
                request_id,
                qubits: n,
                shots,
                amplitudes: maybe_amplitudes(&result.state),
                top_outcomes,
                iterations: Some(result.iterations),
                success_probability: Some(sanitize(result.success_probability)),
                objective_kind: None,
                objective: None,
                expected_objective: None,
                optimal_objective: None,
                approximation_ratio: None,
                exact_ground_energy: None,
                best_bitstring: best,
                parameters: None,
                state_norm: result.state.norm(),
                duration_ms: 0,
                warnings: Vec::new(),
            }
        }

        "qaoa" | "maxcut" => {
            let graph = req
                .graph
                .as_ref()
                .ok_or("qaoa mode requires a `graph` with `edges`")?;
            let edges: Vec<Edge> = graph.edges.iter().map(|e| e.to_edge()).collect();
            if edges.is_empty() {
                return Err("qaoa graph has no edges".into());
            }
            let inferred = edges.iter().map(|e| e.u.max(e.v)).max().unwrap_or(0) + 1;
            let n = graph.nodes.unwrap_or(inferred).max(inferred);
            guard_qubits(n, MAX_QAOA_NODES, "qaoa")?;
            let layers = req.layers.unwrap_or(1).clamp(1, 8);
            let result = qaoa_maxcut(n, &edges, layers, max_evals, &mut rng, deadline)?;
            let (top_outcomes, shots) = build_outcomes(&result.state, shots, &mut rng);
            let approximation_ratio = result
                .optimal_cut
                .filter(|&o| o > 0.0)
                .map(|o| sanitize(result.expected_cut / o));
            if deadline.expired() {
                warnings.push("solve deadline reached; returning best parameters so far".into());
            }
            SolveResponse {
                ok: true,
                mode: "qaoa".into(),
                request_id,
                qubits: n,
                shots,
                amplitudes: maybe_amplitudes(&result.state),
                top_outcomes,
                iterations: None,
                success_probability: None,
                objective_kind: Some("cut".into()),
                objective: Some(sanitize(result.best_cut)),
                expected_objective: Some(sanitize(result.expected_cut)),
                optimal_objective: result.optimal_cut.map(sanitize),
                approximation_ratio,
                exact_ground_energy: None,
                best_bitstring: Some(bitstring(result.best_bitstring, n)),
                parameters: Some(result.params.iter().map(|&p| sanitize(p)).collect()),
                state_norm: result.state.norm(),
                duration_ms: 0,
                warnings: Vec::new(),
            }
        }

        "vqe" | "groundstate" => {
            if req.hamiltonian.is_empty() {
                return Err("vqe mode requires a non-empty `hamiltonian`".into());
            }
            let (terms, inferred) = parse_hamiltonian(&req.hamiltonian)?;
            let n = req.qubits.unwrap_or(inferred).max(inferred);
            guard_qubits(n, MAX_VQE_QUBITS, "vqe")?;
            let layers = req.layers.unwrap_or(2).clamp(1, 8);
            let result = vqe(n, &terms, layers, max_evals, &mut rng, deadline)?;
            let (top_outcomes, shots) = build_outcomes(&result.state, shots, &mut rng);
            if deadline.expired() {
                warnings.push("solve deadline reached; returning best parameters so far".into());
            }
            if result.exact_ground == None && n > 12 {
                warnings.push("exact ground energy omitted (register too large for reference solve)".into());
            }
            SolveResponse {
                ok: true,
                mode: "vqe".into(),
                request_id,
                qubits: n,
                shots,
                amplitudes: maybe_amplitudes(&result.state),
                top_outcomes,
                iterations: None,
                success_probability: None,
                objective_kind: Some("energy".into()),
                objective: Some(sanitize(result.energy)),
                expected_objective: Some(sanitize(result.energy)),
                optimal_objective: result.exact_ground.map(sanitize),
                approximation_ratio: None,
                exact_ground_energy: result.exact_ground.map(sanitize),
                best_bitstring: Some(bitstring(result.best_bitstring, n)),
                parameters: Some(result.params.iter().map(|&p| sanitize(p)).collect()),
                state_norm: result.state.norm(),
                duration_ms: 0,
                warnings: Vec::new(),
            }
        }

        other => return Err(format!("unknown mode: {other}")),
    };

    response.warnings = warnings;
    response.duration_ms = started.elapsed().as_millis();
    Ok(response)
}

fn guard_qubits(n: usize, max: usize, mode: &str) -> Result<(), String> {
    if n == 0 {
        return Err(format!("{mode} needs at least one qubit"));
    }
    if n > max {
        return Err(format!(
            "{mode} requested {n} qubits, exceeds the {max}-qubit limit (2^{max} amplitudes)"
        ));
    }
    Ok(())
}
