//! Liveness, readiness, and Prometheus scrape handlers.

use crate::error::ApiError;
use crate::metrics;
use crate::state::AppState;
use axum::body::Body;
use axum::extract::State;
use axum::response::Response;

pub(crate) async fn live() -> &'static str {
    "ok"
}

pub(crate) async fn ready(State(state): State<AppState>) -> Result<&'static str, ApiError> {
    state.ping_database().await?;
    Ok("ok")
}

pub(crate) async fn prometheus(State(state): State<AppState>) -> Response<Body> {
    metrics::response(&state.metrics)
}
