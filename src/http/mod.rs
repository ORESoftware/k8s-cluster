//! The axum surface.
//!
//! JSON API + a small script-free Maud HTML UI. No websockets.
//! - `GET  /`                           status landing (HTML)
//! - `GET  /ui`                          token-exchange helper (HTML)
//! - `POST /ui/exchange`                 exchange result (HTML)
//! - `GET  /healthz`                     liveness
//! - `GET  /readyz`                      readiness (DB ping if configured)
//! - `GET  /.well-known/jwks.json`       our public JWKS (downstream verifiers)
//! - `POST /auth/exchange`               Supabase access token → OreSoftware JWT
//! - `POST /auth/introspect`             validate an OreSoftware JWT → claims
//! - `GET  /auth/verify`                 bearer check (gateway auth_request)
//! - `GET  /metrics`                     Prometheus

mod docs;
mod exchange;
mod health;
mod introspect;
mod jwks;
mod local;
mod metrics;
mod mfa;
mod passwordless;
mod session_tokens;
mod ui;
pub mod webhook;

use std::time::Duration;

use axum::{
    http::{header, HeaderName, HeaderValue, Method},
    routing::{get, post},
    Router,
};
use tower_http::cors::CorsLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::TimeoutLayer;

use crate::state::AppState;

// TimeoutLayer::new is deprecated in tower-http 0.6 in favour of
// with_status_code; new() still produces the same 408-on-timeout behaviour we
// want, so allow it rather than pin to the newer signature.
#[allow(deprecated)]
pub fn router(state: AppState) -> Router {
    let cors = build_cors(&state);

    Router::new()
        // HTML UI
        .route("/", get(ui::landing))
        .route("/ui", get(ui::sign_in))
        .route("/ui/exchange", post(ui::ui_exchange))
        .route("/docs/api", get(docs::api_docs))
        .route("/api/docs", get(docs::api_docs))
        .route("/api/docs.json", get(docs::openapi))
        // JSON API
        .route("/healthz", get(health::healthz))
        .route("/readyz", get(health::readyz))
        .route("/.well-known/jwks.json", get(jwks::jwks))
        .route("/auth/exchange", post(exchange::exchange))
        .route("/auth/register", post(local::register))
        .route("/auth/login", post(local::login))
        .route("/auth/passwordless/request", post(passwordless::request))
        .route("/auth/passwordless/consume", post(passwordless::consume))
        .route("/auth/mfa/sms/request", post(mfa::request_sms))
        .route("/auth/mfa/sms/verify", post(mfa::verify_sms))
        .route("/auth/refresh", post(local::refresh))
        .route("/auth/logout", post(local::logout))
        .route("/auth/introspect", post(introspect::introspect))
        .route("/auth/verify", get(introspect::verify))
        .route("/internal/webhook/sync", post(webhook::sync_webhook))
        .route("/metrics", get(metrics::metrics))
        // One tracing span per request (W3C traceparent → OTLP), then limits.
        .layer(crate::telemetry::http_trace_layer())
        .layer(TimeoutLayer::new(Duration::from_secs(10)))
        .layer(RequestBodyLimitLayer::new(64 * 1024))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'",
            ),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_FRAME_OPTIONS,
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000; includeSubDomains"),
        ))
        // Token-bearing bodies — the /ui/exchange minted-JWT page, the /auth/*
        // JSON access/refresh tokens, and introspect claims — must never sit in
        // a shared or browser cache. `if_not_present` leaves the JWKS handler's
        // explicit `public, max-age=300` intact, since that is public key
        // material meant to be cached by downstream verifiers.
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
        // The UI uses no powerful browser features; deny them all so a
        // compromised or MITM'd response cannot silently request them.
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("permissions-policy"),
            HeaderValue::from_static(
                "accelerometer=(), camera=(), display-capture=(), geolocation=(), gyroscope=(), magnetometer=(), microphone=(), payment=(), usb=()",
            ),
        ))
        // Isolate the browsing context — no cross-origin window handle can reach
        // an auth page opened from, or opening, another origin.
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("cross-origin-opener-policy"),
            HeaderValue::from_static("same-origin"),
        ))
        .layer(cors)
        .with_state(state)
}

fn build_cors(state: &AppState) -> CorsLayer {
    if state.config.cors_allow_origins.is_empty() {
        return CorsLayer::new();
    }
    let origins = state
        .config
        .cors_allow_origins
        .iter()
        .filter_map(|o| o.parse().ok())
        .collect::<Vec<_>>();
    CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
}

/// Extract a bearer token from the `Authorization` header.
pub(crate) fn bearer(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|t| !t.is_empty())
}
