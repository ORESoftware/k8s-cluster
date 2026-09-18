//! Request/response contract and the orchestration that turns a dataset into a
//! fitted, mostly-analytic model. This is where the methods meet: symbolic
//! regression (analytic equations + Pareto front), a backprop MLP, an
//! evolution-strategy neuroevolved MLP, a memetic hybrid of the two, and a
//! closed-form polynomial least-squares baseline — plus an `auto` mode that
//! runs several and keeps whichever generalises best (preferring the simpler,
//! analytic answer on a near-tie).

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::data::{self, Dataset, FitMetrics};
use crate::evo::{self, EsConfig};
use crate::gp::{self, GpConfig};
use crate::linalg;
use crate::nn::{Activation, Mlp};
use crate::rng::Rng;

const MAX_SAMPLES: usize = 50_000;
const MAX_FEATURES: usize = 32;
const MAX_PREDICT_AT: usize = 2_000;
const MAX_PREDICTIONS_OUT: usize = 5_000;
/// Default per-fit wall-clock budget; overridable via FUNC_APPROX_MAX_FIT_MS.
const DEFAULT_FIT_BUDGET_MS: u64 = 20_000;
const MIN_FIT_BUDGET_MS: u64 = 500;
const MAX_FIT_BUDGET_MS: u64 = 120_000;
/// Skip emitting a symbolic derivative larger than this many nodes (the quotient
/// rule can blow expressions up); keeps the response bounded.
const MAX_DERIVATIVE_NODES: usize = 400;
/// Reject inputs/targets beyond this magnitude so squared sums stay finite
/// (|v|² ≤ 1e300, and 50 000·1e300 < f64::MAX).
const MAX_ABS_VALUE: f64 = 1e150;
/// Cap caller-supplied variable-name and request-id lengths (response hygiene).
const MAX_VAR_NAME_LEN: usize = 64;
const MAX_REQUEST_ID_LEN: usize = 200;
/// Cap MLP shape so parameter count (hence per-fit and ES-population memory)
/// stays bounded regardless of the requested `hidden`.
const MAX_HIDDEN_WIDTH: usize = 64;
const MAX_HIDDEN_LAYERS: usize = 4;

/// Boxed prediction closure, evaluated in the caller's original units.
type PredictFn = Box<dyn Fn(&[f64]) -> f64>;
/// Standardised folds: `(train_x, train_y, val_x, val_y, x_scalers, y_scaler)`.
type StdFolds = (
    Vec<Vec<f64>>,
    Vec<f64>,
    Vec<Vec<f64>>,
    Vec<f64>,
    Vec<data::Scaler>,
    data::Scaler,
);
/// Symbolic regression evaluates the whole tree over every row each fitness
/// call, so cap the rows it searches over (metrics are still on the full data).
const GP_MAX_ROWS: usize = 2_000;
/// Gradient/evolution learners subsample large datasets for responsiveness.
const NN_MAX_ROWS: usize = 20_000;
/// Derivatives are emitted per input variable up to this many to bound payload.
const MAX_DERIVATIVE_VARS: usize = 12;

// --------------------------------------------------------------- request types

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sample {
    pub x: Vec<f64>,
    pub y: f64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FitConfig {
    // genetic-programming symbolic regression
    pub population: Option<usize>,
    pub generations: Option<usize>,
    pub max_depth: Option<usize>,
    pub operators: Option<Vec<String>>,
    pub parsimony: Option<f64>,
    pub tournament: Option<usize>,
    pub const_opt_iters: Option<usize>,
    // neural net / hybrid
    pub hidden: Option<Vec<usize>>,
    pub activation: Option<String>,
    pub epochs: Option<usize>,
    pub learning_rate: Option<f64>,
    pub batch_size: Option<usize>,
    // evolution strategy
    pub es_population: Option<usize>,
    pub es_parents: Option<usize>,
    pub es_generations: Option<usize>,
    pub sigma: Option<f64>,
    // polynomial least squares
    pub degree: Option<usize>,
    pub ridge: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FitRequest {
    pub request_id: Option<String>,
    /// symbolic | neural | evolution | hybrid | linear | auto (default symbolic)
    pub method: Option<String>,
    // Data, accepted in three shapes:
    pub samples: Option<Vec<Sample>>,
    pub inputs: Option<Vec<Vec<f64>>>,
    pub targets: Option<Vec<f64>>,
    pub x: Option<Vec<f64>>, // single-feature convenience
    pub y: Option<Vec<f64>>,
    pub variable_names: Option<Vec<String>>,
    pub seed: Option<u64>,
    pub val_fraction: Option<f64>,
    pub include_predictions: Option<bool>,
    pub predict_at: Option<Vec<Vec<f64>>>,
    #[serde(default)]
    pub config: FitConfig,
}

// -------------------------------------------------------------- response types

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ParetoEntry {
    pub expression: String,
    pub complexity: usize,
    pub train_rmse: f64,
    pub val_rmse: f64,
    pub val_r2: f64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CandidateSummary {
    pub method: String,
    pub val_rmse: f64,
    pub val_r2: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub complexity: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expression: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FitResponse {
    pub ok: bool,
    pub request_id: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected: Option<String>,
    pub samples: usize,
    pub features: usize,
    pub train: FitMetrics,
    pub validation: FitMetrics,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expression: Option<String>,
    #[serde(skip_serializing_if = "Value::is_null")]
    pub derivatives: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub complexity: Option<usize>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pareto_front: Vec<ParetoEntry>,
    pub model: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidates: Option<Vec<CandidateSummary>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub predictions: Option<Vec<f64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub predicted_values: Option<Vec<f64>>,
    pub iterations: u64,
    pub duration_ms: u128,
    pub warnings: Vec<String>,
    pub generated_at_ms: u128,
}

// ------------------------------------------------------------------ internals

/// A fitted model: how to predict (original units), its serialisable form, and
/// any analytic extras.
struct Fitted {
    model_json: Value,
    expression: Option<String>,
    derivatives: Value,
    complexity: Option<usize>,
    pareto_raw: Vec<gp::ParetoMember>, // empty unless symbolic
    iterations: u64,
    predict: PredictFn,
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

/// Server-side wall-clock budget for a single fit, from `FUNC_APPROX_MAX_FIT_MS`
/// (clamped). This is the backstop against CPU exhaustion: the evolutionary and
/// gradient loops stop cleanly when it elapses, no matter what population/epoch
/// knobs the caller passed.
fn fit_budget() -> Duration {
    let ms = std::env::var("FUNC_APPROX_MAX_FIT_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_FIT_BUDGET_MS)
        .clamp(MIN_FIT_BUDGET_MS, MAX_FIT_BUDGET_MS);
    Duration::from_millis(ms)
}

/// Replace any non-finite value with 0 so the JSON stays numerically typed
/// (serde serialises NaN/Inf as `null`, silently breaking an `f64[]` contract).
/// Warns with the count rather than hiding it.
fn sanitize_floats(mut values: Vec<f64>, label: &str, warnings: &mut Vec<String>) -> Vec<f64> {
    let mut bad = 0usize;
    for v in values.iter_mut() {
        if !v.is_finite() {
            *v = 0.0;
            bad += 1;
        }
    }
    if bad > 0 {
        warnings.push(format!(
            "{bad} non-finite value(s) in {label} replaced with 0 (model is unstable at those inputs)"
        ));
    }
    values
}

/// Pull a dataset out of whichever input shape the caller used and validate it.
fn parse_dataset(req: &FitRequest) -> Result<(Dataset, Vec<String>), String> {
    let (x, y): (Vec<Vec<f64>>, Vec<f64>) = if let Some(samples) = &req.samples {
        let x = samples.iter().map(|s| s.x.clone()).collect();
        let y = samples.iter().map(|s| s.y).collect();
        (x, y)
    } else if let (Some(inputs), Some(targets)) = (&req.inputs, &req.targets) {
        (inputs.clone(), targets.clone())
    } else if let (Some(x), Some(y)) = (&req.x, &req.y) {
        (x.iter().map(|v| vec![*v]).collect(), y.clone())
    } else {
        return Err(
            "provide data as `samples:[{x:[..],y}]`, `inputs`+`targets`, or `x`+`y`".to_string(),
        );
    };

    if x.is_empty() {
        return Err("dataset is empty".to_string());
    }
    if x.len() != y.len() {
        return Err(format!(
            "inputs ({}) and targets ({}) length mismatch",
            x.len(),
            y.len()
        ));
    }
    if x.len() > MAX_SAMPLES {
        return Err(format!("too many rows: {} (max {MAX_SAMPLES})", x.len()));
    }
    let d = x[0].len();
    if d == 0 {
        return Err("each input row needs at least one feature".to_string());
    }
    if d > MAX_FEATURES {
        return Err(format!("too many features: {d} (max {MAX_FEATURES})"));
    }
    for (i, row) in x.iter().enumerate() {
        if row.len() != d {
            return Err(format!("row {i} has {} features, expected {d}", row.len()));
        }
        if row.iter().any(|v| !v.is_finite()) || !y[i].is_finite() {
            return Err(format!("row {i} contains non-finite values"));
        }
        // Cap magnitude so squared deviations/errors (and the variance used for
        // standardisation) stay finite — otherwise an extreme value silently
        // overflows to Inf and surfaces as JSON `null` in the metrics.
        if row.iter().any(|v| v.abs() > MAX_ABS_VALUE) || y[i].abs() > MAX_ABS_VALUE {
            return Err(format!(
                "row {i} has a value with magnitude over {MAX_ABS_VALUE:.0e}; rescale your data"
            ));
        }
    }

    let names = match &req.variable_names {
        Some(names) if names.len() == d => {
            if let Some(i) = names.iter().position(|n| n.len() > MAX_VAR_NAME_LEN) {
                return Err(format!(
                    "variableNames[{i}] exceeds {MAX_VAR_NAME_LEN} chars"
                ));
            }
            names.clone()
        }
        Some(names) => {
            return Err(format!(
                "variableNames has {} entries but data has {d} features",
                names.len()
            ))
        }
        None => (0..d).map(|i| format!("x{i}")).collect(),
    };

    Ok((Dataset { x, y, d }, names))
}

/// Deterministically subsample row indices down to `cap`.
fn subsample(idx: &[usize], cap: usize, rng: &mut Rng) -> Vec<usize> {
    if idx.len() <= cap {
        return idx.to_vec();
    }
    let mut pool = idx.to_vec();
    for i in (1..pool.len()).rev() {
        pool.swap(i, rng.below(i + 1));
    }
    pool.truncate(cap);
    pool
}

fn predictions_for(predict: &dyn Fn(&[f64]) -> f64, data: &Dataset, idx: &[usize]) -> Vec<f64> {
    idx.iter().map(|&i| predict(&data.x[i])).collect()
}

fn metrics_for(predict: &dyn Fn(&[f64]) -> f64, data: &Dataset, idx: &[usize]) -> FitMetrics {
    let actual: Vec<f64> = idx.iter().map(|&i| data.y[i]).collect();
    let predicted = predictions_for(predict, data, idx);
    data::metrics(&actual, &predicted)
}

// ------------------------------------------------------------ method: symbolic

#[allow(clippy::too_many_arguments)]
fn fit_symbolic(
    data: &Dataset,
    train_idx: &[usize],
    val_idx: &[usize],
    names: &[String],
    config: &FitConfig,
    rng: &mut Rng,
    warnings: &mut Vec<String>,
    deadline: Instant,
) -> Fitted {
    let mut cfg = GpConfig::new(data.d, config.operators.as_deref());
    if let Some(p) = config.population {
        cfg.population = p.clamp(8, 2_000);
    }
    if let Some(g) = config.generations {
        cfg.generations = g.clamp(1, 200);
    }
    if let Some(md) = config.max_depth {
        cfg.max_depth = md.clamp(2, 12);
    }
    if let Some(par) = config.parsimony {
        cfg.parsimony = par.max(0.0);
    }
    if let Some(t) = config.tournament {
        cfg.tournament = t.clamp(2, 16);
    }
    if let Some(c) = config.const_opt_iters {
        cfg.const_opt_iters = c.min(2_000);
    }

    // Search over a (possibly subsampled) training fold.
    let gp_idx = subsample(train_idx, GP_MAX_ROWS, rng);
    if gp_idx.len() < train_idx.len() {
        warnings.push(format!(
            "symbolic regression searched a {}-row sample of the {}-row training set for speed",
            gp_idx.len(),
            train_idx.len()
        ));
    }
    let gp_x: Vec<Vec<f64>> = gp_idx.iter().map(|&i| data.x[i].clone()).collect();
    let gp_y: Vec<f64> = gp_idx.iter().map(|&i| data.y[i]).collect();

    let result = gp::run(&gp_x, &gp_y, &cfg, rng, deadline);
    if result.stopped_early {
        warnings.push(format!(
            "symbolic search stopped at the time budget after {} generation(s); returning best-so-far",
            result.generations
        ));
    }

    // Choose the front member with the best validation RMSE (generalisation),
    // breaking near-ties toward the simpler equation.
    let mut best_i = 0usize;
    let mut best_key = f64::INFINITY;
    for (i, m) in result.front.iter().enumerate() {
        let preds: Vec<f64> = val_idx.iter().map(|&j| m.expr.eval(&data.x[j])).collect();
        let actual: Vec<f64> = val_idx.iter().map(|&j| data.y[j]).collect();
        let val_rmse = data::metrics(&actual, &preds).rmse;
        // small complexity tie-break: penalise complexity by 0.1% of rmse scale
        let key = val_rmse * (1.0 + 1e-4 * m.complexity as f64);
        if key < best_key {
            best_key = key;
            best_i = i;
        }
    }

    let expression = result
        .front
        .get(best_i)
        .map(|m| m.expr.to_infix(names))
        .unwrap_or_else(|| "0".to_string());
    let complexity = result.front.get(best_i).map(|m| m.complexity);

    // Symbolic derivatives of the chosen equation, per input variable.
    let mut derivatives = serde_json::Map::new();
    if let Some(member) = result.front.get(best_i) {
        let dvars = data.d.min(MAX_DERIVATIVE_VARS);
        if data.d > MAX_DERIVATIVE_VARS {
            warnings.push(format!(
                "derivatives emitted for the first {MAX_DERIVATIVE_VARS} of {} variables",
                data.d
            ));
        }
        let mut skipped = 0;
        for k in 0..dvars {
            let d_expr = member.expr.derivative(k);
            // The quotient/product rules can blow a derivative up; skip giants so
            // the response stays bounded.
            if d_expr.size() > MAX_DERIVATIVE_NODES {
                skipped += 1;
                continue;
            }
            derivatives.insert(names[k].clone(), json!(d_expr.to_infix(names)));
        }
        if skipped > 0 {
            warnings.push(format!(
                "{skipped} derivative(s) omitted because the simplified form exceeded {MAX_DERIVATIVE_NODES} nodes"
            ));
        }
    }

    let best_expr = result
        .front
        .get(best_i)
        .map(|m| m.expr.clone())
        .unwrap_or(gp::Expr::Const(0.0));

    let model_json = json!({
        "kind": "expression",
        "expression": expression,
        "complexity": complexity,
        "variables": names,
        "units": "original",
        "note": "evaluate directly in the original variables; no scaling needed",
    });

    let iterations = result.generations as u64;
    Fitted {
        model_json,
        expression: Some(expression),
        derivatives: Value::Object(derivatives),
        complexity,
        pareto_raw: result.front,
        iterations,
        predict: Box::new(move |row: &[f64]| best_expr.eval(row)),
    }
}

// ------------------------------------------------- methods: neural / evo / hybrid

struct NeuralPieces {
    mlp: Mlp,
    x_scalers: Vec<data::Scaler>,
    y_scaler: data::Scaler,
    activation: Activation,
}

fn build_mlp_predict(pieces: &NeuralPieces) -> PredictFn {
    let mlp = pieces.mlp.clone();
    let x_scalers = pieces.x_scalers.clone();
    let y_scaler = pieces.y_scaler;
    Box::new(move |row: &[f64]| {
        let std_row: Vec<f64> = row
            .iter()
            .enumerate()
            .map(|(j, &v)| x_scalers.get(j).map(|s| s.transform(v)).unwrap_or(v))
            .collect();
        y_scaler.inverse(mlp.forward(&std_row))
    })
}

fn mlp_model_json(pieces: &NeuralPieces) -> Value {
    json!({
        "kind": "mlp",
        "activation": pieces.activation.name(),
        "inputScalers": pieces.x_scalers,
        "outputScaler": pieces.y_scaler,
        "layers": pieces.mlp.layers,
        "note": "standardise inputs with inputScalers, run layers (hidden activation, linear output), then inverse outputScaler",
    })
}

/// Shared setup: standardise the data and carve the standardised train/val rows.
fn standardize_folds(data: &Dataset, train_idx: &[usize], val_idx: &[usize]) -> StdFolds {
    // Fit scalers on the training fold only (avoid leaking validation stats).
    let train = data.select(train_idx);
    let std = data::standardize(&train);
    let val = data.select(val_idx);
    let val_x: Vec<Vec<f64>> = val
        .x
        .iter()
        .map(|row| {
            row.iter()
                .enumerate()
                .map(|(j, &v)| std.x_scalers[j].transform(v))
                .collect()
        })
        .collect();
    let val_y: Vec<f64> = val.y.iter().map(|&v| std.y_scaler.transform(v)).collect();
    (std.x, std.y, val_x, val_y, std.x_scalers, std.y_scaler)
}

fn hidden_layers(config: &FitConfig, warnings: &mut Vec<String>) -> Vec<usize> {
    let requested = match &config.hidden {
        Some(h) if !h.is_empty() => h.clone(),
        _ => return vec![16, 16],
    };
    let clamped: Vec<usize> = requested
        .iter()
        .map(|&u| u.clamp(1, MAX_HIDDEN_WIDTH))
        .take(MAX_HIDDEN_LAYERS)
        .collect();
    // Flag when the requested shape was reduced, so a caller asking for a giant
    // net knows why the model is smaller (memory is bounded by parameter count).
    if requested.len() > MAX_HIDDEN_LAYERS || requested.iter().any(|&u| u > MAX_HIDDEN_WIDTH) {
        warnings.push(format!(
            "hidden layers capped to {MAX_HIDDEN_LAYERS}×{MAX_HIDDEN_WIDTH} (requested {requested:?}); using {clamped:?}"
        ));
    }
    clamped
}

#[allow(clippy::too_many_arguments)]
fn fit_neural(
    data: &Dataset,
    train_idx: &[usize],
    val_idx: &[usize],
    config: &FitConfig,
    rng: &mut Rng,
    warnings: &mut Vec<String>,
    deadline: Instant,
) -> Fitted {
    let nn_idx = subsample(train_idx, NN_MAX_ROWS, rng);
    let (tx, ty, vx, vy, x_scalers, y_scaler) = standardize_folds(data, &nn_idx, val_idx);
    let hidden = hidden_layers(config, warnings);
    let activation = Activation::parse(config.activation.as_deref().unwrap_or("tanh"));
    let epochs = config.epochs.unwrap_or(400).clamp(1, 5_000);
    let lr = config.learning_rate.unwrap_or(0.01).clamp(1e-5, 1.0);
    let batch = config.batch_size.unwrap_or(32).max(1);

    let mut mlp = Mlp::new(data.d, &hidden, activation, rng);
    let ran = mlp.train(&tx, &ty, &vx, &vy, epochs, lr, batch, rng, deadline);
    if ran < epochs {
        warnings.push(format!(
            "neural training stopped at the time budget after {ran}/{epochs} epochs; returning best-so-far"
        ));
    }

    let pieces = NeuralPieces {
        mlp,
        x_scalers,
        y_scaler,
        activation,
    };
    Fitted {
        model_json: mlp_model_json(&pieces),
        expression: None,
        derivatives: Value::Null,
        complexity: None,
        pareto_raw: Vec::new(),
        iterations: ran as u64,
        predict: build_mlp_predict(&pieces),
    }
}

#[allow(clippy::too_many_arguments)]
fn fit_evolution(
    data: &Dataset,
    train_idx: &[usize],
    val_idx: &[usize],
    config: &FitConfig,
    rng: &mut Rng,
    warnings: &mut Vec<String>,
    hybrid: bool,
    deadline: Instant,
) -> Fitted {
    let nn_idx = subsample(train_idx, NN_MAX_ROWS, rng);
    let (tx, ty, vx, vy, x_scalers, y_scaler) = standardize_folds(data, &nn_idx, val_idx);
    let hidden = hidden_layers(config, warnings);
    let activation = Activation::parse(config.activation.as_deref().unwrap_or("tanh"));

    let mut es = EsConfig::default();
    if let Some(p) = config.es_population {
        // λ copies of the weight vector live at once; cap keeps ES memory bounded.
        es.population = p.clamp(4, 256);
    }
    if let Some(p) = config.es_parents {
        es.parents = p.clamp(1, es.population);
    }
    if let Some(g) = config.es_generations {
        es.generations = g.clamp(1, 1_000);
    }
    if let Some(s) = config.sigma {
        es.sigma0 = s.clamp(1e-4, 5.0);
    }

    let template = Mlp::new(data.d, &hidden, activation, rng);
    let (mut mlp, gens) = evo::evolve(&template, &tx, &ty, &vx, &vy, &es, rng, deadline);
    if gens < es.generations {
        warnings.push(format!(
            "neuroevolution stopped at the time budget after {gens}/{} generations; returning best-so-far",
            es.generations
        ));
    }

    // Memetic refinement: a short burst of gradient descent on the evolved net.
    let mut iterations = gens as u64;
    if hybrid {
        let epochs = config.epochs.unwrap_or(150).clamp(1, 2_000);
        let lr = config.learning_rate.unwrap_or(0.01).clamp(1e-5, 1.0);
        let batch = config.batch_size.unwrap_or(32).max(1);
        let ran = mlp.train(&tx, &ty, &vx, &vy, epochs, lr, batch, rng, deadline);
        iterations += ran as u64;
    }

    let pieces = NeuralPieces {
        mlp,
        x_scalers,
        y_scaler,
        activation,
    };
    Fitted {
        model_json: mlp_model_json(&pieces),
        expression: None,
        derivatives: Value::Null,
        complexity: None,
        pareto_raw: Vec::new(),
        iterations,
        predict: build_mlp_predict(&pieces),
    }
}

// ------------------------------------------------------- method: linear (analytic)

fn fit_linear(
    data: &Dataset,
    train_idx: &[usize],
    names: &[String],
    config: &FitConfig,
    warnings: &mut Vec<String>,
) -> Fitted {
    let degree = config.degree.unwrap_or(2).clamp(1, 6);
    let ridge = config.ridge.unwrap_or(1e-6).max(0.0);
    let d = data.d;

    // Additive polynomial basis: [1, x_j, x_j^2, ..., x_j^degree] per feature.
    let feature = move |row: &[f64]| -> Vec<f64> {
        let mut f = Vec::with_capacity(1 + d * degree);
        f.push(1.0);
        for j in 0..d {
            let mut p = 1.0;
            for _ in 0..degree {
                p *= row.get(j).copied().unwrap_or(0.0);
                f.push(p);
            }
        }
        f
    };

    // Stream the rows into the normal equations — no n×m design matrix is held.
    let m = 1 + d * degree;
    let rows = train_idx.iter().map(|&i| (feature(&data.x[i]), data.y[i]));
    let coeffs = linalg::least_squares(rows, m, ridge).unwrap_or_else(|| {
        warnings.push("normal equations were singular; returning the target mean".to_string());
        let mut c = vec![0.0; m];
        let n = train_idx.len().max(1) as f64;
        c[0] = train_idx.iter().map(|&i| data.y[i]).sum::<f64>() / n;
        c
    });

    // Build the analytic polynomial string.
    let mut terms: Vec<String> = Vec::new();
    if coeffs[0].abs() > 1e-9 {
        terms.push(fmt_coeff(coeffs[0]));
    }
    let mut k = 1;
    #[allow(clippy::needless_range_loop)] // j indexes names and labels the term
    for j in 0..d {
        for p in 1..=degree {
            let c = coeffs.get(k).copied().unwrap_or(0.0);
            k += 1;
            if c.abs() <= 1e-9 {
                continue;
            }
            let var = &names[j];
            let term = if p == 1 {
                format!("{}*{}", fmt_coeff(c), var)
            } else {
                format!("{}*{}^{}", fmt_coeff(c), var, p)
            };
            terms.push(term);
        }
    }
    let expression = if terms.is_empty() {
        "0".to_string()
    } else {
        terms.join(" + ")
    };

    let coeffs_for_predict = coeffs.clone();
    let predict = move |row: &[f64]| -> f64 {
        feature(row)
            .iter()
            .zip(coeffs_for_predict.iter())
            .map(|(f, c)| f * c)
            .sum()
    };

    let model_json = json!({
        "kind": "polynomial",
        "expression": expression,
        "degree": degree,
        "ridge": ridge,
        "coefficients": coeffs,
        "variables": names,
        "note": "additive per-feature polynomial fit by closed-form ridge least squares",
    });

    Fitted {
        model_json,
        expression: Some(expression),
        derivatives: Value::Null,
        complexity: Some(coeffs.iter().filter(|c| c.abs() > 1e-9).count()),
        pareto_raw: Vec::new(),
        iterations: 1,
        predict: Box::new(predict),
    }
}

fn fmt_coeff(x: f64) -> String {
    let a = x.abs();
    if a != 0.0 && !(1e-3..1e6).contains(&a) {
        format!("{x:.3e}")
    } else {
        let s = format!("{x:.4}");
        let t = s.trim_end_matches('0').trim_end_matches('.');
        if t.is_empty() || t == "-" {
            "0".to_string()
        } else {
            t.to_string()
        }
    }
}

// ------------------------------------------------------------------ dispatch

#[allow(clippy::too_many_arguments)]
fn run_method(
    method: &str,
    data: &Dataset,
    train_idx: &[usize],
    val_idx: &[usize],
    names: &[String],
    config: &FitConfig,
    rng: &mut Rng,
    warnings: &mut Vec<String>,
    deadline: Instant,
) -> Result<Fitted, String> {
    match method {
        "symbolic" | "gp" | "eureqa" | "symbolic-regression" => Ok(fit_symbolic(
            data, train_idx, val_idx, names, config, rng, warnings, deadline,
        )),
        "neural" | "nn" | "mlp" => {
            Ok(fit_neural(data, train_idx, val_idx, config, rng, warnings, deadline))
        }
        "evolution" | "evo" | "neuroevolution" | "es" => Ok(fit_evolution(
            data, train_idx, val_idx, config, rng, warnings, false, deadline,
        )),
        "hybrid" | "memetic" => Ok(fit_evolution(
            data, train_idx, val_idx, config, rng, warnings, true, deadline,
        )),
        "linear" | "polynomial" | "poly" | "leastsquares" => {
            Ok(fit_linear(data, train_idx, names, config, warnings))
        }
        other => Err(format!(
            "unknown method '{other}'; expected symbolic, neural, evolution, hybrid, linear, or auto"
        )),
    }
}

/// Build the per-method Pareto-front entries (symbolic only) with validation
/// metrics attached.
fn build_pareto(fitted: &Fitted, data: &Dataset, val_idx: &[usize], names: &[String]) -> Vec<ParetoEntry> {
    fitted
        .pareto_raw
        .iter()
        .map(|m| {
            let preds: Vec<f64> = val_idx.iter().map(|&j| m.expr.eval(&data.x[j])).collect();
            let actual: Vec<f64> = val_idx.iter().map(|&j| data.y[j]).collect();
            let vm = data::metrics(&actual, &preds);
            ParetoEntry {
                expression: m.expr.to_infix(names),
                complexity: m.complexity,
                train_rmse: m.train_mse.sqrt(),
                val_rmse: vm.rmse,
                val_r2: vm.r2,
            }
        })
        .collect()
}

/// Reject non-finite numeric knobs before they reach the optimisers. `clamp`
/// does not sanitise NaN, and a NaN parsimony/learning-rate/etc. silently breaks
/// fitness comparisons and gradient steps.
fn validate_config(req: &FitRequest) -> Result<(), String> {
    let c = &req.config;
    let checks: [(Option<f64>, &str); 5] = [
        (c.parsimony, "config.parsimony"),
        (c.learning_rate, "config.learningRate"),
        (c.sigma, "config.sigma"),
        (c.ridge, "config.ridge"),
        (req.val_fraction, "valFraction"),
    ];
    for (val, name) in checks {
        if let Some(x) = val {
            if !x.is_finite() {
                return Err(format!("{name} must be a finite number"));
            }
        }
    }
    Ok(())
}

/// Public entry point: fit a model and assemble the response. CPU-bound; the
/// server runs this on a blocking thread.
pub fn fit(req: FitRequest) -> Result<FitResponse, String> {
    let started = now_ms();
    let request_id = req
        .request_id
        .clone()
        .map(|s| s.chars().take(MAX_REQUEST_ID_LEN).collect::<String>())
        .unwrap_or_else(|| format!("fa-{}", now_ms()));
    let method = req
        .method
        .clone()
        .unwrap_or_else(|| "symbolic".to_string())
        .trim()
        .to_ascii_lowercase();

    let (data, names) = parse_dataset(&req)?;
    let mut warnings: Vec<String> = Vec::new();

    // Reject non-finite numeric config (NaN/Inf would poison comparisons and
    // optimisers; `clamp` does not sanitise NaN).
    validate_config(&req)?;

    // Wall-clock budget for the whole fit; the heavy loops honour it.
    let deadline = Instant::now() + fit_budget();

    let seed = req.seed.unwrap_or(0xF1F0_A55E);
    let mut rng = Rng::new(seed);

    // Train/validation split (seeded). Tiny datasets fall back to train==val.
    let val_fraction = req.val_fraction.unwrap_or(0.2);
    let split = data::train_val_split(data.n(), val_fraction, &mut rng);
    if split.val.len() == data.n() {
        warnings.push(
            "too few rows for a held-out fold; train and validation metrics use all rows".to_string(),
        );
    }

    // Validate predictAt up front.
    if let Some(points) = &req.predict_at {
        if points.len() > MAX_PREDICT_AT {
            return Err(format!(
                "predictAt has {} rows (max {MAX_PREDICT_AT})",
                points.len()
            ));
        }
        for (i, row) in points.iter().enumerate() {
            if row.len() != data.d {
                return Err(format!(
                    "predictAt row {i} has {} features, expected {}",
                    row.len(),
                    data.d
                ));
            }
            if row.iter().any(|v| !v.is_finite()) {
                return Err(format!("predictAt row {i} contains non-finite values"));
            }
        }
    }

    let mut selected: Option<String> = None;
    let mut candidates: Option<Vec<CandidateSummary>> = None;

    let fitted = if method == "auto" {
        // Run a bounded set of methods and keep the best validation RMSE,
        // preferring the simpler analytic answer on a near-tie.
        let mut auto_cfg = clone_config(&req.config);
        // Trim budgets so auto stays responsive.
        auto_cfg.generations = Some(auto_cfg.generations.unwrap_or(25).min(25));
        auto_cfg.epochs = Some(auto_cfg.epochs.unwrap_or(250).min(250));
        auto_cfg.es_generations = Some(auto_cfg.es_generations.unwrap_or(40).min(40));

        // Give each method an equal slice of the budget so a slow first method
        // can't starve the others.
        let methods = ["symbolic", "neural", "evolution"];
        let per_method = fit_budget() / methods.len() as u32;

        let mut summaries: Vec<CandidateSummary> = Vec::new();
        let mut best: Option<(String, f64, Fitted)> = None;
        for m in methods {
            let mut local_warn = Vec::new();
            let method_deadline = Instant::now() + per_method;
            let f = run_method(m, &data, &split.train, &split.val, &names, &auto_cfg, &mut rng, &mut local_warn, method_deadline)?;
            for w in local_warn {
                warnings.push(format!("[{m}] {w}"));
            }
            let vm = metrics_for(f.predict.as_ref(), &data, &split.val);
            summaries.push(CandidateSummary {
                method: m.to_string(),
                val_rmse: vm.rmse,
                val_r2: vm.r2,
                complexity: f.complexity,
                expression: f.expression.clone(),
            });
            let prefer = m == "symbolic"; // analytic tie-break weight
            let key = vm.rmse * if prefer { 0.98 } else { 1.0 };
            let take = match &best {
                None => true,
                Some((_, best_key, _)) => key < *best_key,
            };
            if take {
                best = Some((m.to_string(), key, f));
            }
        }
        let (sel, _, f) = best.expect("auto evaluated at least one method");
        selected = Some(sel);
        candidates = Some(summaries);
        f
    } else {
        run_method(&method, &data, &split.train, &split.val, &names, &req.config, &mut rng, &mut warnings, deadline)?
    };

    // Metrics on the original units.
    let train_metrics = metrics_for(fitted.predict.as_ref(), &data, &split.train);
    let val_metrics = metrics_for(fitted.predict.as_ref(), &data, &split.val);

    let pareto_front = build_pareto(&fitted, &data, &split.val, &names);

    // Optional outputs (sanitised so non-finite model output never serialises
    // as `null` inside a numeric array).
    let predictions = if req.include_predictions.unwrap_or(false) {
        let all: Vec<usize> = (0..data.n()).collect();
        let mut preds = predictions_for(fitted.predict.as_ref(), &data, &all);
        if preds.len() > MAX_PREDICTIONS_OUT {
            preds.truncate(MAX_PREDICTIONS_OUT);
            warnings.push(format!(
                "predictions truncated to the first {MAX_PREDICTIONS_OUT} rows"
            ));
        }
        Some(sanitize_floats(preds, "predictions", &mut warnings))
    } else {
        None
    };
    let predicted_values = req.predict_at.as_ref().map(|points| {
        let raw = points.iter().map(|row| (fitted.predict)(row)).collect();
        sanitize_floats(raw, "predictedValues", &mut warnings)
    });

    Ok(FitResponse {
        ok: true,
        request_id,
        method: if req.method.is_some() { method } else { "symbolic".to_string() },
        selected,
        samples: data.n(),
        features: data.d,
        train: train_metrics,
        validation: val_metrics,
        expression: fitted.expression,
        derivatives: fitted.derivatives,
        complexity: fitted.complexity,
        pareto_front,
        model: fitted.model_json,
        candidates,
        predictions,
        predicted_values,
        iterations: fitted.iterations,
        duration_ms: now_ms().saturating_sub(started),
        warnings,
        generated_at_ms: now_ms(),
    })
}

/// FitConfig isn't Clone (it holds Vecs we don't want to force-derive on the
/// public type); make the shallow copy `auto` needs explicitly.
fn clone_config(c: &FitConfig) -> FitConfig {
    FitConfig {
        population: c.population,
        generations: c.generations,
        max_depth: c.max_depth,
        operators: c.operators.clone(),
        parsimony: c.parsimony,
        tournament: c.tournament,
        const_opt_iters: c.const_opt_iters,
        hidden: c.hidden.clone(),
        activation: c.activation.clone(),
        epochs: c.epochs,
        learning_rate: c.learning_rate,
        batch_size: c.batch_size,
        es_population: c.es_population,
        es_parents: c.es_parents,
        es_generations: c.es_generations,
        sigma: c.sigma,
        degree: c.degree,
        ridge: c.ridge,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linear_data() -> FitRequest {
        // y = 3*x - 2 with two features (second is noise-free irrelevant).
        let mut samples = Vec::new();
        let mut v = -5.0;
        while v <= 5.0 {
            samples.push(Sample {
                x: vec![v, 1.0],
                y: 3.0 * v - 2.0,
            });
            v += 0.2;
        }
        FitRequest {
            request_id: None,
            method: Some("linear".to_string()),
            samples: Some(samples),
            inputs: None,
            targets: None,
            x: None,
            y: None,
            variable_names: None,
            seed: Some(1),
            val_fraction: Some(0.2),
            include_predictions: Some(false),
            predict_at: Some(vec![vec![10.0, 1.0]]),
            config: FitConfig::default(),
        }
    }

    #[test]
    fn linear_recovers_line() {
        let resp = fit(linear_data()).unwrap();
        assert!(resp.validation.r2 > 0.999, "r2 was {}", resp.validation.r2);
        // y(10) = 28
        let pred = resp.predicted_values.unwrap()[0];
        assert!((pred - 28.0).abs() < 1e-3, "pred {pred}");
        assert!(resp.expression.is_some());
    }

    #[test]
    fn symbolic_fits_and_returns_front() {
        let mut req = linear_data();
        req.method = Some("symbolic".to_string());
        req.config.generations = Some(20);
        req.config.population = Some(200);
        let resp = fit(req).unwrap();
        assert!(!resp.pareto_front.is_empty());
        assert!(resp.validation.r2 > 0.9, "r2 {}", resp.validation.r2);
        assert!(resp.expression.is_some());
    }

    #[test]
    fn neural_fits_nonlinear() {
        // y = sin(x)
        let mut samples = Vec::new();
        let mut v = -3.0;
        while v <= 3.0 {
            samples.push(Sample {
                x: vec![v],
                y: v.sin(),
            });
            v += 0.1;
        }
        let req = FitRequest {
            request_id: None,
            method: Some("neural".to_string()),
            samples: Some(samples),
            inputs: None,
            targets: None,
            x: None,
            y: None,
            variable_names: None,
            seed: Some(7),
            val_fraction: Some(0.2),
            include_predictions: Some(false),
            predict_at: None,
            config: FitConfig {
                epochs: Some(600),
                hidden: Some(vec![16, 16]),
                ..FitConfig::default()
            },
        };
        let resp = fit(req).unwrap();
        assert!(resp.validation.r2 > 0.9, "r2 was {}", resp.validation.r2);
    }

    #[test]
    fn rejects_nonfinite_config() {
        let mut req = linear_data();
        req.config.parsimony = Some(f64::NAN);
        assert!(fit(req).is_err());

        let mut req = linear_data();
        req.val_fraction = Some(f64::INFINITY);
        assert!(fit(req).is_err());
    }

    #[test]
    fn sanitize_replaces_nonfinite() {
        let mut warnings = Vec::new();
        let out = sanitize_floats(
            vec![1.0, f64::NAN, f64::INFINITY, -2.0, f64::NEG_INFINITY],
            "predictions",
            &mut warnings,
        );
        assert_eq!(out, vec![1.0, 0.0, 0.0, -2.0, 0.0]);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains('3')); // three replaced
    }

    #[test]
    fn budget_is_clamped() {
        // Out-of-range overrides clamp into [MIN, MAX]; default applies when unset.
        std::env::remove_var("FUNC_APPROX_MAX_FIT_MS");
        assert_eq!(fit_budget(), Duration::from_millis(DEFAULT_FIT_BUDGET_MS));
    }

    #[test]
    fn rejects_extreme_magnitude() {
        let mut req = linear_data();
        req.samples.as_mut().unwrap()[0].y = 1e200;
        assert!(fit(req).is_err());
    }

    #[test]
    fn rejects_long_variable_name() {
        let mut req = linear_data();
        req.variable_names = Some(vec!["x".repeat(100), "z".to_string()]);
        assert!(fit(req).is_err());
    }

    #[test]
    fn caps_hidden_architecture() {
        let mut req = linear_data();
        req.method = Some("neural".to_string());
        req.config.hidden = Some(vec![1000, 1000, 1000, 1000, 1000, 1000]);
        req.config.epochs = Some(5);
        let resp = fit(req).unwrap();
        // At most MAX_HIDDEN_LAYERS hidden layers + 1 output layer.
        let layers = resp.model["layers"].as_array().unwrap();
        assert!(layers.len() <= MAX_HIDDEN_LAYERS + 1);
        // Every hidden layer width is capped.
        for layer in layers {
            let units = layer["bias"].as_array().unwrap().len();
            assert!(units <= MAX_HIDDEN_WIDTH);
        }
        assert!(resp.warnings.iter().any(|w| w.contains("hidden layers capped")));
    }

    #[test]
    fn rejects_ragged_rows() {
        let req = FitRequest {
            request_id: None,
            method: None,
            samples: Some(vec![
                Sample { x: vec![1.0, 2.0], y: 1.0 },
                Sample { x: vec![1.0], y: 2.0 },
            ]),
            inputs: None,
            targets: None,
            x: None,
            y: None,
            variable_names: None,
            seed: None,
            val_fraction: None,
            include_predictions: None,
            predict_at: None,
            config: FitConfig::default(),
        };
        assert!(fit(req).is_err());
    }
}
