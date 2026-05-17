//! Axum router wiring.

pub mod health;
pub mod webhook;

use axum::routing::{get, post};
use axum::Router;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

use crate::state::AppState;

/// Inbound webhook payloads are bounded. GitHub limits payloads to ~25 MB but
/// we expect 1-2 MB at most for pull_request events.
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health::health))
        .route("/ready", get(health::ready))
        .route("/webhook/github", post(webhook::github))
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
