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
        // A caller must not learn whether the signature, the audience, or the
        // email allow-list rejected them — every auth failure is one code.
        let (status, code) = ServiceError::Unauthorized.parts();
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(code, "unauthorized");
    }

    #[test]
    fn dependency_detail_is_withheld_from_the_client() {
        // The detail string carries internal topology; only the coarse code is
        // exposed. parts() never returns the detail.
        let err = ServiceError::Unavailable("postgres connection refused at 10.0.0.4".to_string());
        let (status, code) = err.parts();
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(code, "service_unavailable");
    }

    #[test]
    fn not_found_doubles_as_the_not_yours_response() {
        let (status, code) = ServiceError::NotFound.parts();
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(code, "not_found");
    }
}
