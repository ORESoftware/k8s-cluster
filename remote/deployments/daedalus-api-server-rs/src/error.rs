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

#[derive(Debug)]
pub(crate) enum ServiceError {
    /// Caller-supplied input failed validation. Safe to describe.
    BadRequest(String),
    /// Missing, malformed, unverifiable, or non-permitted credentials.
    Unauthorized,
    NotFound,
    /// A dependency (database, identity provider) is unavailable. The detail is
    /// logged but only a generic message reaches the caller.
    Unavailable(String),
    Internal(String),
}

impl ServiceError {
    fn parts(&self) -> (StatusCode, &'static str, Option<&str>) {
        match self {
            Self::BadRequest(detail) => (StatusCode::BAD_REQUEST, "bad_request", Some(detail)),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized", None),
            Self::NotFound => (StatusCode::NOT_FOUND, "not_found", None),
            Self::Unavailable(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "service_unavailable",
                None,
            ),
            Self::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal", None),
        }
    }
}

impl IntoResponse for ServiceError {
    fn into_response(self) -> Response {
        // Log the full detail server-side before discarding it for the client.
        match &self {
            Self::Unavailable(detail) => tracing::warn!(error = %detail, "dependency unavailable"),
            Self::Internal(detail) => tracing::error!(error = %detail, "internal error"),
            _ => {}
        }
        let (status, code, detail) = self.parts();
        let body = match detail {
            Some(detail) => json!({ "error": code, "detail": detail }),
            None => json!({ "error": code }),
        };
        (status, Json(body)).into_response()
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
