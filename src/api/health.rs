use axum::Json;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
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
    match sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&state.pool)
        .await
    {
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

pub async fn metrics(State(state): State<AppState>) -> Response {
    let db_ready = match sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&state.pool)
        .await
    {
        Ok(_) => 1,
        Err(_) => 0,
    };
    let body = format!(
        concat!(
            "# HELP dd_billing_server_build_info Billing server build metadata.\n",
            "# TYPE dd_billing_server_build_info gauge\n",
            "dd_billing_server_build_info{{service=\"dd-billing-server\",version=\"{}\"}} 1\n",
            "# HELP dd_billing_server_ready Database readiness state.\n",
            "# TYPE dd_billing_server_ready gauge\n",
            "dd_billing_server_ready {}\n"
        ),
        env!("CARGO_PKG_VERSION"),
        db_ready
    );

    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
        .into_response()
}
