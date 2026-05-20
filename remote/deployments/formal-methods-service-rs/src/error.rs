//! Application error type. Each variant maps to a stable HTTP response.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("missing required header: {0}")]
    MissingHeader(&'static str),

    #[error("invalid header value for {0}")]
    InvalidHeader(&'static str),

    #[error("invalid webhook signature")]
    InvalidSignature,

    #[error("unsupported event: {0}")]
    UnsupportedEvent(String),

    #[error("malformed payload: {0}")]
    MalformedPayload(String),

    #[error("internal error: {0}")]
    Internal(String),
}

impl AppError {
    pub fn status(&self) -> StatusCode {
        match self {
            AppError::MissingHeader(_) | AppError::InvalidHeader(_) => StatusCode::BAD_REQUEST,
            AppError::InvalidSignature => StatusCode::UNAUTHORIZED,
            AppError::UnsupportedEvent(_) => StatusCode::ACCEPTED,
            AppError::MalformedPayload(_) => StatusCode::BAD_REQUEST,
            AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status();
        let body = json!({
            "error": self.to_string(),
            "code": status.as_u16(),
        });
        (status, Json(body)).into_response()
    }
}

impl From<anyhow::Error> for AppError {
    fn from(err: anyhow::Error) -> Self {
        AppError::Internal(format!("{err:#}"))
    }
}
