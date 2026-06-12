//! Coarse API error type. Deliberately leaks nothing — no account-existence,
//! no crypto detail — to a caller (see `ProtocolError` in `shared`).

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("unauthorized")]
    Unauthorized,
    // Returned when registration hits a unique-constraint (username taken).
    // (Vault *push* conflicts are a 200 `PushResponse::Conflict` body, not this.)
    #[error("conflict")]
    Conflict,
    #[error("bad request")]
    BadRequest,
    #[error("too many requests")]
    TooManyRequests,
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
            ApiError::TooManyRequests => StatusCode::TOO_MANY_REQUESTS,
            ApiError::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };
        // Body intentionally minimal.
        (code, self.to_string()).into_response()
    }
}
