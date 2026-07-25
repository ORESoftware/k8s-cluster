use std::sync::atomic::Ordering;

use axum::{
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    Json,
};
use maud::html;
use serde_json::json;

use crate::state::AppState;

pub(crate) enum AuthFailure {
    MissingSecret,
    Unauthorized,
}

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
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if constant_time_eq(provided, secret) {
        Ok(())
    } else {
        Err(AuthFailure::Unauthorized)
    }
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

pub(crate) fn auth_failure_response(state: &AppState, failure: AuthFailure) -> Response {
    state
        .metrics
        .auth_failures_total
        .fetch_add(1, Ordering::Relaxed);
    let (status, message) = match failure {
        AuthFailure::MissingSecret => (
            StatusCode::SERVICE_UNAVAILABLE,
            "server auth secret is not configured",
        ),
        AuthFailure::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
    };
    (status, Json(json!({ "ok": false, "error": message }))).into_response()
}

pub(crate) fn ui_auth_failure_response(state: &AppState, failure: AuthFailure) -> Response {
    state
        .metrics
        .auth_failures_total
        .fetch_add(1, Ordering::Relaxed);
    let message = match failure {
        AuthFailure::MissingSecret => {
            "SERVER_AUTH_SECRET is not configured for package generation."
        }
        AuthFailure::Unauthorized => "Package generation is waiting for operator authentication.",
    };
    (
        StatusCode::UNAUTHORIZED,
        Html(
            html! {
                div class="result error" { strong { "Auth required" } p { (message) } }
            }
            .into_string(),
        ),
    )
        .into_response()
}
