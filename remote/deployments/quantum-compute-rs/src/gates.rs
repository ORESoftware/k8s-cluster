//! Gate vocabulary and circuit application.
//!
//! A circuit is a flat list of [`GateSpec`]. Each spec names a gate and the
//! qubit(s) it acts on; rotation/phase gates also carry an angle. The JSON form
//! is intentionally forgiving — a target can be given as `target` or the first
//! of `targets`/`qubits`, an angle as `theta`/`angle`/`param`/`params[0]` — so
//! hand-written circuits stay terse.

use serde::Deserialize;
use std::f64::consts::FRAC_1_SQRT_2;

use crate::complex::Complex;
use crate::state::{Matrix2, State};

#[derive(Debug, Clone, Deserialize)]
pub struct GateSpec {
    pub gate: String,
    #[serde(default)]
    pub target: Option<usize>,
    #[serde(default)]
    pub targets: Vec<usize>,
    #[serde(default)]
    pub control: Option<usize>,
    #[serde(default)]
    pub controls: Vec<usize>,
    #[serde(default)]
    pub qubits: Vec<usize>,
    #[serde(default)]
    pub theta: Option<f64>,
    #[serde(default)]
    pub angle: Option<f64>,
    #[serde(default)]
    pub param: Option<f64>,
    #[serde(default)]
    pub params: Vec<f64>,
}

impl GateSpec {
    fn resolved_targets(&self) -> Vec<usize> {
        if !self.targets.is_empty() {
            self.targets.clone()
        } else if let Some(t) = self.target {
            vec![t]
        } else {
            self.qubits.clone()
        }
    }

    fn resolved_controls(&self) -> Vec<usize> {
        let mut c = self.controls.clone();
        if let Some(ctrl) = self.control {
            c.push(ctrl);
        }
        c
    }

    fn first_param(&self) -> Option<f64> {
        self.theta
            .or(self.angle)
            .or(self.param)
            .or_else(|| self.params.first().copied())
    }
}

fn h_matrix() -> Matrix2 {
    let s = FRAC_1_SQRT_2;
    [
        [Complex::real(s), Complex::real(s)],
        [Complex::real(s), Complex::real(-s)],
    ]
}

pub fn x_matrix() -> Matrix2 {
    [
        [Complex::ZERO, Complex::ONE],
        [Complex::ONE, Complex::ZERO],
    ]
}

fn y_matrix() -> Matrix2 {
    [
        [Complex::ZERO, Complex::new(0.0, -1.0)],
        [Complex::new(0.0, 1.0), Complex::ZERO],
    ]
}

fn z_matrix() -> Matrix2 {
    [
        [Complex::ONE, Complex::ZERO],
        [Complex::ZERO, Complex::real(-1.0)],
    ]
}

fn phase_matrix(theta: f64) -> Matrix2 {
    [
        [Complex::ONE, Complex::ZERO],
        [Complex::ZERO, Complex::phase(theta)],
    ]
}

pub fn rx_matrix(theta: f64) -> Matrix2 {
    let (c, s) = ((theta / 2.0).cos(), (theta / 2.0).sin());
    [
        [Complex::real(c), Complex::new(0.0, -s)],
        [Complex::new(0.0, -s), Complex::real(c)],
    ]
}

pub fn ry_matrix(theta: f64) -> Matrix2 {
    let (c, s) = ((theta / 2.0).cos(), (theta / 2.0).sin());
    [
        [Complex::real(c), Complex::real(-s)],
        [Complex::real(s), Complex::real(c)],
    ]
}

pub fn rz_matrix(theta: f64) -> Matrix2 {
    [
        [Complex::phase(-theta / 2.0), Complex::ZERO],
        [Complex::ZERO, Complex::phase(theta / 2.0)],
    ]
}

/// Resolve a named single-qubit gate to its matrix, or `None` if the name is
/// not a single-qubit gate (e.g. it is a multi-qubit gate handled elsewhere).
fn single_qubit_matrix(name: &str, theta: Option<f64>) -> Option<Matrix2> {
    let m = match name {
        "i" | "id" => [
            [Complex::ONE, Complex::ZERO],
            [Complex::ZERO, Complex::ONE],
        ],
        "h" | "hadamard" => h_matrix(),
        "x" | "not" => x_matrix(),
        "y" => y_matrix(),
        "z" => z_matrix(),
        "s" => phase_matrix(std::f64::consts::FRAC_PI_2),
        "sdg" | "sdagger" => phase_matrix(-std::f64::consts::FRAC_PI_2),
        "t" => phase_matrix(std::f64::consts::FRAC_PI_4),
        "tdg" | "tdagger" => phase_matrix(-std::f64::consts::FRAC_PI_4),
        "rx" => rx_matrix(theta.unwrap_or(0.0)),
        "ry" => ry_matrix(theta.unwrap_or(0.0)),
        "rz" => rz_matrix(theta.unwrap_or(0.0)),
        "p" | "phase" | "u1" => phase_matrix(theta.unwrap_or(0.0)),
        _ => return None,
    };
    Some(m)
}

/// Apply one gate to the state, validating qubit indices against `n`.
pub fn apply_gate(state: &mut State, spec: &GateSpec) -> Result<(), String> {
    let n = state.n;
    let name = spec.gate.trim().to_lowercase();
    let targets = spec.resolved_targets();
    let controls = spec.resolved_controls();
    let theta = spec.first_param();

    let check = |q: usize| -> Result<(), String> {
        if q >= n {
            Err(format!("qubit index {q} out of range for {n}-qubit register"))
        } else {
            Ok(())
        }
    };

    match name.as_str() {
        // No-ops that may appear in exported circuits.
        "barrier" | "measure" | "measureall" | "reset" => Ok(()),

        // Two-qubit controlled gates.
        "cx" | "cnot" => {
            let (c, t) = two_qubit_pair(&controls, &targets, &spec.qubits)?;
            check(c)?;
            check(t)?;
            distinct(c, t)?;
            state.apply_controlled_1q(&[c], t, &x_matrix());
            Ok(())
        }
        "cz" => {
            let (c, t) = two_qubit_pair(&controls, &targets, &spec.qubits)?;
            check(c)?;
            check(t)?;
            distinct(c, t)?;
            state.apply_controlled_1q(&[c], t, &z_matrix());
            Ok(())
        }
        "cy" => {
            let (c, t) = two_qubit_pair(&controls, &targets, &spec.qubits)?;
            check(c)?;
            check(t)?;
            distinct(c, t)?;
            state.apply_controlled_1q(&[c], t, &y_matrix());
            Ok(())
        }
        "crx" | "cry" | "crz" | "cphase" | "cp" => {
            let (c, t) = two_qubit_pair(&controls, &targets, &spec.qubits)?;
            check(c)?;
            check(t)?;
            distinct(c, t)?;
            let base = &name[1..];
            let m = single_qubit_matrix(base, theta)
                .ok_or_else(|| format!("unknown controlled gate: {name}"))?;
            state.apply_controlled_1q(&[c], t, &m);
            Ok(())
        }
        "swap" => {
            let pair = if spec.qubits.len() >= 2 {
                (spec.qubits[0], spec.qubits[1])
            } else if targets.len() >= 2 {
                (targets[0], targets[1])
            } else {
                return Err("swap requires two qubits".into());
            };
            check(pair.0)?;
            check(pair.1)?;
            distinct(pair.0, pair.1)?;
            state.swap(pair.0, pair.1);
            Ok(())
        }
        "ccx" | "toffoli" | "ccnot" => {
            let (c1, c2, t) = three_qubit(&controls, &targets, &spec.qubits)?;
            check(c1)?;
            check(c2)?;
            check(t)?;
            if c1 == c2 || c1 == t || c2 == t {
                return Err("ccx requires three distinct qubits".into());
            }
            state.apply_controlled_1q(&[c1, c2], t, &x_matrix());
            Ok(())
        }

        // Everything else is a single-qubit gate applied to each listed target,
        // optionally controlled when `controls` are present (e.g. controlled-RY
        // for variational ansätze).
        _ => {
            let m = single_qubit_matrix(&name, theta)
                .ok_or_else(|| format!("unknown gate: {name}"))?;
            if targets.is_empty() {
                return Err(format!("gate {name} needs at least one target qubit"));
            }
            for &t in &targets {
                check(t)?;
                for &c in &controls {
                    check(c)?;
                    distinct(c, t)?;
                }
                state.apply_controlled_1q(&controls, t, &m);
            }
            Ok(())
        }
    }
}

fn distinct(a: usize, b: usize) -> Result<(), String> {
    if a == b {
        Err(format!("control and target must differ (both {a})"))
    } else {
        Ok(())
    }
}

/// Resolve a (control, target) pair from whichever fields the caller supplied.
fn two_qubit_pair(
    controls: &[usize],
    targets: &[usize],
    qubits: &[usize],
) -> Result<(usize, usize), String> {
    if !controls.is_empty() && !targets.is_empty() {
        Ok((controls[0], targets[0]))
    } else if qubits.len() >= 2 {
        Ok((qubits[0], qubits[1]))
    } else if targets.len() >= 2 {
        Ok((targets[0], targets[1]))
    } else {
        Err("two-qubit gate requires a control and a target".into())
    }
}

/// Resolve (control, control, target) for a Toffoli from the supplied fields.
fn three_qubit(
    controls: &[usize],
    targets: &[usize],
    qubits: &[usize],
) -> Result<(usize, usize, usize), String> {
    if controls.len() >= 2 && !targets.is_empty() {
        Ok((controls[0], controls[1], targets[0]))
    } else if qubits.len() >= 3 {
        Ok((qubits[0], qubits[1], qubits[2]))
    } else if targets.len() >= 3 {
        Ok((targets[0], targets[1], targets[2]))
    } else {
        Err("ccx requires two controls and a target".into())
    }
}

/// Run a whole circuit on a fresh |0…0⟩ register.
pub fn run_circuit(n: usize, gates: &[GateSpec]) -> Result<State, String> {
    let mut state = State::zero(n);
    for (i, g) in gates.iter().enumerate() {
        apply_gate(&mut state, g).map_err(|e| format!("gate {i} ({}): {e}", g.gate))?;
    }
    Ok(state)
}
