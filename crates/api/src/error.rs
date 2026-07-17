//! Uniform JSON error envelope: `{"ok": false, "error": "..."}` with a
//! meaningful status, matching the fleet's response shape.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use t2v_core::audio::AudioError;
use t2v_core::fft::FftError;
use t2v_llm::LlmError;

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

impl ApiError {
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: msg.into(),
        }
    }

    pub fn unprocessable(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            message: msg.into(),
        }
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: msg.into(),
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.status)
    }
}

impl std::error::Error for ApiError {}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        if self.status.is_server_error() {
            tracing::error!("t2v-api error {}: {}", self.status, self.message);
        }
        (
            self.status,
            Json(json!({ "ok": false, "error": self.message })),
        )
            .into_response()
    }
}

impl From<LlmError> for ApiError {
    fn from(e: LlmError) -> Self {
        let status = match &e {
            LlmError::MissingApiKey(_) => StatusCode::SERVICE_UNAVAILABLE,
            LlmError::UnknownProvider(_) => StatusCode::BAD_REQUEST,
            // Upstream provider misbehaved; we're the gateway.
            LlmError::Http(_) | LlmError::Api { .. } | LlmError::Parse { .. } => {
                StatusCode::BAD_GATEWAY
            }
        };
        Self {
            status,
            message: e.to_string(),
        }
    }
}

impl From<sea_orm::DbErr> for ApiError {
    fn from(e: sea_orm::DbErr) -> Self {
        Self::internal(format!("database error: {e}"))
    }
}

impl From<AudioError> for ApiError {
    fn from(e: AudioError) -> Self {
        Self::bad_request(format!("audio decode failed: {e}"))
    }
}

impl From<FftError> for ApiError {
    fn from(e: FftError) -> Self {
        Self::bad_request(format!("spectral analysis failed: {e}"))
    }
}
