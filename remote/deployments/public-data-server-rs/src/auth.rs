use std::sync::atomic::Ordering;

use axum::{
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

use crate::state::AppState;
use crate::types::AuthFailure;

pub(crate) fn require_auth(headers: &HeaderMap, state: &AppState) -> Result<(), AuthFailure> {
    if state.config.allow_unauthenticated {
        return Ok(());
    }
    let Some(secret) = state.config.server_auth_secret.as_ref() else {
        return Err(AuthFailure::MissingSecret);
    };
    let provided = headers
        .get("x-server-auth")
        .or_else(|| headers.get("auth"))
        .and_then(|value| value.to_str().ok());
    match provided {
        Some(value) if value == secret => Ok(()),
        _ => Err(AuthFailure::Unauthorized),
    }
}

pub(crate) fn require_webhook_auth(headers: &HeaderMap, state: &AppState) -> Result<(), AuthFailure> {
    if state.config.allow_unauthenticated_webhooks {
        return Ok(());
    }
    if let Some(secret) = state.config.webhook_secret.as_ref() {
        let provided = headers
            .get("x-public-data-webhook-secret")
            .or_else(|| headers.get("x-webhook-secret"))
            .and_then(|value| value.to_str().ok());
        return match provided {
            Some(value) if value == secret => Ok(()),
            _ => Err(AuthFailure::Unauthorized),
        };
    }
    require_auth(headers, state)
}

pub(crate) fn auth_failure_response(state: &AppState, failure: AuthFailure) -> Response {
    state
        .metrics
        .auth_failures_total
        .fetch_add(1, Ordering::Relaxed);
    let message = match failure {
        AuthFailure::MissingSecret => "server auth secret is not configured",
        AuthFailure::Unauthorized => "unauthorized",
    };
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "ok": false, "error": message })),
    )
        .into_response()
}
