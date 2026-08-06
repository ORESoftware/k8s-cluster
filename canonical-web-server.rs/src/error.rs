use axum::{
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("authentication failed")]
    Unauthorized,
    #[error("forbidden")]
    Forbidden,
    #[error("invalid request: {0}")]
    BadRequest(String),
    #[error("invalid sync cursor")]
    InvalidSyncCursor,
    #[error("resource not found")]
    NotFound,
    #[error("upstream authentication service failed")]
    AuthUpstream,
    #[error("upstream application service failed")]
    ServiceUpstream,
    #[error("request rate limit exceeded")]
    RateLimited { retry_after_seconds: u64 },
    #[error("password authentication capacity is temporarily exhausted")]
    LoginAuthBusy,
    #[error("bearer authentication capacity is temporarily exhausted")]
    AuthBusy,
    #[error("database error")]
    Database(#[from] sea_orm::DbErr),
    #[error("HTTP client configuration failed")]
    HttpClient(#[from] reqwest::Error),
    #[error("Supabase Auth client configuration failed")]
    AuthClient(#[from] canonical_auth::SupabaseAuthBuildError),
    #[error("I/O error")]
    Io(#[from] std::io::Error),
    #[error("session cryptography failed")]
    Crypto,
    #[error("serialization failed")]
    Serialization(#[from] serde_json::Error),
}

impl From<canonical_session::SessionError> for AppError {
    fn from(error: canonical_session::SessionError) -> Self {
        match error {
            canonical_session::SessionError::Unauthorized => Self::Unauthorized,
            canonical_session::SessionError::BadRequest(message) => Self::BadRequest(message),
            canonical_session::SessionError::AuthUpstream => Self::AuthUpstream,
            canonical_session::SessionError::Database(error) => Self::Database(error),
            canonical_session::SessionError::Crypto => Self::Crypto,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let retry_after_seconds = match &self {
            Self::RateLimited {
                retry_after_seconds,
            } => Some(*retry_after_seconds),
            Self::LoginAuthBusy | Self::AuthBusy => Some(1),
            _ => None,
        };
        let (status, code, message) = match &self {
            Self::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "authentication required",
            ),
            Self::Forbidden => (StatusCode::FORBIDDEN, "forbidden", "request is not allowed"),
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, "bad_request", message.as_str()),
            Self::InvalidSyncCursor => (
                StatusCode::BAD_REQUEST,
                "invalid_sync_cursor",
                "invalid sync cursor",
            ),
            Self::NotFound => (StatusCode::NOT_FOUND, "not_found", "resource not found"),
            Self::AuthUpstream => (
                StatusCode::SERVICE_UNAVAILABLE,
                "auth_upstream_unavailable",
                "authentication service is temporarily unavailable",
            ),
            Self::ServiceUpstream => (
                StatusCode::SERVICE_UNAVAILABLE,
                "service_upstream_unavailable",
                "quote analysis is temporarily unavailable",
            ),
            Self::RateLimited { .. } => (
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                "too many requests; retry later",
            ),
            Self::LoginAuthBusy => (
                StatusCode::TOO_MANY_REQUESTS,
                "login_auth_busy",
                "authentication is temporarily busy; retry later",
            ),
            Self::AuthBusy => (
                StatusCode::TOO_MANY_REQUESTS,
                "auth_verification_busy",
                "authentication verification is temporarily busy; retry later",
            ),
            _ => {
                tracing::error!(error = %self, "request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "the server could not complete the request",
                )
            }
        };

        let mut response = (
            status,
            Json(json!({ "error": { "code": code, "message": message } })),
        )
            .into_response();
        if let Some(retry_after_seconds) = retry_after_seconds {
            if let Ok(value) = HeaderValue::from_str(&retry_after_seconds.to_string()) {
                response.headers_mut().insert(header::RETRY_AFTER, value);
            }
        }
        response
    }
}
