//! The axum surface.
//!
//! JSON API + a small script-free Maud HTML UI. No websockets.
//! - `GET  /`                           status landing (HTML)
//! - `GET  /ui`                         token-exchange helper (HTML)
//! - `POST /ui/exchange`                exchange result (HTML)
//! - `GET  /authorize`                  browser sign-in for registered clients
//! - `POST /authorize`                  issue a PKCE-bound one-time code
//! - `POST /auth/handoff/redeem`        backend-only code redemption
//! - `GET  /healthz`                    liveness
//! - `GET  /readyz`                     readiness (DB ping if configured)
//! - `GET  /.well-known/jwks.json`      our public JWKS (downstream verifiers)
//! - `POST /auth/exchange`              provider access token → OreSoftware JWT
//! - `POST /auth/delegate`              OreSoftware JWT → narrow product JWT
//! - `POST /auth/introspect`            validate an OreSoftware JWT → claims
//! - `GET  /auth/verify`                bearer check (gateway auth_request)
//! - `GET  /metrics`                    Prometheus

mod delegate;
mod docs;
mod exchange;
mod handoff;
mod health;
pub(crate) mod introspect;
mod jwks;
mod local;
mod metrics;
mod mfa;
mod passwordless;
pub(crate) mod session_tokens;
mod ui;
pub mod webhook;

use std::time::Duration;

use axum::{
    http::{header, HeaderName, HeaderValue, Method},
    routing::{delete, get, post},
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
        .route(
            "/authorize",
            get(handoff::authorize).post(handoff::authorize_password),
        )
        .route("/docs/api", get(docs::api_docs))
        .route("/api/docs", get(docs::api_docs))
        .route("/api/docs.json", get(docs::openapi))
        // JSON API
        .route("/healthz", get(health::healthz))
        .route("/readyz", get(health::readyz))
        .route("/.well-known/jwks.json", get(jwks::jwks))
        .route("/auth/exchange", post(exchange::exchange))
        .route("/auth/delegate", post(delegate::delegate))
        .route("/auth/handoff/redeem", post(handoff::redeem))
        .route("/auth/register", post(local::register))
        .route("/auth/login", post(local::login))
        .route("/auth/passwordless/request", post(passwordless::request))
        .route("/auth/passwordless/consume", post(passwordless::consume))
        .route("/auth/mfa/sms/request", post(mfa::request_sms))
        .route("/auth/mfa/sms/verify", post(mfa::verify_sms))
        .route("/auth/capabilities", get(crate::factors::capabilities))
        .route("/auth/factors", get(crate::factors::list))
        .route(
            "/auth/factors/{factorId}",
            delete(crate::factors::delete),
        )
        .route(
            "/auth/factors/totp/enroll",
            post(crate::factors::enroll_totp),
        )
        .route(
            "/auth/factors/totp/confirm",
            post(crate::factors::confirm_totp),
        )
        .route(
            "/auth/challenges",
            post(crate::factors::create_challenge),
        )
        .route(
            "/auth/challenges/{challengeId}/verify",
            post(crate::factors::verify_challenge),
        )
        .route(
            "/auth/passkeys/registration/options",
            post(crate::factors::start_passkey_registration),
        )
        .route(
            "/auth/passkeys/registration/verify",
            post(crate::factors::finish_passkey_registration),
        )
        .route(
            "/auth/passkeys/authentication/options",
            post(crate::factors::start_passkey_authentication),
        )
        .route(
            "/auth/passkeys/authentication/verify",
            post(crate::factors::finish_passkey_authentication),
        )
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
        // Token-bearing responses must not enter shared or browser caches.
        // if_not_present preserves the explicit public JWKS cache policy.
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
        // The script-free UI needs no powerful browser capabilities.
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("permissions-policy"),
            HeaderValue::from_static(
                "accelerometer=(), camera=(), display-capture=(), geolocation=(), gyroscope=(), magnetometer=(), microphone=(), payment=(), usb=()",
            ),
        ))
        // Isolate authentication UI from cross-origin opener relationships.
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
        .filter_map(|origin| origin.parse().ok())
        .collect::<Vec<_>>();
    CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([Method::GET, Method::POST, Method::DELETE, Method::OPTIONS])
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
        .filter(|token| !token.is_empty())
}
