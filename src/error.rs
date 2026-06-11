//! Coarse API error type. Deliberately leaks nothing — no account-existence,
//! no crypto detail — to a caller (see `ProtocolError` in `shared`).

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("unauthorized")]
    Unauthorized,
    // Vault push conflicts are returned as `PushResponse::Conflict` (200 body),
    // not an HTTP error; kept here for completeness of the error surface.
    #[allow(dead_code)]
    #[error("conflict")]
    Conflict,
    #[error("bad request")]
    BadRequest,
    #[error("internal error")]
    Internal,
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        tracing::error!(error = %e, "database error");
        ApiError::Internal
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let code = match self {
            ApiError::Unauthorized => StatusCode::UNAUTHORIZED,
            ApiError::Conflict => StatusCode::CONFLICT,
            ApiError::BadRequest => StatusCode::BAD_REQUEST,
            ApiError::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };
        // Body intentionally minimal.
        (code, self.to_string()).into_response()
    }
}
