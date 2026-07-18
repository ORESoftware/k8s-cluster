//! HTTP surface: router, shared state, and request handlers.
//!
//! Routes:
//!   POST /v1/register        -> create account + first device, returns token
//!   POST /v1/login           -> verify account, register a device, returns token
//!   POST /v1/devices/revoke  -> revoke a device   (auth)
//!   GET  /v1/vault           -> pull sealed blob   (auth)
//!   POST /v1/vault           -> push sealed blob   (auth)
//!   GET  /livez              -> liveness (no DB)   (/healthz: back-compat alias)
//!   GET  /readyz             -> readiness (DB ping)
//!
//! The unauthenticated `/v1/register` and `/v1/login` routes are per-client
//! rate-limited; all routes are body-size capped and wrapped in a request
//! timeout (see [`router`]).

use crate::state::AppState;
use crate::{accounts, devices, health, metrics, telemetry, vault_blob};
use axum::http::StatusCode;
use axum::middleware;
use axum::routing::{get, post};
use axum::Router;
use std::sync::Arc;
use std::time::Duration;
use tower_governor::governor::GovernorConfigBuilder;
use tower_governor::key_extractor::SmartIpKeyExtractor;
use tower_governor::GovernorLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;
/// Per-request wall-clock budget. Bounds slow/stuck handlers.
const REQUEST_TIMEOUT_SECS: u64 = 15;

pub fn router(state: AppState) -> Router {
    // Per-client rate limit for the *unauthenticated* credential endpoints, which
    // are the online-brute-force / account-spam surface. GCRA: replenish ~1 req/s
    // with a small burst. The key is the client IP taken from `X-Forwarded-For` /
    // `X-Real-IP` (set by the trusted ingress) so all clients aren't collapsed to
    // the ingress pod's source address; it falls back to the socket peer.
    let governor = Arc::new(
        GovernorConfigBuilder::default()
            .key_extractor(SmartIpKeyExtractor)
            .per_second(1)
            .burst_size(8)
            .finish()
            .expect("valid rate-limit config"),
    );

    let auth_routes = Router::new()
        .route("/v1/register", post(accounts::register))
        .route("/v1/login", post(accounts::login))
        // Route-only layering preserves the outer router's normal 404 fallback.
        .route_layer(GovernorLayer { config: governor });

    Router::new()
        // Liveness: process is up. Must NOT depend on the DB, or a transient DB
        // blip would get the pod killed instead of merely pulled from rotation.
        .route("/livez", get(health::live))
        // Back-compat alias for the old liveness path.
        .route("/healthz", get(health::live))
        // Readiness: only serve traffic if the DB pool is actually usable.
        .route("/readyz", get(health::ready))
        .route("/metrics", get(health::prometheus))
        .merge(auth_routes)
        .route("/v1/devices/revoke", post(devices::revoke_handler))
        .route(
            "/v1/vault",
            get(vault_blob::pull_handler).post(vault_blob::push_handler),
        )
        // Outermost-to-innermost: request log, body cap, then a hard timeout.
        .layer(telemetry::http_trace_layer())
        .layer(middleware::from_fn_with_state(
            state.metrics.clone(),
            metrics::record_http_metrics,
        ))
        // Sealed blobs are small; cap bodies to 1 MiB to bound abuse.
        .layer(RequestBodyLimitLayer::new(1024 * 1024))
        // Bound every request's lifetime so slow/hung clients can't pin the small
        // connection pool (slowloris-style exhaustion).
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(REQUEST_TIMEOUT_SECS),
        ))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use http_body_util::BodyExt;
    use sea_orm::{DatabaseBackend, MockDatabase};
    use tower::ServiceExt;

    fn test_state() -> AppState {
        let database = MockDatabase::new(DatabaseBackend::Postgres).into_connection();
        AppState::new(database, 1).expect("test state")
    }

    #[tokio::test]
    async fn operational_routes_are_composed_into_the_router() {
        for (path, expected) in [
            ("/livez", StatusCode::OK),
            ("/healthz", StatusCode::OK),
            ("/readyz", StatusCode::OK),
            ("/metrics", StatusCode::OK),
            ("/not-a-route", StatusCode::NOT_FOUND),
        ] {
            let response = router(test_state())
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), expected, "status for {path}");
        }
    }

    #[tokio::test]
    async fn router_middleware_records_requests_for_prometheus() {
        let app = router(test_state());
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/livez")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/plain; version=0.0.4"
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("threefa_http_requests_total"));
        assert!(body.contains("route=\"/livez\""));
    }

    #[test]
    fn kubernetes_manifest_matches_operational_and_telemetry_routes() {
        let manifest = include_str!("../deploy/k8s/deployment.yaml");
        for required in [
            "path: /livez",
            "path: /readyz",
            "prometheus.io/path: /metrics",
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            "DEPLOYMENT_ENVIRONMENT",
            "sea_orm=warn",
        ] {
            assert!(manifest.contains(required), "manifest missing {required}");
        }
        assert!(!manifest.contains("sqlx=warn"));
    }
}
