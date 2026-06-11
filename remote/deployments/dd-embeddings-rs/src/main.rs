//! dd-embeddings-rs — multi-provider embedding gateway + RAG indexing service.
//!
//! Boot: read env -> build a shared HTTP client -> register every provider
//! whose credentials are present -> wire Qdrant -> serve axum. The service is
//! deliberately tolerant of missing credentials: it will boot with zero
//! providers and report that on `/api/providers`, so you can add keys to the
//! backing secret incrementally without redeploying.

mod config;
mod docs;
mod error;
mod providers;
mod rag;
mod state;
mod validate;

use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{middleware, Json, Router};
use serde::Deserialize;
use serde_json::json;
use tower_http::normalize_path::NormalizePathLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::error::ApiError;
use crate::providers::{EmbedRequest, Registry};
use crate::rag::qdrant::Qdrant;
use crate::rag::{IndexRequest, RagService, SearchRequest};
use crate::state::AppState;
use crate::validate::{
    check_dimensions, clamp_top_k, constant_time_eq, enforce_input_limits, validate_collection,
    validate_distance,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // .env is convenience for local runs; in-cluster config comes from the env.
    let _ = dotenvy::dotenv();
    let cfg = Config::from_env()?;
    init_tracing(cfg.log_format_json);

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(cfg.request_timeout_secs))
        .user_agent("dd-embeddings-rs/0.1")
        // Do not follow redirects: provider + Qdrant endpoints are fixed POST
        // targets that don't legitimately 3xx, and following a redirect could
        // be steered at an internal/metadata IP (SSRF).
        .redirect(reqwest::redirect::Policy::none())
        .build()?;

    let registry = Arc::new(Registry::from_config(&cfg, http.clone()));
    let qdrant = Arc::new(Qdrant::new(cfg.qdrant_url.clone(), cfg.qdrant_api_key.clone(), http.clone()));
    let rag = Arc::new(RagService::new(registry.clone(), qdrant.clone()));

    let provider_ids: Vec<&str> = registry.iter().map(|p| p.id()).collect();
    tracing::info!(
        providers = registry.len(),
        ids = ?provider_ids,
        aliases = ?registry.aliases(),
        "embedding providers registered"
    );
    if registry.is_empty() {
        tracing::warn!(
            "no embedding providers configured — set provider API keys in the \
             dd-embeddings-rs secret to enable them"
        );
    }

    if cfg.api_auth_bearer.is_none() {
        tracing::warn!(
            "EMBEDDINGS_API_AUTH_BEARER is not set — the functional /api routes are \
             UNAUTHENTICATED at this layer; rely on an upstream gateway or set the token"
        );
    }

    let state = AppState {
        registry,
        rag,
        api_auth_bearer: cfg.api_auth_bearer.clone().map(Arc::new),
        limits: cfg.limits,
    };

    // Public, unauthenticated surface: probes + API docs.
    let public = Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/api/docs.json", get(docs::docs_json))
        .route("/api/docs", get(docs_html))
        .route("/docs/api", get(docs_html));

    // Authenticated surface: the functional API. Gated by the bearer middleware
    // only when EMBEDDINGS_API_AUTH_BEARER is set.
    let api = Router::new()
        .route("/api/providers", get(list_providers))
        .route("/api/embeddings", post(embed))
        .route("/api/rag/index", post(rag_index))
        .route("/api/rag/search", post(rag_search))
        .layer(middleware::from_fn_with_state(state.clone(), auth));

    let app = public
        .merge(api)
        .with_state(state)
        .layer(TraceLayer::new_for_http())
        // No CORS layer: this is a service-to-service, bearer-authenticated API
        // reached through the gateway, not a browser-facing one. Add a tightly
        // scoped CorsLayer with explicit origins only if a browser caller ever
        // needs it.
        //
        // Cap request bodies; embedding batches are text, not blobs.
        .layer(tower_http::limit::RequestBodyLimitLayer::new(8 * 1024 * 1024))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(120),
        ))
        .layer(NormalizePathLayer::trim_trailing_slash());

    let listener = tokio::net::TcpListener::bind(cfg.addr).await?;
    tracing::info!(addr = %cfg.addr, "dd-embeddings-rs listening");
    axum::serve(listener, app).await?;
    Ok(())
}

fn init_tracing(json: bool) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,hyper=warn,reqwest=warn,tower_http=info"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    if json {
        builder.json().flatten_event(true).init();
    } else {
        builder.init();
    }
}

/// Bearer-token gate. No-op when no token is configured.
async fn auth(
    State(state): State<AppState>,
    req: axum::extract::Request,
    next: middleware::Next,
) -> axum::response::Response {
    let Some(expected) = state.api_auth_bearer.as_ref() else {
        return next.run(req).await;
    };
    let presented = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    match presented {
        Some(tok) if constant_time_eq(tok.as_bytes(), expected.as_bytes()) => next.run(req).await,
        _ => ApiError::Unauthorized.into_response(),
    }
}

async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

/// Readiness: confirms the vector store is reachable. Embedding providers are
/// external SaaS and intentionally not probed (they'd make readiness flap on
/// third-party hiccups).
async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    match state.rag.qdrant_health().await {
        Ok(()) => (StatusCode::OK, Json(json!({ "status": "ready" }))),
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "status": "degraded", "qdrant": e.to_string() })),
        ),
    }
}

async fn list_providers(State(state): State<AppState>) -> impl IntoResponse {
    let providers: Vec<_> = state
        .registry
        .iter()
        .map(|p| {
            json!({
                "id": p.id(),
                "default_model": p.default_model(),
                "models": p.known_models(),
            })
        })
        .collect();
    Json(json!({
        "count": providers.len(),
        "providers": providers,
        "aliases": state.registry.aliases(),
    }))
}

#[derive(Deserialize)]
struct EmbedApiRequest {
    /// Provider id or alias (`openai`, `voyage`, `anthropic`, `gemini`, ...).
    provider: String,
    #[serde(flatten)]
    req: EmbedRequest,
}

async fn embed(
    State(state): State<AppState>,
    Json(body): Json<EmbedApiRequest>,
) -> Result<impl IntoResponse, ApiError> {
    enforce_input_limits(&body.req.input, &state.limits)?;
    check_dimensions(body.req.dimensions, &state.limits)?;
    let provider = state.registry.resolve(&body.provider)?;
    let result = provider.embed(&body.req).await?;
    Ok(Json(result))
}

async fn rag_index(
    State(state): State<AppState>,
    Json(body): Json<IndexRequest>,
) -> Result<impl IntoResponse, ApiError> {
    validate_collection(&body.collection)?;
    validate_distance(&body.distance)?;
    check_dimensions(body.dimensions, &state.limits)?;
    // Validate the document texts under the same batch/size guardrails as the
    // raw embedding endpoint before we embed-and-upsert them.
    let texts: Vec<String> = body.documents.iter().map(|d| d.text.clone()).collect();
    enforce_input_limits(&texts, &state.limits)?;
    let result = state.rag.index(body).await?;
    Ok(Json(result))
}

async fn rag_search(
    State(state): State<AppState>,
    Json(mut body): Json<SearchRequest>,
) -> Result<impl IntoResponse, ApiError> {
    validate_collection(&body.collection)?;
    check_dimensions(body.dimensions, &state.limits)?;
    enforce_input_limits(std::slice::from_ref(&body.query), &state.limits)?;
    body.top_k = clamp_top_k(body.top_k, &state.limits);
    let result = state.rag.search(body).await?;
    Ok(Json(result))
}

async fn docs_html() -> impl IntoResponse {
    Html(docs::docs_html_string())
}
