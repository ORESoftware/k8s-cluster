//! Shared authentication helpers: constant-time secret comparison and the
//! server-auth middleware that guards operator + history endpoints.

use crate::state::AppState;
use axum::extract::State;
use axum::http::{header, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// Length-independent byte comparison. Returns false on length mismatch (the
/// length itself is not secret here) and otherwise compares every byte.
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn deny(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "ok": false, "error": message }))).into_response()
}

/// Require `Authorization: Bearer <T2V_SERVER_AUTH_SECRET>` on the request.
///
/// Fails **closed**: if no server-auth secret is configured, every protected
/// route returns 503 rather than silently running unauthenticated. A missing
/// or wrong token is 401.
pub async fn require_server_auth(
    State(state): State<AppState>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let Some(expected) = state.server_auth_secret.as_deref() else {
        return deny(
            StatusCode::SERVICE_UNAVAILABLE,
            "operator endpoints are disabled: T2V_SERVER_AUTH_SECRET is not configured",
        );
    };

    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim);

    match presented {
        Some(token) if constant_time_eq(token, expected) => next.run(request).await,
        _ => deny(StatusCode::UNAUTHORIZED, "invalid or missing bearer token"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_behaves_like_eq() {
        assert!(constant_time_eq("secret", "secret"));
        assert!(!constant_time_eq("secret", "secreT"));
        assert!(!constant_time_eq("secret", "secret-longer"));
        assert!(!constant_time_eq("", "x"));
        assert!(constant_time_eq("", ""));
    }
}
