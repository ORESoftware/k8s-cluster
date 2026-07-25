//! HTTP surface: router composition and cross-cutting middleware.
//!
//! Routes:
//!   POST /v1/auth/shared     -> enroll a device via a shared-auth access token
//!   POST /v1/auth/supabase   -> compatibility exchange of a Supabase access JWT
//!   GET  /v1/devices         -> list this account's devices   (auth)
//!   POST /v1/devices/revoke  -> revoke a device   (auth)
//!   GET  /v1/vault           -> pull sealed blob   (auth)
//!   POST /v1/vault           -> push sealed blob   (auth)
//!   GET  /livez              -> liveness (no DB)   (/healthz: back-compat alias)
//!   GET  /readyz             -> readiness (DB ping)
//!
//! Unauthenticated identity routes are per-client rate-limited; all routes are
//! body-size capped and wrapped in a request timeout (see [`router`]).

use crate::state::AppState;
use crate::{devices, health, metrics, supabase_auth, telemetry, vault_blob};
use axum::http::{header, HeaderName, HeaderValue, StatusCode};
use axum::middleware;
use axum::routing::{get, post};
use axum::Router;
use std::sync::Arc;
use std::time::Duration;
use tower_governor::governor::GovernorConfigBuilder;
use tower_governor::key_extractor::SmartIpKeyExtractor;
use tower_governor::GovernorLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::TimeoutLayer;

/// Per-request wall-clock budget. Bounds slow/stuck handlers.
const REQUEST_TIMEOUT_SECS: u64 = 15;

pub fn router(state: AppState) -> Router {
    // GCRA: replenish ~1 request/s with a small burst. SmartIpKeyExtractor uses
    // trusted ingress forwarding headers and falls back to the socket peer.
    //
    // Trust assumption (load-bearing): SmartIpKeyExtractor keys on
    // X-Forwarded-For, which any client could spoof to dodge the limiter — it
    // is trustworthy here ONLY because deploy/k8s/networkpolicy.yaml admits
    // ingress traffic from ingress-nginx alone, which overwrites the header.
    // Loosening that NetworkPolicy silently breaks rate limiting.
    let governor = Arc::new(
        GovernorConfigBuilder::default()
            .key_extractor(SmartIpKeyExtractor)
            .per_second(1)
            .burst_size(8)
            .finish()
            .expect("valid rate-limit config"),
    );

    // Authed routes get a more generous per-IP budget: normal sync traffic is
    // bursty (pull + push + device list in quick succession) but a stolen sync
    // token still can't hammer the vault or enumerate devices unthrottled.
    let authed_governor = Arc::new(
        GovernorConfigBuilder::default()
            .key_extractor(SmartIpKeyExtractor)
            .per_second(1)
            .burst_size(30)
            .finish()
            .expect("valid rate-limit config"),
    );

    let auth_routes = Router::new()
        .route("/v1/auth/shared", post(supabase_auth::enroll_shared))
        .route("/v1/auth/supabase", post(supabase_auth::enroll_provider))
        // Route-only layering preserves the outer router's normal 404 fallback.
        .route_layer(GovernorLayer { config: governor });

    let authed_routes = Router::new()
        .route("/v1/devices", get(devices::list_handler))
        .route("/v1/devices/revoke", post(devices::revoke_handler))
        .route(
            "/v1/vault",
            get(vault_blob::pull_handler).post(vault_blob::push_handler),
        )
        .route_layer(GovernorLayer {
            config: authed_governor,
        });

    Router::new()
        // Liveness must not depend on the DB; readiness does. These stay
        // unthrottled: kubelet probes and Prometheus scrapes share a node IP.
        .route("/livez", get(health::live))
        .route("/healthz", get(health::live))
        .route("/readyz", get(health::ready))
        .route("/metrics", get(health::prometheus))
        .merge(auth_routes)
        .merge(authed_routes)
        .layer(telemetry::http_trace_layer())
        .layer(middleware::from_fn_with_state(
            state.metrics.clone(),
            metrics::record_http_metrics,
        ))
        // Defense-in-depth headers for a JSON API behind TLS ingress.
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=63072000; includeSubDomains"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("x-frame-options"),
            HeaderValue::from_static("DENY"),
        ))
        .layer(RequestBodyLimitLayer::new(1024 * 1024))
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
        AppState::new(database).expect("test state")
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
    async fn router_exposes_only_shared_auth_identity_enrollment() {
        let app = router(test_state());
        for path in ["/v1/auth/shared", "/v1/auth/supabase"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(path)
                        .header(header::AUTHORIZATION, "Bearer test-token")
                        .header(header::CONTENT_TYPE, "application/json")
                        .header("x-forwarded-for", "127.0.0.1")
                        .body(Body::from(r#"{"device_name":"test desktop"}"#))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::NOT_IMPLEMENTED,
                "configured route for {path}"
            );
        }

        for retired in ["/v1/register", "/v1/login"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(retired)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{retired}");
        }
    }

    #[tokio::test]
    async fn authed_routes_are_rate_limited_but_probes_are_not() {
        let app = router(test_state());

        // Drive one client IP past the authed burst budget: the governor must
        // start rejecting with 429 before the handler runs.
        let mut throttled = false;
        for _ in 0..40 {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/v1/vault")
                        .header("x-forwarded-for", "203.0.113.7")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            if response.status() == StatusCode::TOO_MANY_REQUESTS {
                throttled = true;
                break;
            }
        }
        assert!(throttled, "/v1/vault must carry the per-IP governor");

        // Probe routes share the kubelet's node IP and must stay unthrottled.
        for _ in 0..40 {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/livez")
                        .header("x-forwarded-for", "203.0.113.7")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }
    }

    #[tokio::test]
    async fn router_middleware_records_requests_and_sets_security_headers() {
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
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            response.headers()[header::X_CONTENT_TYPE_OPTIONS],
            "nosniff"
        );

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
            "SHARED_AUTH_BASE_URL",
            "sea_orm=warn",
        ] {
            assert!(manifest.contains(required), "manifest missing {required}");
        }
        assert!(!manifest.contains("sqlx=warn"));
        assert!(!manifest.contains("SUPABASE_JWT_LEGACY_SECRET"));

        let network_policy = include_str!("../deploy/k8s/networkpolicy.yaml");
        for required in ["app: dd-remote-gateway", "port: 80", "port: 4318"] {
            assert!(
                network_policy.contains(required),
                "network policy missing {required}"
            );
        }

        // The ExternalSecret must target the cluster's real store and the GA
        // API version, or the DSN never materializes.
        let external_secret = include_str!("../deploy/k8s/externalsecret.yaml");
        for required in [
            "apiVersion: external-secrets.io/v1",
            "name: dd-cluster-secrets",
            "kind: ClusterSecretStore",
        ] {
            assert!(
                external_secret.contains(required),
                "external secret missing {required}"
            );
        }
        assert!(!external_secret.contains("external-secrets.io/v1beta1"));
    }
}
