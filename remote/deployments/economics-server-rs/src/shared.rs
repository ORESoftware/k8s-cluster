use std::{
    sync::atomic::Ordering,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};

use crate::state::*;
use crate::types::*;

pub(crate) fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

pub(crate) fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) fn now_unix_nano_string() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .to_string()
}

pub(crate) fn severity_number(severity_text: &str) -> u8 {
    match severity_text {
        "TRACE" => 1,
        "DEBUG" => 5,
        "INFO" => 9,
        "WARN" => 13,
        "ERROR" => 17,
        _ => 9,
    }
}

pub(crate) fn telemetry_log_record(
    severity_text: &str,
    event_name: &str,
    body: &str,
    attributes: Value,
) -> Value {
    json!({
        "schema": "dd.log.v1",
        "time_unix_nano": now_unix_nano_string(),
        "severity_text": severity_text,
        "severity_number": severity_number(severity_text),
        "body": body,
        "resource_service_name": SERVICE_NAME,
        "resource_service_namespace": env_value("OTEL_SERVICE_NAMESPACE", "remote-dev"),
        "scope_name": "economics-server",
        "event_name": event_name,
        "attributes": attributes
    })
}

pub(crate) fn emit_log(severity_text: &str, event_name: &str, body: &str, attributes: Value) {
    let record = telemetry_log_record(severity_text, event_name, body, attributes).to_string();
    if severity_number(severity_text) >= 17 {
        tracing::error!("{record}");
    } else {
        tracing::info!("{record}");
    }
}

pub(crate) fn error_summary(error: &str) -> String {
    error
        .chars()
        .filter(|ch| !ch.is_control())
        .take(256)
        .collect()
}

pub(crate) fn clamp(value: f64, min: f64, max: f64) -> f64 {
    value.max(min).min(max)
}

pub(crate) fn finite_or(value: Option<f64>, fallback: f64) -> f64 {
    value
        .filter(|number| number.is_finite())
        .unwrap_or(fallback)
}

pub(crate) fn request_id(input: Option<&String>, fallback: &str) -> String {
    input
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback)
        .chars()
        .take(MAX_TOKEN_LEN)
        .collect()
}

pub(crate) fn clean_token(value: &str, label: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{label} must not be empty"));
    }
    if trimmed.len() > MAX_TOKEN_LEN {
        return Err(format!("{label} must be at most {MAX_TOKEN_LEN} bytes"));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(format!("{label} must not contain control characters"));
    }
    Ok(trimmed.to_string())
}

pub(crate) fn clean_optional_token(value: &Option<String>, label: &str) -> Result<(), String> {
    if let Some(value) = value.as_deref() {
        clean_token(value, label)?;
    }
    Ok(())
}

pub(crate) fn validate_source_auth_env(config: &Config, env_name: &str) -> Result<String, String> {
    let clean = clean_token(env_name, "authHeaderEnv")?;
    let normalized = clean.to_ascii_lowercase();
    if config
        .allowed_source_auth_envs
        .iter()
        .any(|allowed| allowed == &normalized)
    {
        return Ok(clean);
    }
    Err(
        "authHeaderEnv must be listed in ECONOMICS_ALLOWED_SOURCE_AUTH_ENVS or one of the built-in ECONOMICS_* credential placeholders"
            .to_string(),
    )
}

pub(crate) fn validate_source_auth_header_name(name: &str) -> Result<reqwest::header::HeaderName, String> {
    let clean = clean_token(name, "authHeaderName")?;
    let lower = clean.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "host"
            | "connection"
            | "content-length"
            | "transfer-encoding"
            | "cookie"
            | "set-cookie"
            | "proxy-authorization"
            | "upgrade"
    ) {
        return Err(
            "authHeaderName cannot be a hop-by-hop, cookie, host, or payload framing header"
                .to_string(),
        );
    }
    clean
        .parse::<reqwest::header::HeaderName>()
        .map_err(|error| format!("authHeaderName is invalid: {error}"))
}

pub(crate) fn constant_time_eq(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    let max_len = left.len().max(right.len());
    let mut diff = left.len() ^ right.len();
    for index in 0..max_len {
        let l = left.get(index).copied().unwrap_or(0);
        let r = right.get(index).copied().unwrap_or(0);
        diff |= usize::from(l ^ r);
    }
    diff == 0
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
        .or_else(|| headers.get("authorization"))
        .and_then(|value| value.to_str().ok());
    match provided {
        Some(value) if constant_time_eq(value.trim_start_matches("Bearer ").trim(), secret) => {
            Ok(())
        }
        _ => Err(AuthFailure::Unauthorized),
    }
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
    emit_log(
        "WARN",
        "economics.auth.failure",
        "economics request authentication failed",
        json!({
            "failure": message,
            "authConfigured": state.config.server_auth_secret.is_some(),
            "allowUnauthenticated": state.config.allow_unauthenticated
        }),
    );
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "ok": false, "error": message })),
    )
        .into_response()
}
