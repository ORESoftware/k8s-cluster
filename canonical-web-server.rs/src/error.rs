use axum::{
    http::StatusCode,
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
    #[error("database error")]
    Database(#[from] sea_orm::DbErr),
    #[error("HTTP client configuration failed")]
    HttpClient(#[from] reqwest::Error),
    #[error("I/O error")]
    Io(#[from] std::io::Error),
    #[error("session cryptography failed")]
    Crypto,
    #[error("serialization failed")]
    Serialization(#[from] serde_json::Error),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
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
            _ => {
                tracing::error!(error = %self, "request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "the server could not complete the request",
                )
            }
        };

        (
            status,
            Json(json!({ "error": { "code": code, "message": message } })),
        )
            .into_response()
    }
}
