use std::{
    error::Error,
    net::SocketAddr,
    sync::{Arc, RwLock},
    time::Duration,
};

use axum::{
    extract::DefaultBodyLimit,
    http::{header, HeaderName, HeaderValue, StatusCode},
    routing::{get, post},
    Router,
};
use tower_http::{
    limit::RequestBodyLimitLayer, set_header::SetResponseHeaderLayer, timeout::TimeoutLayer,
    trace::TraceLayer,
};
use tracing::{error, info};

mod ai;
mod auth;
mod claims;
mod deadlines;
mod fees;
mod handlers;
mod package;
mod state;
#[cfg(test)]
mod tests;
mod types;
mod ui;
mod util;

use crate::handlers::*;
use crate::state::{config_from_env, env_value, AppState, Metrics, PatentStore};

const SERVICE_NAME: &str = "dd-patent-filing-rs";
const SCHEMA_VERSION: &str = "patent_filing.package.v1";
const MAX_HTTP_BODY_BYTES: usize = 1024 * 1024;
const MAX_MATTERS_DEFAULT: usize = 200;
const MAX_TEXT_LEN: usize = 24_000;
const MAX_SHORT_TEXT_LEN: usize = 1_000;
const MAX_LIST_ITEMS: usize = 64;
const MAX_TOKEN_LEN: usize = 160;
const MAX_CLAIMS: usize = 200;
const ABSTRACT_WORD_LIMIT: usize = 150;
const REQUEST_TIMEOUT_SECS: u64 = 15;
/// AI drafting can take much longer than the deterministic endpoints (model
/// thinking + generation), so it gets its own request + HTTP timeouts.
const AI_REQUEST_TIMEOUT_SECS: u64 = 150;
const AI_HTTP_TIMEOUT_SECS: u64 = 140;
const AI_MAX_TOKENS: u32 = 12_000;
const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Upper bound on the prompt brief sent to the model, so a large intake cannot
/// amplify into unbounded model cost.
const AI_BRIEF_MAX_CHARS: usize = 20_000;
/// Cap on upstream error text echoed back to clients.
const AI_ERROR_SNIPPET_CHARS: usize = 500;
/// USPTO fee schedule effective date encoded in [`fee_schedule`].
const FEE_EFFECTIVE_DATE: &str = "2025-01-19";
/// Pinned htmx asset + Subresource Integrity hash (supply-chain hardening).
const HTMX_SRC: &str = "https://unpkg.com/htmx.org@1.9.12/dist/htmx.min.js";
const HTMX_SRI: &str = "sha384-ujb1lZYygJmzgSwoxRggbCHcjc0rB2XoQrxeTUQyRjrOnlCoYta87iKBWq3EsdM2";

fn security_header_layers() -> [SetResponseHeaderLayer<HeaderValue>; 5] {
    let csp = "default-src 'self'; \
               script-src 'self' https://unpkg.com; \
               style-src 'self' 'unsafe-inline'; \
               img-src 'self' data:; \
               connect-src 'self'; \
               base-uri 'none'; \
               form-action 'self'; \
               frame-ancestors 'none'";
    [
        SetResponseHeaderLayer::overriding(
            HeaderName::from_static("x-content-type-options"),
            HeaderValue::from_static("nosniff"),
        ),
        SetResponseHeaderLayer::overriding(
            HeaderName::from_static("x-frame-options"),
            HeaderValue::from_static("DENY"),
        ),
        SetResponseHeaderLayer::overriding(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ),
        SetResponseHeaderLayer::overriding(
            HeaderName::from_static("cross-origin-opener-policy"),
            HeaderValue::from_static("same-origin"),
        ),
        SetResponseHeaderLayer::overriding(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(csp),
        ),
    ]
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(error) => error!(%error, "failed to install SIGTERM handler"),
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => info!("received SIGINT, beginning graceful shutdown"),
        _ = terminate => info!("received SIGTERM, beginning graceful shutdown"),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let _otel = dd_telemetry::init(SERVICE_NAME);
    let host = env_value("HOST", "0.0.0.0");
    let port = env_value("PORT", "8116").parse::<u16>()?;
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(AI_HTTP_TIMEOUT_SECS))
        .user_agent(format!("{SERVICE_NAME}/0.2"))
        // The API client must never follow redirects: reqwest does not strip the
        // custom `x-api-key` header on cross-host redirects, so a redirecting or
        // hijacked base URL could leak the key to another host.
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let config = config_from_env();
    let ai_permits = Arc::new(tokio::sync::Semaphore::new(config.ai_max_concurrency));
    let state = AppState {
        config: Arc::new(config),
        metrics: Arc::new(Metrics::default()),
        store: Arc::new(RwLock::new(PatentStore::default())),
        http,
        ai_permits,
    };

    let [sec0, sec1, sec2, sec3, sec4] = security_header_layers();
    // AI drafting needs a much longer per-request timeout than the deterministic
    // endpoints, so it lives on its own sub-router with its own TimeoutLayer.
    let ai_routes = Router::new()
        .route("/draft/ai", post(draft_ai_json))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(AI_REQUEST_TIMEOUT_SECS),
        ));
    let fast_routes = Router::new()
        .route("/", get(root))
        .route("/descriptor", get(descriptor))
        .route("/schema", get(schema))
        .route("/example", get(example))
        .route("/matters", get(matters))
        .route("/matters/:matter_id", get(matter))
        .route("/packages/provisional", post(package_json))
        .route("/ui/packages", post(package_form))
        .route("/readiness", post(readiness_json))
        .route("/search/plan", post(search_plan_json))
        .route("/review/package", post(review_package_json))
        .route("/claims/check", post(claims_check_json))
        .route("/fees/estimate", post(fees_estimate_json))
        .route("/deadlines", post(deadlines_json))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(REQUEST_TIMEOUT_SECS),
        ));
    let app = fast_routes
        .merge(ai_routes)
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .layer(RequestBodyLimitLayer::new(MAX_HTTP_BODY_BYTES))
        .layer(sec0)
        .layer(sec1)
        .layer(sec2)
        .layer(sec3)
        .layer(sec4)
        .layer(TraceLayer::new_for_http())
        .with_state(state.clone())
        .merge(dd_runtime_config_client::router());

    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(
        addr = %addr,
        auth_configured = state.config.server_auth_secret.is_some(),
        allow_unauthenticated = state.config.allow_unauthenticated,
        "{SERVICE_NAME} listening"
    );
    axum::serve(listener, app.layer(dd_telemetry::http_trace_layer()))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    info!("{SERVICE_NAME} shut down cleanly");
    Ok(())
}
