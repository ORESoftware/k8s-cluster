use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("unauthorized")]
    Unauthorized,

    #[error("forbidden")]
    Forbidden,

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("ledger invariant violated: {0}")]
    LedgerInvariant(String),

    #[error("provider error ({provider}): {message}")]
    Provider { provider: String, message: String },

    #[error("provider rate limited ({provider}): retry after {retry_after_seconds}s: {message}")]
    ProviderRateLimited {
        provider: String,
        retry_after_seconds: i64,
        message: String,
    },

    #[error("crypto error: {0}")]
    Crypto(String),

    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

#[derive(Serialize)]
struct ErrBody<'a> {
    error: &'a str,
    message: String,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let retry_after = self.retry_after_seconds();
        let (status, code) = match &self {
            AppError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            AppError::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            AppError::Forbidden => (StatusCode::FORBIDDEN, "forbidden"),
            AppError::Conflict(_) => (StatusCode::CONFLICT, "conflict"),
            AppError::LedgerInvariant(_) => (StatusCode::UNPROCESSABLE_ENTITY, "ledger_invariant"),
            AppError::Provider { .. } => (StatusCode::BAD_GATEWAY, "provider_error"),
            AppError::ProviderRateLimited { .. } => {
                (StatusCode::TOO_MANY_REQUESTS, "provider_rate_limited")
            }
            AppError::Crypto(_) => (StatusCode::INTERNAL_SERVER_ERROR, "crypto_error"),
            AppError::Database(err) => {
                tracing::error!(error = %err, "database error");
                (StatusCode::INTERNAL_SERVER_ERROR, "database_error")
            }
            AppError::Other(err) => {
                tracing::error!(error = %err, "internal error");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal_error")
            }
        };

        let mut response = (
            status,
            Json(ErrBody {
                error: code,
                message: self.to_string(),
            }),
        )
            .into_response();
        if let Some(seconds) = retry_after {
            if let Ok(value) = seconds.to_string().parse() {
                response
                    .headers_mut()
                    .insert(axum::http::header::RETRY_AFTER, value);
            }
        }
        response
    }
}

impl AppError {
    pub fn retry_after_seconds(&self) -> Option<i64> {
        match self {
            AppError::ProviderRateLimited {
                retry_after_seconds,
                ..
            } => Some((*retry_after_seconds).clamp(1, 3600)),
            _ => None,
        }
    }
}

pub type AppResult<T> = Result<T, AppError>;
