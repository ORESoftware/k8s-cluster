//! Error types. Every request-time rejection collapses to a small set of HTTP
//! responses; internal detail stays in logs so a caller cannot probe which
//! projects or users exist.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

/// Configuration failures (startup only).
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("missing required env var: {0}")]
    Missing(&'static str),
    #[error("invalid config value: {0}")]
    Invalid(&'static str),
}

/// Request-time errors.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// Any authentication failure — bad signature, unknown issuer, expired,
    /// wrong audience, malformed token. Deliberately uniform.
    #[error("unauthorized")]
    Unauthorized,
    /// Malformed request (missing bearer token, bad body).
    #[error("bad request: {0}")]
    BadRequest(&'static str),
    /// Upstream dependency failed (JWKS fetch, DB). Logged with detail.
    #[error("upstream unavailable")]
    Upstream,
    /// Programming/there-is-no-good-recovery errors.
    #[error("internal error")]
    Internal,
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, code) = match self {
            AuthError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            AuthError::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            AuthError::Upstream => (StatusCode::BAD_GATEWAY, "upstream_unavailable"),
            AuthError::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
        };
        // Log the real cause; return only the coarse code to the caller.
        if matches!(self, AuthError::Upstream | AuthError::Internal) {
            tracing::error!(error = %self, "request failed");
        }
        (status, Json(json!({ "error": code }))).into_response()
    }
}
