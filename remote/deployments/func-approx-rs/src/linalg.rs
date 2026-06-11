//! Closed-form linear algebra for the *analytic* fitters: ridge-regularised
//! least squares via the normal equations, solved with Gaussian elimination and
//! partial pivoting. No external BLAS — these systems are small (a handful of
//! basis columns), and an exact solve is what makes the linear/polynomial method
//! and the genetic-programming linear scaling analytic rather than iterative.

/// Solve `A x = b` for a square `n × n` system using partial pivoting.
/// Returns `None` if the matrix is singular to working precision.
pub fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    if n == 0 || a.len() != n || a.iter().any(|row| row.len() != n) {
        return None;
    }
    for col in 0..n {
        // Partial pivot: move the largest-magnitude row into place for stability.
        let mut pivot = col;
        let mut best = a[col][col].abs();
        for row in (col + 1)..n {
            let mag = a[row][col].abs();
            if mag > best {
                best = mag;
                pivot = row;
            }
        }
        if best < 1e-12 {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);

        let diag = a[col][col];
        for row in (col + 1)..n {
            let factor = a[row][col] / diag;
            if factor == 0.0 {
                continue;
            }
            for k in col..n {
                a[row][k] -= factor * a[col][k];
            }
            b[row] -= factor * b[col];
        }
    }

    // Back-substitution.
    let mut x = vec![0.0; n];
    for row in (0..n).rev() {
        let mut sum = b[row];
        for k in (row + 1)..n {
            sum -= a[row][k] * x[k];
        }
        x[row] = sum / a[row][row];
        if !x[row].is_finite() {
            return None;
        }
    }
    Some(x)
}

/// Ridge least squares: minimise `||design·w - y||² + lambda·||w||²`.
///
/// `design` is `n × m` (each row is the feature/basis vector for one sample).
/// Solves the `m × m` normal equations `(XᵀX + lambdaI) w = Xᵀy` exactly.
pub fn least_squares(design: &[Vec<f64>], y: &[f64], lambda: f64) -> Option<Vec<f64>> {
    let n = design.len();
    if n == 0 || y.len() != n {
        return None;
    }
    let m = design[0].len();
    if m == 0 || design.iter().any(|row| row.len() != m) {
        return None;
    }

    // Normal matrix XᵀX (+ ridge) and right-hand side Xᵀy.
    let mut ata = vec![vec![0.0; m]; m];
    let mut atb = vec![0.0; m];
    for (row, &target) in design.iter().zip(y.iter()) {
        for i in 0..m {
            atb[i] += row[i] * target;
            for j in 0..m {
                ata[i][j] += row[i] * row[j];
            }
        }
    }
    for i in 0..m {
        ata[i][i] += lambda;
    }
    solve(ata, atb)
}

/// Optimal affine rescaling `a·f + b` of a single predictor against the target,
/// in closed form (ordinary least squares on one feature plus intercept). This
/// is the Keijzer "linear scaling" used to make every genetic-programming
/// candidate hit its best possible slope/offset before fitness is measured.
///
/// Returns `(a, b)`. When `f` has no variance, slope collapses to 0 and `b`
/// becomes the target mean (the best constant predictor).
pub fn linear_scaling(f: &[f64], y: &[f64]) -> (f64, f64) {
    let n = f.len();
    if n == 0 || y.len() != n {
        return (0.0, 0.0);
    }
    let nf = n as f64;
    let mean_f = f.iter().sum::<f64>() / nf;
    let mean_y = y.iter().sum::<f64>() / nf;
    let mut cov = 0.0;
    let mut var = 0.0;
    for i in 0..n {
        let df = f[i] - mean_f;
        cov += df * (y[i] - mean_y);
        var += df * df;
    }
    if var < 1e-12 {
        return (0.0, mean_y);
    }
    let a = cov / var;
    let b = mean_y - a * mean_f;
    (a, b)
}
