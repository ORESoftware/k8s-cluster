//! HTTP surface: router composition and cross-cutting middleware.
//!
//! Routes:
//!   POST /v1/auth/shared     -> enroll a device via a shared-auth access token
//!   POST /v1/auth/supabase   -> compatibility exchange of a Supabase access JWT
//!   GET  /v1/devices         -> list this account's devices   (auth)
//!   POST /v1/devices/revoke  -> revoke a device   (auth)
//!   GET  /v1/vault           -> pull sealed blob   (auth)
//!   POST /v1/vault           -> push sealed blob   (auth)
//!   PUT  /v1/signal/prekeys  -> publish public Signal prekeys (auth, flag)
//!   POST /v1/signal/envelopes -> enqueue opaque recipient ciphertext (auth, flag)
//!   GET  /v1/signal/mailbox  -> pull opaque recipient ciphertext (auth, flag)
//!   POST /v1/signal/mailbox/{id}/ack -> acknowledge local apply (auth, flag)
//!   GET  /livez              -> liveness (no DB)   (/healthz: back-compat alias)
//!   GET  /readyz             -> readiness (DB ping)
//!
//! `/metrics` is deliberately NOT here: it is served by [`metrics_router`] on a
//! separate listener (`METRICS_BIND_ADDR`, default `0.0.0.0:9091`) that the
//! public Ingress does not front. See [`metrics_router`].
//!
//! Unauthenticated identity routes are per-client rate-limited; all routes are
//! body-size capped and wrapped in a request timeout (see [`router`]).

use crate::state::AppState;
use crate::{devices, health, metrics, signal_api, supabase_auth, telemetry, vault_blob};
use axum::extract::DefaultBodyLimit;
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

/// Body cap for every route that is not a vault push. Enrollment and device
/// bodies are a few hundred bytes; 1 MiB is already enormous for them.
const DEFAULT_BODY_LIMIT: usize = 1024 * 1024;

/// Body cap for the vault routes, sized so a *conforming* client can actually
/// send the published `MAX_CIPHERTEXT_LEN`.
///
/// The arithmetic, because the old 1 MiB cap made the published constant a lie
/// (a 512 KiB ciphertext was refused with 413 at ~300 KiB of plaintext bytes):
/// byte fields travel as JSON integer arrays, so each byte costs up to four
/// characters on the wire — `"255,"`.
///
///   ciphertext   512 KiB x 4 chars/byte              = 2 097 152
///   nonce        NONCE_LEN (24) x 4                  =       96
///   kdf_salt     MAX_KDF_SALT_LEN (64) x 4           =      256
///   kdf_params   three u32 fields, generously        =      128
///   device_id    MAX_DEVICE_ID_LEN (64) + quoting    =       80
///   base_version MAX_VERSION_ENTRIES (64) entries
///                x (64-char id + u64 counter + JSON) =   10 240
///   field names, brackets, commas                    =      256
///                                                      ---------
///                                            total    ~2 108 208  (~2.01 MiB)
///
/// 4 MiB is that rounded up to the next power of two, leaving headroom for a
/// client that pretty-prints or sends extra forward-compatible fields.
///
/// DoS note: this doubles the memory a single accepted request can buffer, so
/// it is applied to the vault routes ONLY — never globally, and never to the
/// unauthenticated enrollment routes. Both the vault routes and enrollment now
/// carry per-IP GCRA rate limiting (see below), so the sustained cost is capped
/// at burst x limit rather than by concurrency alone, and `store()` still
/// rejects anything past `MAX_CIPHERTEXT_LEN` before it touches the database.
const VAULT_BODY_LIMIT: usize = 4 * 1024 * 1024;

#[expect(
    dead_code,
    reason = "compatibility constructor retained for tests and embeddings while production startup passes the Signal rollout flag explicitly"
)]
pub fn router(state: AppState) -> Router {
    router_with_signal(state, false)
}

/// Compose the public router. Signal sync routes are absent unless startup
/// explicitly enables the guarded rollout flag.
pub fn router_with_signal(state: AppState, signal_sync_enabled: bool) -> Router {
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
        .route_layer(RequestBodyLimitLayer::new(DEFAULT_BODY_LIMIT))
        .route_layer(GovernorLayer { config: governor });

    let device_routes = Router::new()
        .route("/v1/devices", get(devices::list_handler))
        .route("/v1/devices/revoke", post(devices::revoke_handler))
        .route_layer(RequestBodyLimitLayer::new(DEFAULT_BODY_LIMIT))
        .route_layer(GovernorLayer {
            config: Arc::clone(&authed_governor),
        });

    // The one route that legitimately carries a large body. Kept in its own
    // sub-router so the larger cap cannot leak onto anything else; the GET on
    // the same path is bodyless and unaffected by the limit it inherits.
    //
    // BOTH caps have to be raised. `RequestBodyLimitLayer` bounds the transport;
    // `DefaultBodyLimit` is axum's own extractor-side limit, which defaults to
    // 2 MB — just under the ~2.01 MiB a worst-case conforming push needs, so
    // leaving it alone would have kept `MAX_CIPHERTEXT_LEN` unreachable with a
    // 413 raised from the `Bytes` extractor instead of the layer.
    let vault_routes = Router::new()
        .route(
            "/v1/vault",
            get(vault_blob::pull_handler).post(vault_blob::push_handler),
        )
        .route_layer(DefaultBodyLimit::max(VAULT_BODY_LIMIT))
        .route_layer(RequestBodyLimitLayer::new(VAULT_BODY_LIMIT))
        .route_layer(GovernorLayer {
            config: Arc::clone(&authed_governor),
        });

    let signal_routes = if signal_sync_enabled {
        signal_api::routes()
            .route_layer(DefaultBodyLimit::max(VAULT_BODY_LIMIT))
            .route_layer(RequestBodyLimitLayer::new(VAULT_BODY_LIMIT))
            .route_layer(GovernorLayer {
                config: authed_governor,
            })
    } else {
        Router::<AppState>::new()
    };

    Router::new()
        // Liveness must not depend on the DB; readiness does. These stay
        // unthrottled: kubelet probes and Prometheus scrapes share a node IP.
        .route("/livez", get(health::live))
        .route("/healthz", get(health::live))
        .route("/readyz", get(health::ready))
        .merge(auth_routes)
        .merge(device_routes)
        .merge(vault_routes)
        .merge(signal_routes)
        // Inside the trace layer, so every access-log line inherits the request
        // span's trace/span ids; outside the routes, so a 404, a 405, and a
        // rate-limited 429 are logged like everything else.
        .layer(middleware::from_fn(telemetry::log_http_response))
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
        // Outermost hard ceiling, so an unmatched path cannot be used to stream
        // an unbounded body at the process. Every *matched* route tightens this
        // with its own `route_layer` above (1 MiB, or `VAULT_BODY_LIMIT` for the
        // vault), and the inner limit is the one that fires.
        .layer(RequestBodyLimitLayer::new(VAULT_BODY_LIMIT))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(REQUEST_TIMEOUT_SECS),
        ))
        .with_state(state)
}

/// The telemetry listener: `/metrics` and nothing else.
///
/// `/metrics` used to sit on the public router, on the same port the Ingress
/// fronts, with no credential and outside both governors — so whether request
/// volumes, error rates, database latency, and `threefa_vault_conflicts_total`
/// (a side channel on how often accounts sync) were world-readable depended
/// entirely on an Ingress path rule in a *different* repository. Serving it from
/// a second socket makes that a property of the service: port 8080 has no
/// `/metrics` route to expose, and `deploy/k8s/networkpolicy.yaml` admits only
/// the observability namespace to 9091.
///
/// Deliberately not wrapped in the HTTP metrics middleware: a scrape should not
/// be a data point in the series it is scraping.
pub fn metrics_router(state: AppState) -> Router {
    Router::new()
        .route("/metrics", get(health::prometheus))
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
    use sea_orm::{DatabaseBackend, MockDatabase, MockExecResult};
    use tower::ServiceExt;

    fn test_state() -> AppState {
        let database = MockDatabase::new(DatabaseBackend::Postgres)
            // `/readyz` runs its probe through the instrumented query path
            // (`AppState::ping_database`), so the mock needs a canned result per
            // probe a test issues.
            .append_exec_results(vec![MockExecResult::default(); 8])
            .into_connection();
        AppState::new(database).expect("test state")
    }

    #[tokio::test]
    async fn operational_routes_are_composed_into_the_router() {
        for (path, expected) in [
            ("/livez", StatusCode::OK),
            ("/healthz", StatusCode::OK),
            ("/readyz", StatusCode::OK),
            ("/not-a-route", StatusCode::NOT_FOUND),
            // Telemetry moved to its own listener; the public router must not
            // serve it. This is the assertion that keeps it that way.
            ("/metrics", StatusCode::NOT_FOUND),
        ] {
            let response = router(test_state())
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), expected, "status for {path}");
        }
    }

    #[tokio::test]
    async fn metrics_are_served_only_by_the_separate_telemetry_router() {
        let response = metrics_router(test_state())
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/plain; version=0.0.4; charset=utf-8")
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert!(!body.is_empty());
    }

    #[tokio::test]
    async fn signal_routes_are_absent_until_the_rollout_flag_is_enabled() {
        let disabled = router_with_signal(test_state(), false)
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/v1/signal/prekeys")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(disabled.status(), StatusCode::NOT_FOUND);

        let enabled = router_with_signal(test_state(), true)
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/v1/signal/prekeys")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(enabled.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn public_router_sets_defense_in_depth_headers() {
        let response = router(test_state())
            .oneshot(
                Request::builder()
                    .uri("/livez")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response
                .headers()
                .get(header::X_CONTENT_TYPE_OPTIONS)
                .and_then(|value| value.to_str().ok()),
            Some("nosniff")
        );
        assert_eq!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-store")
        );
        assert_eq!(
            response
                .headers()
                .get(header::STRICT_TRANSPORT_SECURITY)
                .and_then(|value| value.to_str().ok()),
            Some("max-age=63072000; includeSubDomains")
        );
    }
}
