//! Input validation and small security primitives shared by the handlers.

use crate::config::Limits;
use crate::error::ApiError;

/// Constant-time byte-slice equality, to keep the bearer-token check from
/// leaking length-prefix matches through response timing. Returns early only
/// on a length mismatch (token length is not a meaningful secret).
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Enforce batch/size guardrails on a set of input texts. Runs before any
/// upstream call so an oversized request is rejected for free.
pub fn enforce_input_limits(inputs: &[String], limits: &Limits) -> Result<(), ApiError> {
    if inputs.is_empty() || inputs.iter().all(|s| s.trim().is_empty()) {
        return Err(ApiError::Invalid("input must contain at least one non-empty string".into()));
    }
    if inputs.len() > limits.max_batch_size {
        return Err(ApiError::Invalid(format!(
            "batch size {} exceeds limit of {}",
            inputs.len(),
            limits.max_batch_size
        )));
    }
    let mut total = 0usize;
    for s in inputs {
        let len = s.chars().count();
        if len > limits.max_item_chars {
            return Err(ApiError::Invalid(format!(
                "an input of {} chars exceeds the per-item limit of {}",
                len, limits.max_item_chars
            )));
        }
        total = total.saturating_add(len);
    }
    if total > limits.max_total_chars {
        return Err(ApiError::Invalid(format!(
            "total input of {} chars exceeds the limit of {}",
            total, limits.max_total_chars
        )));
    }
    Ok(())
}

/// Validate a requested embedding dimensionality against the cap.
pub fn check_dimensions(dimensions: Option<u32>, limits: &Limits) -> Result<(), ApiError> {
    match dimensions {
        Some(0) => Err(ApiError::Invalid("dimensions must be > 0".into())),
        Some(d) if d > limits.max_dimensions => Err(ApiError::Invalid(format!(
            "dimensions {} exceeds the limit of {}",
            d, limits.max_dimensions
        ))),
        _ => Ok(()),
    }
}

/// Clamp a requested top_k into `[1, max_top_k]`.
pub fn clamp_top_k(top_k: usize, limits: &Limits) -> usize {
    top_k.clamp(1, limits.max_top_k)
}

/// Validate the Qdrant distance metric. Body-only (no injection risk), but a
/// clean 400 beats a downstream 502 for an obvious typo.
pub fn validate_distance(distance: &str) -> Result<(), ApiError> {
    match distance {
        "Cosine" | "Dot" | "Euclid" | "Manhattan" => Ok(()),
        other => Err(ApiError::Invalid(format!(
            "unknown distance `{other}`; expected one of Cosine, Dot, Euclid, Manhattan"
        ))),
    }
}

/// Validate a Qdrant collection name before it is interpolated into a REST
/// path. Restricting the charset prevents path traversal (`..`), query-string
/// injection (`?`, `#`), and slash-based path escapes. Mirrors Qdrant's own
/// accepted-name shape.
pub fn validate_collection(name: &str) -> Result<(), ApiError> {
    const MAX: usize = 255;
    if name.is_empty() || name.len() > MAX {
        return Err(ApiError::Invalid(format!(
            "collection name must be 1..={MAX} characters"
        )));
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')) {
        return Err(ApiError::Invalid(
            "collection name may contain only [A-Za-z0-9._-]".into(),
        ));
    }
    // Defense in depth even though '/' is already excluded above.
    if name.contains("..") {
        return Err(ApiError::Invalid("collection name may not contain `..`".into()));
    }
    Ok(())
}
