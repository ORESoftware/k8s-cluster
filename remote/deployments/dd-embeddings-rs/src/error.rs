//! HTTP-facing error type. Maps domain errors to status codes + a stable JSON
//! envelope `{ "error": { "kind": ..., "message": ... } }`.
//!
//! Client-facing messages are deliberately decoupled from the internal error
//! detail: upstream provider/Qdrant bodies are logged server-side but never
//! echoed back to the caller (they can carry request context we don't want to
//! reflect). Validation errors, by contrast, are returned verbatim — they're
//! the caller's own input and actionable.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use crate::providers::ProviderError;
use crate::rag::RagError;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("unauthorized")]
    Unauthorized,
    /// Server is at its concurrency limit; caller should retry later.
    #[error("service overloaded")]
    Overloaded,
    /// Caller-input validation failure. The message is safe to return.
    #[error("{0}")]
    Invalid(String),
    #[error("upstream provider failure: {0}")]
    Provider(#[from] ProviderError),
    #[error("rag failure: {0}")]
    Rag(#[from] RagError),
}

impl ApiError {
    fn status_kind(&self) -> (StatusCode, &'static str) {
        match self {
            ApiError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            ApiError::Overloaded => (StatusCode::SERVICE_UNAVAILABLE, "overloaded"),
            ApiError::Invalid(_) => (StatusCode::BAD_REQUEST, "invalid_request"),
            ApiError::Provider(e) => provider_status_kind(e),
            ApiError::Rag(RagError::Provider(e)) => provider_status_kind(e),
            ApiError::Rag(RagError::NoDocuments) => (StatusCode::BAD_REQUEST, "no_documents"),
            ApiError::Rag(RagError::CountMismatch { .. }) => (StatusCode::BAD_GATEWAY, "count_mismatch"),
            ApiError::Rag(RagError::Qdrant(_)) => (StatusCode::BAD_GATEWAY, "qdrant_error"),
        }
    }

    /// Message returned to the caller. For anything that originates upstream we
    /// return a generic string and rely on logs + `kind` for diagnosis, so we
    /// never reflect a provider/Qdrant response body back over the wire.
    fn client_message(&self, kind: &str) -> String {
        match self {
            ApiError::Unauthorized => "unauthorized".into(),
            ApiError::Overloaded => "service overloaded, retry later".into(),
            ApiError::Invalid(m) => m.clone(),
            ApiError::Provider(ProviderError::Unknown(p)) => format!("unknown provider `{p}`"),
            ApiError::Provider(ProviderError::NotConfigured(p)) => {
                format!("provider `{p}` is not configured")
            }
            ApiError::Provider(ProviderError::EmptyInput) => {
                "input must contain at least one non-empty string".into()
            }
            ApiError::Provider(ProviderError::InvalidModel(_, m)) => format!("invalid model name: {m}"),
            ApiError::Rag(RagError::NoDocuments) => "at least one document is required".into(),
            // Upstream transport/decode/5xx, Qdrant, count mismatch: don't leak detail.
            _ => format!("{kind} (see server logs for detail)"),
        }
    }
}

fn provider_status_kind(e: &ProviderError) -> (StatusCode, &'static str) {
    match e {
        ProviderError::Unknown(_) => (StatusCode::NOT_FOUND, "unknown_provider"),
        ProviderError::NotConfigured(_) => (StatusCode::SERVICE_UNAVAILABLE, "provider_not_configured"),
        ProviderError::InvalidModel(..) => (StatusCode::BAD_REQUEST, "invalid_model"),
        ProviderError::EmptyInput => (StatusCode::BAD_REQUEST, "empty_input"),
        ProviderError::Transport { .. } => (StatusCode::BAD_GATEWAY, "upstream_transport"),
        ProviderError::Upstream { .. } => (StatusCode::BAD_GATEWAY, "upstream_error"),
        ProviderError::Decode(..) => (StatusCode::BAD_GATEWAY, "upstream_decode"),
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, kind) = self.status_kind();
        // Log full internal detail (including any upstream body) server-side;
        // 5xx are the actionable ones.
        if status.is_server_error() {
            tracing::warn!(error = %self, kind, "request failed");
        }
        let body = Json(json!({
            "error": { "kind": kind, "message": self.client_message(kind) }
        }));
        (status, body).into_response()
    }
}
