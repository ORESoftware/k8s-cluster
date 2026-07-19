//! Service error type and its HTTP projection.
//!
//! Error responses deliberately carry coarse messages. Auth failures in
//! particular must not distinguish "unknown signing key" from "email not on the
//! allow-list" — that difference tells an attacker whether an identity exists.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

// This server is read-only, so there is no request-body validation and thus no
// BadRequest variant (unlike the API server). NotFound doubles as the "not
// yours" response so plan ownership cannot be probed.
#[derive(Debug)]
pub(crate) enum ServiceError {
    /// Missing, malformed, unverifiable, or non-permitted credentials.
    Unauthorized,
    NotFound,
    /// A dependency (database, identity provider) is unavailable. The detail is
    /// logged but only a generic message reaches the caller.
    Unavailable(String),
}

impl ServiceError {
    fn parts(&self) -> (StatusCode, &'static str) {
        match self {
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            Self::NotFound => (StatusCode::NOT_FOUND, "not_found"),
            Self::Unavailable(_) => (StatusCode::SERVICE_UNAVAILABLE, "service_unavailable"),
        }
    }
}

impl IntoResponse for ServiceError {
    fn into_response(self) -> Response {
        // Log the full detail server-side before discarding it for the client.
        if let Self::Unavailable(detail) = &self {
            tracing::warn!(error = %detail, "dependency unavailable");
        }
        let (status, code) = self.parts();
        (status, Json(json!({ "error": code }))).into_response()
    }
}

impl From<sea_orm::DbErr> for ServiceError {
    fn from(err: sea_orm::DbErr) -> Self {
        Self::Unavailable(format!("database error: {err}"))
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
        let err = ServiceError::Unavailable("postgres connection refused at 10.0.0.4".to_string());
        let (status, _, detail) = err.parts();
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(detail.is_none(), "internal topology must not reach callers");
    }

    #[test]
    fn bad_request_detail_is_returned() {
        let err = ServiceError::BadRequest("title must not be empty".to_string());
        let (status, _, detail) = err.parts();
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(detail, Some("title must not be empty"));
    }
}
