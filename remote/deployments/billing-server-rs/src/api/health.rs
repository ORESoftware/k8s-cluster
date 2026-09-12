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
    let (published, dropped_oversize, failed) = state.events.counters();
    let nats_enabled = u8::from(state.events.is_enabled());
    let body = format!(
        concat!(
            "# HELP dd_billing_server_build_info Billing server build metadata.\n",
            "# TYPE dd_billing_server_build_info gauge\n",
            "dd_billing_server_build_info{{service=\"dd-billing-server\",version=\"{}\"}} 1\n",
            "# HELP dd_billing_server_ready Database readiness state.\n",
            "# TYPE dd_billing_server_ready gauge\n",
            "dd_billing_server_ready {}\n",
            "# HELP dd_billing_server_nats_enabled Whether the NATS event bus is connected.\n",
            "# TYPE dd_billing_server_nats_enabled gauge\n",
            "dd_billing_server_nats_enabled {}\n",
            "# HELP dd_billing_server_nats_published_total Domain events published to NATS.\n",
            "# TYPE dd_billing_server_nats_published_total counter\n",
            "dd_billing_server_nats_published_total {}\n",
            "# HELP dd_billing_server_nats_dropped_oversize_total Events dropped for exceeding the payload ceiling.\n",
            "# TYPE dd_billing_server_nats_dropped_oversize_total counter\n",
            "dd_billing_server_nats_dropped_oversize_total {}\n",
            "# HELP dd_billing_server_nats_publish_failed_total Event publishes that errored against the broker.\n",
            "# TYPE dd_billing_server_nats_publish_failed_total counter\n",
            "dd_billing_server_nats_publish_failed_total {}\n"
        ),
        env!("CARGO_PKG_VERSION"),
        db_ready,
        nats_enabled,
        published,
        dropped_oversize,
        failed
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
