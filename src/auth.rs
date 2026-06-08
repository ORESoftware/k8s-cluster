use axum::http::HeaderMap;
use std::sync::atomic::Ordering;

use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};

const AUTH_HEADER: &str = "auth";
const SERVER_AUTH_HEADER: &str = "x-server-auth";
const AGENT_AUTH_HEADER: &str = "x-agent-auth";

pub fn require_auth(headers: &HeaderMap, state: &AppState) -> ApiResult<()> {
    if !state.config.auth_required {
        return Ok(());
    }

    let Some(secret) = state.config.auth_secret.as_ref() else {
        state
            .metrics
            .auth_failures_total
            .fetch_add(1, Ordering::Relaxed);
        return Err(ApiError::unavailable(
            "USACC API auth is required but no auth secret is configured",
        ));
    };

    let ok = [SERVER_AUTH_HEADER, AGENT_AUTH_HEADER, AUTH_HEADER]
        .iter()
        .filter_map(|name| headers.get(*name))
        .filter_map(|value| value.to_str().ok())
        .any(|value| value == secret);

    if ok {
        Ok(())
    } else {
        state
            .metrics
            .auth_failures_total
            .fetch_add(1, Ordering::Relaxed);
        Err(ApiError::unauthorized(
            "missing or invalid USACC API auth header",
        ))
    }
}
