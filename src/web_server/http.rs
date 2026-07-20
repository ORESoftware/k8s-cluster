//! Health, readiness, and Prometheus adapters for the web process.

use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

use super::WebState;

pub(super) async fn healthz() -> Json<serde_json::Value> {
    Json(json!({
        "ok": true,
        "service": "dd-fabrication-web-server",
        "architecture": "mash",
        "databaseClient": "seaorm",
        "transports": ["http", "websocket-html", "websocket-json", "tcp", "nats"],
    }))
}

pub(super) async fn readyz(State(state): State<WebState>) -> Response {
    if state.persistence.is_ready().await {
        Json(json!({
            "ok": true,
            "database": if state.persistence.is_enabled() { "ready" } else { "disabled" },
            "backendBridge": "configured",
            "nats": if state.nats_enabled { "connected" } else { "disabled" },
            "supabaseRealtime": if state.supabase_enabled { "configured" } else { "disabled" },
        }))
        .into_response()
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"ok": false, "database": "unavailable"})),
        )
            .into_response()
    }
}

pub(super) async fn metrics(State(state): State<WebState>) -> Response {
    let body = format!(
        concat!(
            "# HELP dd_fabrication_web_up Whether the web process is running.\n",
            "# TYPE dd_fabrication_web_up gauge\n",
            "dd_fabrication_web_up 1\n",
            "# HELP dd_fabrication_web_persistence_enabled Whether SeaORM persistence is configured.\n",
            "# TYPE dd_fabrication_web_persistence_enabled gauge\n",
            "dd_fabrication_web_persistence_enabled {}\n",
            "# HELP dd_fabrication_web_nats_enabled Whether the NATS relay is connected.\n",
            "# TYPE dd_fabrication_web_nats_enabled gauge\n",
            "dd_fabrication_web_nats_enabled {}\n",
            "# HELP dd_fabrication_web_supabase_realtime_enabled Whether Supabase Realtime is configured.\n",
            "# TYPE dd_fabrication_web_supabase_realtime_enabled gauge\n",
            "dd_fabrication_web_supabase_realtime_enabled {}\n",
            "# HELP dd_fabrication_web_realtime_events_published_total Events published to the shared realtime hub.\n",
            "# TYPE dd_fabrication_web_realtime_events_published_total counter\n",
            "dd_fabrication_web_realtime_events_published_total {}\n",
            "# HELP dd_fabrication_web_realtime_subscribers Current in-process transport subscribers.\n",
            "# TYPE dd_fabrication_web_realtime_subscribers gauge\n",
            "dd_fabrication_web_realtime_subscribers {}\n",
        ),
        usize::from(state.persistence.is_enabled()),
        usize::from(state.nats_enabled),
        usize::from(state.supabase_enabled),
        state.realtime.published_total(),
        state.realtime.subscriber_count(),
    );
    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], body).into_response()
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;

    use crate::{
        persistence::Persistence,
        realtime::{EventHub, ServiceSurface},
    };

    use super::*;

    #[tokio::test]
    async fn prometheus_metrics_cover_realtime_and_dependency_state() {
        let hub = EventHub::new(ServiceSurface::Web, 8);
        hub.publish_payload(
            "test",
            "supabase.postgres_changes",
            json!({"status": "printing"}),
        );
        let state = WebState {
            persistence: Persistence::Disabled,
            realtime: hub,
            nats_enabled: true,
            supabase_enabled: true,
        };
        let response = metrics(State(state)).await;
        let body = to_bytes(response.into_body(), 16 * 1024)
            .await
            .expect("metrics body");
        let body = String::from_utf8(body.to_vec()).expect("UTF-8 metrics");

        assert!(body.contains("dd_fabrication_web_up 1"));
        assert!(body.contains("dd_fabrication_web_nats_enabled 1"));
        assert!(body.contains("dd_fabrication_web_supabase_realtime_enabled 1"));
        assert!(body.contains("dd_fabrication_web_realtime_events_published_total 1"));
    }
}
