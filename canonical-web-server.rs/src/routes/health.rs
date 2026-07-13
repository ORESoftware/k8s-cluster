use crate::{AppState, SERVICE};
use axum::{extract::State, http::StatusCode, Json};
use serde::Serialize;

#[derive(Serialize)]
pub struct HealthResponse {
    status: &'static str,
    service: &'static str,
}

#[derive(Serialize)]
pub struct InfoResponse {
    service: &'static str,
    version: &'static str,
    domain: &'static str,
    stack: [&'static str; 5],
}

pub async fn healthz() -> StatusCode {
    StatusCode::OK
}

pub async fn readyz(State(state): State<AppState>) -> StatusCode {
    match state.db.ping().await {
        Ok(()) => StatusCode::OK,
        Err(error) => {
            tracing::warn!(%error, "database readiness check failed");
            StatusCode::SERVICE_UNAVAILABLE
        }
    }
}

pub async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        service: SERVICE,
    })
}

pub async fn info() -> Json<InfoResponse> {
    Json(InfoResponse {
        service: SERVICE,
        version: env!("CARGO_PKG_VERSION"),
        domain: "canonical.cloud",
        stack: ["supabase", "maud", "axum", "seaorm", "htmx"],
    })
}
