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
        .route_layer(GovernorLayer::new(governor));

    let device_routes = Router::new()
        .route("/v1/devices", get(devices::list_handler))
        .route("/v1/devices/revoke", post(devices::revoke_handler))
        .route_layer(RequestBodyLimitLayer::new(DEFAULT_BODY_LIMIT))
        .route_layer(GovernorLayer::new(Arc::clone(&authed_governor)));

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
        .route_layer(GovernorLayer::new(Arc::clone(&authed_governor)));

    let signal_routes = if signal_sync_enabled {
        signal_api::routes()
            .route_layer(DefaultBodyLimit::max(VAULT_BODY_LIMIT))
            .route_layer(RequestBodyLimitLayer::new(VAULT_BODY_LIMIT))
            .route_layer(GovernorLayer::new(authed_governor))
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
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/plain; version=0.0.4"
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        // An unlabelled gauge always renders, even on a registry that has not
        // served a request yet; the labelled families are covered by
        // `router_middleware_records_requests_and_sets_security_headers`.
        assert!(String::from_utf8(body.to_vec())
            .unwrap()
            .contains("threefa_http_requests_in_flight"));

        // And it serves nothing else: no API surface leaks onto the port the
        // observability namespace is allowed to reach.
        for path in ["/livez", "/readyz", "/v1/vault", "/v1/devices"] {
            let response = metrics_router(test_state())
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
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

        // The unauthenticated enrollment budget is intentionally smaller than
        // the sync budget. Exercise that independent GovernorLayer instance so
        // an API migration cannot silently leave the highest-risk routes
        // unthrottled while the authed layer below still works.
        let mut auth_throttled = None;
        for _ in 0..16 {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/v1/auth/shared")
                        .header("x-forwarded-for", "203.0.113.6")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            if response.status() == StatusCode::TOO_MANY_REQUESTS {
                auth_throttled = Some(response);
                break;
            }
        }
        let auth_throttled =
            auth_throttled.expect("/v1/auth/shared must carry the per-IP governor");
        assert!(
            auth_throttled.headers().contains_key(header::RETRY_AFTER),
            "the limiter response must tell clients when retrying is safe"
        );

        // Drive one client IP past the authed burst budget: the governor must
        // start rejecting with 429 before the handler runs.
        let mut throttled = None;
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
                throttled = Some(response);
                break;
            }
        }
        let throttled = throttled.expect("/v1/vault must carry the per-IP governor");
        assert!(
            throttled.headers().contains_key(header::RETRY_AFTER),
            "the limiter response must tell clients when retrying is safe"
        );

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
        let state = test_state();
        let app = router(state.clone());
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

        // The middleware records against the shared registry, so the request
        // above is visible on the telemetry listener even though the two
        // routers no longer share a socket.
        let response = metrics_router(state)
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

    #[tokio::test]
    async fn a_max_ciphertext_len_push_is_not_refused_by_the_body_limit() {
        // The regression: `MAX_CIPHERTEXT_LEN` was unreachable over HTTP because
        // the router capped every body at 1 MiB while a 512 KiB ciphertext
        // serializes to ~2 MiB of JSON integer array. Worst case (`255` in every
        // byte, 4 chars each) must reach the handler, not the limit layer.
        let token = "a-test-sync-token";
        let device = crate::entity::device::Model {
            id: uuid::Uuid::new_v4(),
            account_id: uuid::Uuid::new_v4(),
            device_name: "body limit probe".to_owned(),
            sync_token_hash: crate::auth::token_hash(token),
            revoked: false,
            created_at: time::OffsetDateTime::now_utc(),
            last_seen_at: None,
        };

        let body = serde_json::to_vec(&crate::protocol::PushRequest {
            device_id: device.id.to_string(),
            blob: crate::protocol::SealedBlob {
                ciphertext: vec![255u8; crate::protocol::MAX_CIPHERTEXT_LEN],
                nonce: vec![255u8; crate::protocol::NONCE_LEN],
                kdf_salt: vec![255u8; crate::protocol::MAX_KDF_SALT_LEN],
                kdf_params: crate::protocol::KdfParams::default(),
            },
            base_version: Vec::new(),
        })
        .expect("serializable push request");
        assert!(
            body.len() > DEFAULT_BODY_LIMIT,
            "fixture precondition: this payload used to be refused ({} bytes)",
            body.len()
        );
        assert!(
            body.len() < VAULT_BODY_LIMIT,
            "the documented arithmetic must leave headroom: {} bytes vs {VAULT_BODY_LIMIT}",
            body.len()
        );

        // Authenticate for real, so the body is actually read and deserialized.
        // A test that stops at 401 would pass even with the body cap still too
        // low, because the credential check now runs first — that is exactly how
        // the first version of this test missed the axum `DefaultBodyLimit`.
        let database = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results([vec![device]])
            .append_exec_results([MockExecResult::default()])
            .into_connection();
        let state = AppState::new(database).expect("test state");

        let response = router(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/vault")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::CONTENT_LENGTH, body.len())
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header("x-forwarded-for", "203.0.113.9")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        // The mock has no rows left for `store()`, so the handler ends in a 500.
        // That is the assertion: the request got past the body cap (413), past
        // authentication (401) and past deserialization + envelope validation
        // (400) — the full-size ciphertext was accepted as conforming.
        assert_eq!(
            response.status(),
            StatusCode::INTERNAL_SERVER_ERROR,
            "a full-size conforming vault push must reach the handler, not a 413/401/400"
        );
    }

    #[tokio::test]
    async fn the_vault_body_cap_still_refuses_a_grossly_oversized_push() {
        let body = vec![b'x'; VAULT_BODY_LIMIT + 1];
        let response = router(test_state())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/vault")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::CONTENT_LENGTH, body.len())
                    .header("x-forwarded-for", "203.0.113.10")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn non_vault_routes_keep_the_smaller_default_body_cap() {
        let body = vec![b'x'; DEFAULT_BODY_LIMIT + 1];
        for path in ["/v1/devices/revoke", "/v1/auth/shared"] {
            let response = router(test_state())
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(path)
                        .header(header::CONTENT_TYPE, "application/json")
                        .header(header::CONTENT_LENGTH, body.len())
                        .header("x-forwarded-for", "203.0.113.11")
                        .body(Body::from(body.clone()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::PAYLOAD_TOO_LARGE,
                "the larger vault cap must not leak onto {path}"
            );
        }
    }

    #[tokio::test]
    async fn an_unauthenticated_post_with_a_malformed_body_is_401_not_a_body_error() {
        // Credentials are checked before the body is parsed, so an anonymous
        // caller cannot use rejection messages to enumerate the wire type.
        for path in [
            "/v1/vault",
            "/v1/devices/revoke",
            "/v1/signal/envelopes",
            "/v1/signal/mailbox/ack",
        ] {
            let response = router_with_signal(test_state(), true)
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(path)
                        .header(header::CONTENT_TYPE, "application/json")
                        .header("x-forwarded-for", "203.0.113.12")
                        .body(Body::from(r#"{"totally":"wrong"}"#))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
            let body = response.into_body().collect().await.unwrap().to_bytes();
            let body = String::from_utf8(body.to_vec()).unwrap();
            assert_eq!(body, "unauthorized");
            assert!(
                !body.contains("missing field"),
                "no schema detail may reach an unauthenticated caller: {body}"
            );
        }
    }

    #[test]
    fn kubernetes_manifest_matches_operational_and_telemetry_routes() {
        // `/metrics` is not on the public router any more (see `metrics_router`
        // and the route table above); the manifests must describe the same
        // two-port arrangement or the pod ships unscrapeable, or worse, with
        // telemetry reachable from the Ingress again.
        let manifest = include_str!("../deploy/k8s/deployment.yaml");
        for required in [
            "path: /livez",
            "path: /readyz",
            "prometheus.io/path: /metrics",
            // The scrape must target the telemetry port, not the API port.
            "prometheus.io/port: \"9091\"",
            "name: metrics",
            "containerPort: 9091",
            "METRICS_BIND_ADDR",
            "value: \"0.0.0.0:9091\"",
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            "DEPLOYMENT_ENVIRONMENT",
            "SHARED_AUTH_BASE_URL",
            "sea_orm=warn",
            // SeaORM's statement logging is emitted under the `sqlx::query`
            // target, which `sea_orm=warn` does not match — without this
            // directive production logs every SQL statement it issues at INFO.
            "sqlx::query=warn",
        ] {
            assert!(manifest.contains(required), "manifest missing {required}");
        }
        assert!(
            telemetry::default_log_filter().contains("sqlx::query=warn"),
            "the built-in default must silence per-statement SQL too, so a deployment that \
             forgets RUST_LOG does not ship the noisy filter"
        );
        assert!(
            !manifest.contains("prometheus.io/port: \"8080\""),
            "the scrape annotation must not point at the public API port"
        );
        assert!(!manifest.contains("sqlx=warn"));
        assert!(!manifest.contains("SUPABASE_JWT_LEGACY_SECRET"));

        let service = include_str!("../deploy/k8s/service.yaml");
        for required in ["name: metrics", "port: 9091", "targetPort: metrics"] {
            assert!(service.contains(required), "service missing {required}");
        }

        let network_policy = include_str!("../deploy/k8s/networkpolicy.yaml");
        for required in [
            "app: dd-remote-gateway",
            "port: 80",
            "port: 4318",
            "port: 9091",
        ] {
            assert!(
                network_policy.contains(required),
                "network policy missing {required}"
            );
        }

        // ingress-nginx must reach 8080 and ONLY 8080; the observability
        // namespace must reach 9091 and ONLY 9091. Assert on each `from:` block
        // separately so a merged or widened rule fails loudly.
        let (ingress_rule, observability_rule) = network_policy
            .split_once("kubernetes.io/metadata.name: observability")
            .expect("networkpolicy names the observability namespace");
        assert!(
            ingress_rule.contains("kubernetes.io/metadata.name: ingress-nginx"),
            "the first ingress rule must be the ingress-nginx one"
        );
        assert!(
            ingress_rule.contains("port: 8080") && !ingress_rule.contains("port: 9091"),
            "ingress-nginx must not be able to reach the telemetry port"
        );
        let observability_ports: &str = observability_rule
            .split("egress:")
            .next()
            .expect("ingress section ends at egress");
        assert!(
            observability_ports.contains("port: 9091")
                && !observability_ports.contains("port: 8080"),
            "the observability namespace must be admitted to 9091 only"
        );

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
