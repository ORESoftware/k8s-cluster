//! Service error type and its HTTP projection.
//!
//! Ported from daedalus-api-server so the two services reject callers
//! identically. Error responses deliberately carry coarse messages. Auth
//! failures in particular must not distinguish "unknown signing key" from
//! "email not on the allow-list" — that difference tells an attacker whether an
//! identity exists.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

#[derive(Debug)]
pub(crate) enum ServiceError {
    /// Missing, malformed, unverifiable, or non-permitted credentials.
    Unauthorized,
    /// A dependency (identity provider) is unavailable, or the auth gate is not
    /// configured at all. The detail is logged but only a generic message
    /// reaches the caller.
    Unavailable(String),
}

impl ServiceError {
    fn parts(&self) -> (StatusCode, &'static str, Option<&str>) {
        match self {
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized", None),
            Self::Unavailable(_) => (StatusCode::SERVICE_UNAVAILABLE, "service_unavailable", None),
        }
    }
}

impl IntoResponse for ServiceError {
    fn into_response(self) -> Response {
        // Log the full detail server-side before discarding it for the client.
        if let Self::Unavailable(detail) = &self {
            tracing::warn!(error = %detail, "dependency unavailable");
        }
        let (status, code, detail) = self.parts();
        let body = match detail {
            Some(detail) => json!({ "error": code, "detail": detail }),
            None => json!({ "error": code }),
        };
        (status, Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_failures_do_not_leak_a_reason() {
        let (status, code, detail) = ServiceError::Unauthorized.parts();
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(code, "unauthorized");
        // No detail: a caller must not learn whether the signature, the
        // audience, or the email allow-list rejected them.
        assert!(detail.is_none());
    }

    #[test]
    fn dependency_detail_is_withheld_from_the_client() {
        let err = ServiceError::Unavailable("supabase jwks fetch failed at 10.0.0.4".to_string());
        let (status, _, detail) = err.parts();
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(detail.is_none(), "internal topology must not reach callers");
    }
}
