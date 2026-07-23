//! Liveness and readiness probes.

use axum::{extract::State, http::StatusCode, Json};
use serde_json::json;

use crate::state::AppState;

/// Liveness: the process is up. Never touches dependencies.
pub async fn healthz() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok" }))
}

/// Readiness: safe to route traffic. Pings the DB when the mirror is configured;
/// a JWKS project misconfig would already have failed startup.
pub async fn readyz(State(state): State<AppState>) -> (StatusCode, Json<serde_json::Value>) {
    if let Some(db) = &state.db {
        if db.ping().await.is_err() {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "status": "degraded", "db": "unavailable" })),
            );
        }
    }
    // Redis is deliberately not readiness-critical: Postgres remains the
    // source of truth and the service degrades safely to DB-backed checks.
    (StatusCode::OK, Json(json!({ "status": "ready" })))
}
