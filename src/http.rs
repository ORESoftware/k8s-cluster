//! The HTTP surface.
//!
//! - `POST /publish` — internal callers (shared-auth-server, the sync outbox
//!   flusher) hand the bridge an event; it lands on NATS broker-confirmed.
//!   Bearer-gated with `BRIDGE_INTERNAL_TOKEN` and prefix-constrained: this is
//!   the shared-auth event plane, not a generic NATS proxy.
//! - `GET /healthz` — process liveness.
//! - `GET /readyz` — NATS connectivity (503 while the broker is unreachable).
//! - `GET /metrics` — Prometheus.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;

use crate::config::{validate_publish_subject, Config};
use crate::metrics::Metrics;
use crate::publisher::{PublishError, Publisher};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub publisher: Publisher,
    pub metrics: Metrics,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
        .route("/publish", post(publish))
        .with_state(state)
}

async fn healthz() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok" }))
}

async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    if state.publisher.ready().await {
        (StatusCode::OK, Json(json!({ "status": "ready" })))
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "status": "degraded", "nats": "unavailable" })),
        )
    }
}

async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    let (content_type, body) = state.metrics.render();
    (StatusCode::OK, [(header::CONTENT_TYPE, content_type)], body)
}

async fn publish(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    // Constant-shape auth check; uniform 401.
    let authorized = bearer(&headers)
        .map(|token| token == state.config.internal_token)
        .unwrap_or(false);
    if !authorized {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        );
    }

    if body.len() > state.config.max_payload_bytes {
        state
            .metrics
            .publishes
            .with_label_values(&["rejected"])
            .inc();
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({ "error": "payload_too_large" })),
        );
    }

    #[derive(serde::Deserialize)]
    struct PublishRequest {
        subject: String,
        payload: serde_json::Value,
    }
    let request: PublishRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => {
            state
                .metrics
                .publishes
                .with_label_values(&["rejected"])
                .inc();
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "expected {subject, payload}" })),
            );
        }
    };

    if let Err(reason) = validate_publish_subject(&request.subject, &state.config.subject_prefix) {
        state
            .metrics
            .publishes
            .with_label_values(&["rejected"])
            .inc();
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": reason })));
    }

    let payload = serde_json::to_vec(&request.payload).unwrap_or_default();
    match state.publisher.publish(&request.subject, payload).await {
        Ok(()) => {
            state.metrics.publishes.with_label_values(&["ok"]).inc();
            tracing::info!(subject = %request.subject, "published");
            (
                StatusCode::ACCEPTED,
                Json(json!({ "published": request.subject })),
            )
        }
        Err(PublishError::NotConnected) => {
            state.metrics.publishes.with_label_values(&["failed"]).inc();
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "nats_unavailable" })),
            )
        }
        Err(PublishError::Failed(error)) => {
            state.metrics.publishes.with_label_values(&["failed"]).inc();
            tracing::error!(%error, "publish failed");
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": "publish_failed" })),
            )
        }
    }
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|token| !token.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    const TOKEN: &str = "test-internal-token-1234";

    fn state() -> AppState {
        AppState {
            config: Arc::new(Config {
                bind_addr: "127.0.0.1:0".parse().unwrap(),
                nats_url: "nats://127.0.0.1:1".into(),
                subject_prefix: "shared-auth.".into(),
                internal_token: TOKEN.into(),
                deliveries: vec![],
                max_payload_bytes: 64 * 1024,
            }),
            publisher: Publisher::mock(),
            metrics: Metrics::new(),
        }
    }

    fn publish_request(
        token: Option<&str>,
        body: serde_json::Value,
    ) -> axum::http::Request<axum::body::Body> {
        let mut builder =
            axum::http::Request::post("/publish").header("content-type", "application/json");
        if let Some(token) = token {
            builder = builder.header("authorization", format!("Bearer {token}"));
        }
        builder
            .body(axum::body::Body::from(body.to_string()))
            .unwrap()
    }

    #[tokio::test]
    async fn publish_requires_the_internal_token() {
        let app = router(state());
        let missing = app
            .clone()
            .oneshot(publish_request(
                None,
                serde_json::json!({"subject":"shared-auth.x","payload":{}}),
            ))
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);

        let wrong = app
            .oneshot(publish_request(
                Some("nope"),
                serde_json::json!({"subject":"shared-auth.x","payload":{}}),
            ))
            .await
            .unwrap();
        assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn publish_lands_on_the_broker_and_counts() {
        let s = state();
        let seen = match &s.publisher {
            Publisher::Mock(seen) => seen.clone(),
            _ => unreachable!(),
        };
        let app = router(s);
        let response = app
            .oneshot(publish_request(
                Some(TOKEN),
                serde_json::json!({"subject":"shared-auth.events.identity","payload":{"user":"u-1"}}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let published = seen.read().await;
        assert_eq!(published.len(), 1);
        assert_eq!(published[0].0, "shared-auth.events.identity");
        assert_eq!(
            published[0].1,
            serde_json::to_vec(&serde_json::json!({"user":"u-1"})).unwrap()
        );
    }

    #[tokio::test]
    async fn publish_rejects_foreign_and_wildcard_subjects() {
        let app = router(state());
        for subject in ["dd.events.x", "shared-auth.events.*", "shared-auth.>"] {
            let response = app
                .clone()
                .oneshot(publish_request(
                    Some(TOKEN),
                    serde_json::json!({"subject": subject, "payload": {}}),
                ))
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::BAD_REQUEST,
                "subject {subject}"
            );
        }
    }

    #[tokio::test]
    async fn oversized_payload_is_413() {
        let app = router(state());
        let big = "x".repeat(70 * 1024);
        let response = app
            .oneshot(publish_request(
                Some(TOKEN),
                serde_json::json!({"subject":"shared-auth.events.big","payload": big}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn readyz_reflects_broker_state_and_healthz_is_alive() {
        let app = router(state()); // Mock publisher → ready
        let ready = app
            .clone()
            .oneshot(
                axum::http::Request::get("/readyz")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ready.status(), StatusCode::OK);

        let health = app
            .oneshot(
                axum::http::Request::get("/healthz")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(health.status(), StatusCode::OK);
        let body = health.into_body().collect().await.unwrap().to_bytes();
        assert!(String::from_utf8_lossy(&body).contains("ok"));
    }

    #[tokio::test]
    async fn disconnected_nats_publisher_reports_degraded() {
        let mut s = state();
        // A real Nats publisher whose slot is still empty (connect loop pending).
        s.publisher = Publisher::Nats(Arc::new(tokio::sync::RwLock::new(None)));
        let app = router(s);
        let ready = app
            .clone()
            .oneshot(
                axum::http::Request::get("/readyz")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ready.status(), StatusCode::SERVICE_UNAVAILABLE);

        let publish = app
            .oneshot(publish_request(
                Some(TOKEN),
                serde_json::json!({"subject":"shared-auth.events.x","payload":{}}),
            ))
            .await
            .unwrap();
        assert_eq!(publish.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn metrics_route_is_prometheus_and_content_free() {
        let s = state();
        s.metrics.publishes.with_label_values(&["ok"]).inc();
        s.metrics
            .deliveries
            .with_label_values(&["shared-auth.events.identity", "ok"])
            .inc();
        let app = router(s);
        let response = app
            .oneshot(
                axum::http::Request::get("/metrics")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/plain")));
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("shared_auth_bridge_publishes_total"));
        assert!(body.contains("shared_auth_bridge_deliveries_total"));
        for forbidden_label in [
            "email=",
            "identity=",
            "jwt=",
            "token=",
            "token_prefix=",
            "url=",
        ] {
            assert!(!body.contains(forbidden_label));
        }
    }
}
