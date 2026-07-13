//! End-to-end correctness tests: each exercises a known textbook result so a
//! regression in the gate kernels or the algorithms shows up immediately.

use serde_json::{from_value, json};

use crate::algorithms::{grover, qaoa_maxcut, vqe, Deadline, Edge, PauliTerm};
use crate::complex::Complex;
use crate::gates::{run_circuit, GateSpec};
use crate::rng::Rng;
use crate::run::{solve, SolveRequest};
use crate::state::State;

fn gates(value: serde_json::Value) -> Vec<GateSpec> {
    from_value(value).expect("gate list should deserialize")
}

#[test]
fn complex_multiply_and_phase() {
    // i · i = -1
    let i = Complex::new(0.0, 1.0);
    let prod = i * i;
    assert!((prod.re - -1.0).abs() < 1e-12 && prod.im.abs() < 1e-12);
    // e^{iπ} = -1
    let p = Complex::phase(std::f64::consts::PI);
    assert!((p.re - -1.0).abs() < 1e-12 && p.im.abs() < 1e-9);
}

#[test]
fn hadamard_makes_equal_superposition() {
    let state = run_circuit(1, &gates(json!([{ "gate": "h", "target": 0 }]))).unwrap();
    let probs = state.probabilities();
    assert!((probs[0] - 0.5).abs() < 1e-12);
    assert!((probs[1] - 0.5).abs() < 1e-12);
    assert!((state.norm() - 1.0).abs() < 1e-12);
}

#[test]
fn bell_state_is_maximally_entangled() {
    // H on q0, then CNOT(0 -> 1): (|00> + |11>)/√2
    let circuit = gates(json!([
        { "gate": "h", "target": 0 },
        { "gate": "cx", "control": 0, "target": 1 }
    ]));
    let state = run_circuit(2, &circuit).unwrap();
    let probs = state.probabilities();
    assert!((probs[0b00] - 0.5).abs() < 1e-12, "p(00) = {}", probs[0b00]);
    assert!((probs[0b11] - 0.5).abs() < 1e-12, "p(11) = {}", probs[0b11]);
    assert!(probs[0b01] < 1e-12 && probs[0b10] < 1e-12);
}

#[test]
fn swap_exchanges_qubits() {
    // Prepare |10> (X on q1), then SWAP(0,1) -> |01>.
    let circuit = gates(json!([
        { "gate": "x", "target": 1 },
        { "gate": "swap", "qubits": [0, 1] }
    ]));
    let state = run_circuit(2, &circuit).unwrap();
    let probs = state.probabilities();
    assert!((probs[0b01] - 1.0).abs() < 1e-12, "expected |01>, got {:?}", probs);
}

#[test]
fn toffoli_flips_only_when_both_controls_set() {
    // |110> with controls q0,q1 set -> CCX flips q2 -> |111> (index 7).
    let circuit = gates(json!([
        { "gate": "x", "target": 0 },
        { "gate": "x", "target": 1 },
        { "gate": "ccx", "controls": [0, 1], "target": 2 }
    ]));
    let state = run_circuit(3, &circuit).unwrap();
    let probs = state.probabilities();
    assert!((probs[0b111] - 1.0).abs() < 1e-12, "expected |111>, got {:?}", probs);
}

#[test]
fn rx_pi_is_a_bit_flip() {
    // RX(π) maps |0> to (up to global phase) |1>.
    let state = run_circuit(1, &gates(json!([{ "gate": "rx", "target": 0, "theta": std::f64::consts::PI }]))).unwrap();
    let probs = state.probabilities();
    assert!((probs[1] - 1.0).abs() < 1e-9, "p(1) = {}", probs[1]);
}

#[test]
fn unknown_gate_is_rejected() {
    let err = run_circuit(1, &gates(json!([{ "gate": "frobnicate", "target": 0 }]))).unwrap_err();
    assert!(err.contains("frobnicate"), "{err}");
}

#[test]
fn out_of_range_qubit_is_rejected() {
    let err = run_circuit(2, &gates(json!([{ "gate": "h", "target": 5 }]))).unwrap_err();
    assert!(err.contains("out of range"), "{err}");
}

#[test]
fn grover_amplifies_a_single_marked_state() {
    // 3 qubits, mark |101> (index 5). Optimal ~2 iterations should push its
    // probability well above the uniform 1/8.
    let result = grover(3, &[5], None).unwrap();
    assert_eq!(result.iterations, 2);
    let p_marked = result.state.probabilities()[5];
    assert!(p_marked > 0.9, "marked probability only {p_marked}");
    assert!(result.success_probability > 0.9);
}

#[test]
fn grover_amplifies_two_marked_states() {
    let result = grover(4, &[3, 12], None).unwrap();
    assert!(result.success_probability > 0.9, "success {}", result.success_probability);
}

#[test]
fn qaoa_finds_single_edge_cut() {
    // Single edge graph: optimal cut is 1; p=1 QAOA can reach it.
    let mut rng = Rng::new(7);
    let edges = vec![Edge { u: 0, v: 1, weight: 1.0 }];
    let result = qaoa_maxcut(2, &edges, 1, 4000, &mut rng, Deadline::after_ms(10_000)).unwrap();
    assert_eq!(result.optimal_cut, Some(1.0));
    assert!(result.expected_cut > 0.95, "expected cut {}", result.expected_cut);
    assert_eq!(result.best_cut, 1.0);
}

#[test]
fn qaoa_solves_triangle_maxcut() {
    // Triangle: every edge crossing is impossible, optimal cut is 2.
    let mut rng = Rng::new(11);
    let edges = vec![
        Edge { u: 0, v: 1, weight: 1.0 },
        Edge { u: 1, v: 2, weight: 1.0 },
        Edge { u: 0, v: 2, weight: 1.0 },
    ];
    let result = qaoa_maxcut(3, &edges, 2, 6000, &mut rng, Deadline::after_ms(15_000)).unwrap();
    assert_eq!(result.optimal_cut, Some(2.0));
    assert_eq!(result.best_cut, 2.0);
    // Optimised expectation should beat the random-assignment baseline of 1.5.
    assert!(result.expected_cut > 1.5, "expected cut {}", result.expected_cut);
}

#[test]
fn vqe_ground_energy_of_single_z() {
    // H = Z has ground energy -1 (the |1> eigenstate).
    let mut rng = Rng::new(3);
    let terms = vec![PauliTerm { coeff: 1.0, ops: vec![(0, 'Z')] }];
    let result = vqe(1, &terms, 1, 3000, &mut rng, Deadline::after_ms(10_000)).unwrap();
    assert!((result.energy - -1.0).abs() < 0.05, "energy {}", result.energy);
    let exact = result.exact_ground.expect("exact ground for 1 qubit");
    assert!((exact - -1.0).abs() < 1e-6, "exact {exact}");
}

#[test]
fn vqe_ground_energy_of_transverse_field() {
    // H = X has ground energy -1 (the |-> eigenstate); requires off-diagonal terms.
    let mut rng = Rng::new(5);
    let terms = vec![PauliTerm { coeff: 1.0, ops: vec![(0, 'X')] }];
    let result = vqe(1, &terms, 1, 4000, &mut rng, Deadline::after_ms(10_000)).unwrap();
    assert!(result.energy < -0.9, "energy {}", result.energy);
    let exact = result.exact_ground.expect("exact ground for 1 qubit");
    assert!((exact - -1.0).abs() < 1e-6, "exact {exact}");
}

#[test]
fn vqe_ground_energy_of_zz_coupling() {
    // H = Z0 Z1 has ground energy -1.
    let mut rng = Rng::new(13);
    let terms = vec![PauliTerm { coeff: 1.0, ops: vec![(0, 'Z'), (1, 'Z')] }];
    let result = vqe(2, &terms, 2, 5000, &mut rng, Deadline::after_ms(12_000)).unwrap();
    assert!(result.energy < -0.9, "energy {}", result.energy);
    let exact = result.exact_ground.expect("exact ground for 2 qubits");
    assert!((exact - -1.0).abs() < 1e-6, "exact {exact}");
}

#[test]
fn pauli_y_application_is_correct() {
    // Y|0> = i|1>: probability fully on |1>.
    let mut state = State::zero(1);
    state.apply_pauli(0, 'Y');
    let probs = state.probabilities();
    assert!((probs[1] - 1.0).abs() < 1e-12);
}

#[test]
fn solve_dispatches_circuit_from_json() {
    let request: SolveRequest = from_value(json!({
        "mode": "circuit",
        "qubits": 2,
        "shots": 2000,
        "seed": 42,
        "gates": [
            { "gate": "h", "target": 0 },
            { "gate": "cx", "control": 0, "target": 1 }
        ]
    }))
    .unwrap();
    let response = solve(request).unwrap();
    assert_eq!(response.mode, "circuit");
    assert_eq!(response.qubits, 2);
    // Bell state: only |00> and |11> appear.
    assert_eq!(response.top_outcomes.len(), 2);
    for outcome in &response.top_outcomes {
        assert!(outcome.bitstring == "00" || outcome.bitstring == "11");
        assert!(outcome.count.unwrap_or(0) > 0);
    }
    assert!((response.state_norm - 1.0).abs() < 1e-9);
}

#[test]
fn solve_infers_mode_from_payload_shape() {
    // No explicit mode, but a `marked` list -> Grover.
    let request: SolveRequest = from_value(json!({
        "qubits": 3,
        "marked": [5],
        "shots": 0
    }))
    .unwrap();
    let response = solve(request).unwrap();
    assert_eq!(response.mode, "grover");
    assert!(response.success_probability.unwrap() > 0.9);
}

#[test]
fn solve_parses_hamiltonian_pauli_string() {
    let request: SolveRequest = from_value(json!({
        "mode": "vqe",
        "layers": 2,
        "seed": 9,
        "maxEvals": 4000,
        "hamiltonian": [ { "coeff": 1.0, "pauli": "ZZ" } ]
    }))
    .unwrap();
    let response = solve(request).unwrap();
    assert_eq!(response.mode, "vqe");
    assert_eq!(response.qubits, 2);
    assert!(response.objective.unwrap() < -0.9, "energy {:?}", response.objective);
    assert!((response.exact_ground_energy.unwrap() - -1.0).abs() < 1e-6);
}

#[test]
fn solve_rejects_oversized_register() {
    let request: SolveRequest = from_value(json!({
        "mode": "circuit",
        "qubits": 30,
        "gates": []
    }))
    .unwrap();
    assert!(solve(request).is_err());
}
