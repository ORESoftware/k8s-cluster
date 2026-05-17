//! Liveness / readiness endpoints.

use axum::extract::State;
use axum::Json;
use serde_json::json;

use crate::state::AppState;

pub async fn health() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok" }))
}

pub async fn ready(State(state): State<AppState>) -> Json<serde_json::Value> {
    let analyzers = state.pipeline.analyzer_names();
    let dedupe_len = state.delivery_dedupe.lock().map(|g| g.len()).unwrap_or(0);

    Json(json!({
        "status": "ok",
        "analyzers": analyzers,
        "github_token_configured": state.github.has_token(),
        "repo_allowlist": {
            "allow_all": state.repo_allowlist.allow_all(),
        },
        "path_filter": {
            "active": !state.path_filter.is_empty(),
            "prefixes": state.path_filter.prefixes(),
        },
        "delivery_dedupe": {
            "tracked": dedupe_len,
        },
    }))
}
