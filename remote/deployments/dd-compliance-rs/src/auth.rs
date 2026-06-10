use axum::{
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

use crate::{config::Config, metrics::Metrics};

pub enum AuthFailure {
    MissingSecret,
    Unauthorized,
}

pub fn require_auth(
    headers: &HeaderMap,
    config: &Config,
    metrics: &Metrics,
) -> Result<(), Response> {
    if config.allow_unauthenticated {
        return Ok(());
    }
    let Some(secret) = config.server_auth_secret.as_deref() else {
        metrics.auth_failures_total.fetch_add(1);
        return Err(auth_failure_response(AuthFailure::MissingSecret));
    };
    let provided = headers
        .get("x-server-auth")
        .or_else(|| headers.get("x-compliance-auth"))
        .or_else(|| headers.get("auth"))
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if constant_time_eq(provided, secret) {
        Ok(())
    } else {
        metrics.auth_failures_total.fetch_add(1);
        Err(auth_failure_response(AuthFailure::Unauthorized))
    }
}

fn auth_failure_response(failure: AuthFailure) -> Response {
    let (status, message) = match failure {
        AuthFailure::MissingSecret => (
            StatusCode::SERVICE_UNAVAILABLE,
            "SERVER_AUTH_SECRET is not configured",
        ),
        AuthFailure::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
    };
    (status, Json(json!({ "ok": false, "error": message }))).into_response()
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_checks_exact_match() {
        assert!(constant_time_eq("secret", "secret"));
        assert!(!constant_time_eq("secret", "Secret"));
        assert!(!constant_time_eq("secret", "secret1"));
    }
}
