//! Dataset handling: parsing-agnostic container, per-column standardisation,
//! a seeded train/validation split, and honest regression metrics (RMSE, MAE,
//! R²) computed on the original units.

use serde::Serialize;

use crate::rng::Rng;

/// A regression dataset: `n` rows of `d` input features mapped to a scalar target.
#[derive(Clone)]
pub struct Dataset {
    /// Row-major inputs, `n` × `d`.
    pub x: Vec<Vec<f64>>,
    /// Targets, length `n`.
    pub y: Vec<f64>,
    /// Feature dimension.
    pub d: usize,
}

impl Dataset {
    pub fn n(&self) -> usize {
        self.y.len()
    }

    /// Materialise a sub-view from a set of row indices (used for train/val folds).
    pub fn select(&self, idx: &[usize]) -> Dataset {
        Dataset {
            x: idx.iter().map(|&i| self.x[i].clone()).collect(),
            y: idx.iter().map(|&i| self.y[i]).collect(),
            d: self.d,
        }
    }
}

/// Affine standardiser for a single channel: `(v - mean) / std`.
#[derive(Clone, Copy, Serialize)]
pub struct Scaler {
    pub mean: f64,
    pub std: f64,
}

impl Scaler {
    /// Fit mean/std over an iterator; a (near) zero standard deviation collapses
    /// to 1.0 so a constant column maps to 0 instead of producing NaNs.
    pub fn fit<I: Iterator<Item = f64>>(values: I) -> Scaler {
        let collected: Vec<f64> = values.collect();
        let n = collected.len().max(1) as f64;
        let mean = collected.iter().sum::<f64>() / n;
        let var = collected.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / n;
        let std = var.sqrt();
        Scaler {
            mean,
            std: if std < 1e-9 { 1.0 } else { std },
        }
    }

    pub fn transform(&self, v: f64) -> f64 {
        (v - self.mean) / self.std
    }

    pub fn inverse(&self, v: f64) -> f64 {
        v * self.std + self.mean
    }
}

/// A standardised copy of a dataset plus the scalers needed to invert it. Used
/// by the gradient/evolution learners; symbolic regression fits raw units so its
/// equations stay in the caller's variables.
pub struct Standardized {
    pub x: Vec<Vec<f64>>,
    pub y: Vec<f64>,
    pub x_scalers: Vec<Scaler>,
    pub y_scaler: Scaler,
}

pub fn standardize(data: &Dataset) -> Standardized {
    let x_scalers: Vec<Scaler> = (0..data.d)
        .map(|j| Scaler::fit(data.x.iter().map(|row| row[j])))
        .collect();
    let y_scaler = Scaler::fit(data.y.iter().copied());
    let x = data
        .x
        .iter()
        .map(|row| {
            row.iter()
                .enumerate()
                .map(|(j, &v)| x_scalers[j].transform(v))
                .collect()
        })
        .collect();
    let y = data.y.iter().map(|&v| y_scaler.transform(v)).collect();
    Standardized {
        x,
        y,
        x_scalers,
        y_scaler,
    }
}

/// Row indices for a train/validation split.
pub struct Split {
    pub train: Vec<usize>,
    pub val: Vec<usize>,
}

/// Deterministic Fisher–Yates shuffle, then carve off `val_fraction` for
/// validation. With too few rows to spare any, validation falls back to the
/// training rows so metrics are still defined (the caller is warned upstream).
pub fn train_val_split(n: usize, val_fraction: f64, rng: &mut Rng) -> Split {
    let mut idx: Vec<usize> = (0..n).collect();
    for i in (1..n).rev() {
        let j = rng.below(i + 1);
        idx.swap(i, j);
    }
    let frac = val_fraction.clamp(0.0, 0.9);
    let val_count = ((n as f64) * frac).round() as usize;
    if val_count == 0 || val_count >= n {
        return Split {
            train: idx.clone(),
            val: idx,
        };
    }
    let val = idx[..val_count].to_vec();
    let train = idx[val_count..].to_vec();
    Split { train, val }
}

/// Regression goodness-of-fit on the original units.
#[derive(Clone, Copy, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FitMetrics {
    pub rmse: f64,
    pub mae: f64,
    pub r2: f64,
}

/// Compute RMSE / MAE / R² for aligned actual/predicted vectors. Non-finite
/// predictions are treated as a worst-case large residual so a blown-up model
/// can't masquerade as a good fit via NaN arithmetic.
pub fn metrics(actual: &[f64], predicted: &[f64]) -> FitMetrics {
    let n = actual.len();
    if n == 0 || predicted.len() != n {
        return FitMetrics::default();
    }
    let mean = actual.iter().sum::<f64>() / n as f64;
    let mut sse = 0.0;
    let mut sae = 0.0;
    let mut sst = 0.0;
    for i in 0..n {
        let p = if predicted[i].is_finite() {
            predicted[i]
        } else {
            // Penalise non-finite output rather than poisoning the sums.
            actual[i] + 1e6
        };
        let err = actual[i] - p;
        sse += err * err;
        sae += err.abs();
        sst += (actual[i] - mean) * (actual[i] - mean);
    }
    let rmse = (sse / n as f64).sqrt();
    let mae = sae / n as f64;
    let r2 = if sst < 1e-12 {
        // Target is (nearly) constant: define R² as 1 for a perfect fit, else 0.
        if sse < 1e-12 {
            1.0
        } else {
            0.0
        }
    } else {
        1.0 - sse / sst
    };
    FitMetrics { rmse, mae, r2 }
}
