use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Serialize;

use crate::state::AppState;

#[derive(Serialize)]
pub struct HealthBody {
    pub status: &'static str,
    pub service: &'static str,
    pub version: &'static str,
}

pub async fn healthz() -> Json<HealthBody> {
    Json(HealthBody {
        status: "ok",
        service: "billing-server-rs",
        version: env!("CARGO_PKG_VERSION"),
    })
}

pub async fn readyz(State(state): State<AppState>) -> (StatusCode, Json<HealthBody>) {
    match sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(&state.pool).await {
        Ok(_) => (
            StatusCode::OK,
            Json(HealthBody {
                status: "ready",
                service: "billing-server-rs",
                version: env!("CARGO_PKG_VERSION"),
            }),
        ),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(HealthBody {
                status: "db_unavailable",
                service: "billing-server-rs",
                version: env!("CARGO_PKG_VERSION"),
            }),
        ),
    }
}
