//! dd-embeddings-rs — multi-provider embedding gateway + RAG indexing service.
//!
//! Boot: read env -> build a shared HTTP client -> register every provider
//! whose credentials are present -> wire Qdrant -> serve axum. The service is
//! deliberately tolerant of missing credentials: it will boot with zero
//! providers and report that on `/api/providers`, so you can add keys to the
//! backing secret incrementally without redeploying.

mod cache;
mod config;
mod db;
mod docs;
mod embedder;
mod error;
mod metrics;
mod providers;
mod rag;
mod search;
mod state;
mod validate;

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{middleware, Json, Router};
use serde::Deserialize;
use serde_json::json;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::normalize_path::NormalizePathLayer;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use crate::cache::EmbeddingCache;
use crate::config::Config;
use crate::embedder::Embedder;
use crate::error::ApiError;
use crate::metrics::Metrics;
use crate::providers::rerank::{RerankRegistry, RerankRequest};
use crate::providers::{EmbedRequest, Registry};
use crate::rag::qdrant::Qdrant;
use crate::rag::{DeletePointsRequest, IndexRequest, RagService, SearchRequest};
use crate::search::{
    AddEdgesRequest as SearchAddEdgesRequest, DeleteRequest as SearchDeleteRequest,
    IndexRequest as SearchIndexRequest, SearchRequest as SearchQueryRequest, SearchService,
};
use crate::state::AppState;
use crate::validate::{
    check_dimensions, clamp_top_k, constant_time_eq, enforce_input_limits, validate_collection,
    validate_distance, validate_model,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // .env is a local-dev convenience only. In release builds (what runs
    // in-cluster) we never read a dotfile — config + secrets come solely from
    // the real environment, so a stray committed `.env` can't shadow them.
    #[cfg(debug_assertions)]
    let _ = dotenvy::dotenv();
    let _otel = dd_telemetry::init("dd-embeddings-rs");
    let cfg = Config::from_env()?;

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(cfg.request_timeout_secs))
        // Fail fast on a dead/unreachable host instead of burning the full
        // request budget on connection setup.
        .connect_timeout(Duration::from_secs(10))
        .user_agent("dd-embeddings-rs/0.1")
        // Do not follow redirects: provider + Qdrant endpoints are fixed POST
        // targets that don't legitimately 3xx, and following a redirect could
        // be steered at an internal/metadata IP (SSRF).
        .redirect(reqwest::redirect::Policy::none())
        .build()?;

    let registry = Arc::new(Registry::from_config(&cfg, http.clone()));
    let rerank = Arc::new(RerankRegistry::from_config(&cfg, http.clone()));
    let metrics = Arc::new(Metrics::default());
    let cache = Arc::new(EmbeddingCache::new(cfg.cache_max_entries, cfg.cache_max_item_bytes));
    let embedder = Arc::new(Embedder::new(registry.clone(), cache.clone(), metrics.clone()));
    let qdrant = Arc::new(Qdrant::new(cfg.qdrant_url.clone(), cfg.qdrant_api_key.clone(), http.clone()));
    let rag = Arc::new(RagService::new(embedder.clone(), qdrant.clone()));

    // Optional Postgres search subsystem — only when DATABASE_URL is set.
    // Schema is dpm-managed (schema/schema.sql + scripts/dpm.sh), never
    // applied at boot.
    let search = if let Some(url) = &cfg.database_url {
        let pool = db::connect(url).await?;
        tracing::info!(search_dim = cfg.search_dim, "postgres search subsystem enabled");
        Some(Arc::new(SearchService::new(
            pool,
            embedder.clone(),
            rerank.clone(),
            cfg.search_dim,
            cfg.search_candidate_k,
            cfg.search_max_hops,
        )))
    } else {
        tracing::info!("postgres search subsystem disabled (no DATABASE_URL) — /api/search/* will 503");
        None
    };

    let provider_ids: Vec<&str> = registry.iter().map(|p| p.id()).collect();
    let rerank_ids: Vec<&str> = rerank.iter().map(|p| p.id()).collect();
    tracing::info!(
        providers = registry.len(),
        ids = ?provider_ids,
        aliases = ?registry.aliases(),
        rerank_providers = rerank.len(),
        rerank_ids = ?rerank_ids,
        "providers registered"
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
        embedder,
        rerank,
        rag,
        search,
        metrics,
        api_auth_bearer: cfg.api_auth_bearer.clone().map(Arc::new),
        limits: cfg.limits,
        // `.max(1)` guards against a `0` typo turning into a total outage.
        inflight: Arc::new(tokio::sync::Semaphore::new(cfg.max_concurrency.max(1))),
    };

    // Public, unauthenticated surface: probes, metrics (prometheus scrape) + docs.
    let public = Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics_endpoint))
        .route("/api/docs.json", get(docs::docs_json))
        .route("/api/docs", get(docs_html))
        .route("/docs/api", get(docs_html));

    // Authenticated surface: the functional API. Gated by the bearer middleware
    // only when EMBEDDINGS_API_AUTH_BEARER is set.
    let api = Router::new()
        .route("/api/providers", get(list_providers))
        .route("/api/embeddings", post(embed))
        .route("/api/rerank", post(rerank_handler))
        .route("/api/rag/index", post(rag_index))
        .route("/api/rag/search", post(rag_search))
        .route("/api/rag/delete", post(rag_delete))
        .route("/api/rag/collections", get(rag_list_collections))
        .route("/api/rag/collections/{collection}", delete(rag_delete_collection))
        // Postgres multi-signal search.
        .route("/api/search", post(search_query))
        .route("/api/search/index", post(search_index))
        .route("/api/search/edges", post(search_edges))
        .route("/api/search/delete", post(search_delete))
        .route("/api/search/collections", get(search_list_collections))
        .route("/api/search/collections/{collection}", delete(search_delete_collection))
        .layer(middleware::from_fn_with_state(state.clone(), auth));

    let app = public
        .merge(api)
        .with_state(state)
        // Outermost: turn any handler/layer panic into a clean 500 instead of
        // dropping the connection or leaking a backtrace.
        .layer(CatchPanicLayer::new())
        .layer(TraceLayer::new_for_http())
        // Defense-in-depth header on every response (the only HTML surface is
        // /api/docs; the rest is JSON).
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
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
    // Graceful shutdown: on SIGTERM (rolling deploy) / SIGINT, stop accepting
    // and let in-flight requests finish, so we don't abort already-paid
    // provider calls mid-flight. Bounded by terminationGracePeriodSeconds.
    axum::serve(listener, app.layer(dd_telemetry::http_trace_layer()))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(e) => tracing::error!(error = %e, "failed to install SIGTERM handler"),
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    tracing::info!("shutdown signal received — draining in-flight requests");
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
    // /readyz is unauthenticated — log internal detail, don't return it.
    if let Err(e) = state.rag.qdrant_health().await {
        tracing::warn!(error = %e, "readiness: qdrant unreachable");
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "status": "degraded" })));
    }
    // When the search subsystem is enabled, its database must be reachable too.
    if let Some(search) = &state.search {
        if let Err(e) = search.health().await {
            tracing::warn!(error = %e, "readiness: search database unreachable");
            return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "status": "degraded" })));
        }
    }
    (StatusCode::OK, Json(json!({ "status": "ready" })))
}

/// Acquire a slot from the global in-flight limiter, or shed with 503. Held
/// for the duration of a cost-bearing handler so a flood can't fan out
/// unbounded outbound calls.
fn acquire_slot(state: &AppState) -> Result<tokio::sync::OwnedSemaphorePermit, ApiError> {
    state
        .inflight
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::Overloaded)
}

/// Prometheus scrape endpoint (text exposition). Public, like the probes.
async fn metrics_endpoint(State(state): State<AppState>) -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        state.metrics.render(),
    )
}

/// Record an error against metrics if the result is an `Err`, then return it.
fn track<T>(state: &AppState, r: Result<T, ApiError>) -> Result<T, ApiError> {
    if r.is_err() {
        state.metrics.record_error();
    }
    r
}

async fn list_providers(State(state): State<AppState>) -> impl IntoResponse {
    state.metrics.record_request("providers");
    let embed: Vec<_> = state
        .registry
        .iter()
        .map(|p| json!({ "id": p.id(), "default_model": p.default_model(), "models": p.known_models() }))
        .collect();
    let rerank: Vec<_> = state
        .rerank
        .iter()
        .map(|p| json!({ "id": p.id(), "default_model": p.default_model(), "models": p.known_models() }))
        .collect();
    Json(json!({
        "embedding": { "count": embed.len(), "providers": embed, "aliases": state.registry.aliases() },
        "rerank": { "count": rerank.len(), "providers": rerank, "aliases": state.rerank.aliases() },
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
) -> Result<Response, ApiError> {
    state.metrics.record_request("embed");
    let out = async {
        enforce_input_limits(&body.req.input, &state.limits)?;
        check_dimensions(body.req.dimensions, &state.limits)?;
        validate_model(body.req.model.as_deref())?;
        let _permit = acquire_slot(&state)?;
        let result = state.embedder.embed(&body.provider, &body.req).await?;
        Ok(Json(result).into_response())
    }
    .await;
    track(&state, out)
}

#[derive(Deserialize)]
struct RerankApiRequest {
    /// Rerank provider id or alias (`cohere`, `jina`, `voyage`, `anthropic`).
    provider: String,
    #[serde(flatten)]
    req: RerankRequest,
}

async fn rerank_handler(
    State(state): State<AppState>,
    Json(body): Json<RerankApiRequest>,
) -> Result<Response, ApiError> {
    state.metrics.record_request("rerank");
    let out = async {
        let RerankApiRequest { provider, mut req } = body;
        enforce_input_limits(std::slice::from_ref(&req.query), &state.limits)?;
        enforce_input_limits(&req.documents, &state.limits)?;
        validate_model(req.model.as_deref())?;
        if let Some(n) = req.top_n {
            req.top_n = Some(clamp_top_k(n, &state.limits));
        }
        let _permit = acquire_slot(&state)?;
        let p = state.rerank.resolve(&provider)?;
        let result = p.rerank(&req).await?;
        state.metrics.record_provider(&format!("rerank:{}", result.provider));
        Ok(Json(result).into_response())
    }
    .await;
    track(&state, out)
}

async fn rag_index(
    State(state): State<AppState>,
    Json(body): Json<IndexRequest>,
) -> Result<Response, ApiError> {
    state.metrics.record_request("rag_index");
    let out = async {
        validate_collection(&body.collection)?;
        validate_distance(&body.distance)?;
        validate_model(body.model.as_deref())?;
        check_dimensions(body.dimensions, &state.limits)?;
        // Validate the document texts under the same batch/size guardrails as
        // the raw embedding endpoint before we embed-and-upsert them.
        let texts: Vec<String> = body.documents.iter().map(|d| d.text.clone()).collect();
        enforce_input_limits(&texts, &state.limits)?;
        let _permit = acquire_slot(&state)?;
        let result = state.rag.index(body).await?;
        Ok(Json(result).into_response())
    }
    .await;
    track(&state, out)
}

async fn rag_search(
    State(state): State<AppState>,
    Json(mut body): Json<SearchRequest>,
) -> Result<Response, ApiError> {
    state.metrics.record_request("rag_search");
    let out = async {
        validate_collection(&body.collection)?;
        validate_model(body.model.as_deref())?;
        check_dimensions(body.dimensions, &state.limits)?;
        enforce_input_limits(std::slice::from_ref(&body.query), &state.limits)?;
        body.top_k = clamp_top_k(body.top_k, &state.limits);
        let _permit = acquire_slot(&state)?;
        let result = state.rag.search(body).await?;
        Ok(Json(result).into_response())
    }
    .await;
    track(&state, out)
}

async fn rag_delete(
    State(state): State<AppState>,
    Json(body): Json<DeletePointsRequest>,
) -> Result<Response, ApiError> {
    state.metrics.record_request("rag_delete");
    let out = async {
        validate_collection(&body.collection)?;
        if body.ids.is_empty() {
            return Err(ApiError::Invalid("ids must be non-empty".into()));
        }
        if body.ids.len() > state.limits.max_batch_size {
            return Err(ApiError::Invalid(format!(
                "id count {} exceeds limit of {}",
                body.ids.len(),
                state.limits.max_batch_size
            )));
        }
        let result = state.rag.delete_points(body).await?;
        Ok(Json(result).into_response())
    }
    .await;
    track(&state, out)
}

async fn rag_list_collections(State(state): State<AppState>) -> Result<Response, ApiError> {
    state.metrics.record_request("rag_collections");
    let out = async {
        let collections = state.rag.list_collections().await?;
        Ok(Json(json!({ "collections": collections })).into_response())
    }
    .await;
    track(&state, out)
}

async fn rag_delete_collection(
    State(state): State<AppState>,
    Path(collection): Path<String>,
) -> Result<Response, ApiError> {
    state.metrics.record_request("rag_delete_collection");
    let out = async {
        validate_collection(&collection)?;
        state.rag.delete_collection(&collection).await?;
        Ok(Json(json!({ "collection": collection, "deleted": true })).into_response())
    }
    .await;
    track(&state, out)
}

/// Fetch the search service or fail with 503 if no DB is configured.
fn search_svc(state: &AppState) -> Result<&Arc<SearchService>, ApiError> {
    state.search.as_ref().ok_or(ApiError::SearchDisabled)
}

async fn search_index(
    State(state): State<AppState>,
    Json(body): Json<SearchIndexRequest>,
) -> Result<Response, ApiError> {
    state.metrics.record_request("search_index");
    let out = async {
        validate_collection(&body.collection)?;
        validate_model(body.model.as_deref())?;
        let texts: Vec<String> = body.documents.iter().map(|d| d.content.clone()).collect();
        enforce_input_limits(&texts, &state.limits)?;
        let svc = search_svc(&state)?;
        let _permit = acquire_slot(&state)?;
        Ok(Json(svc.index(body).await?).into_response())
    }
    .await;
    track(&state, out)
}

async fn search_query(
    State(state): State<AppState>,
    Json(mut body): Json<SearchQueryRequest>,
) -> Result<Response, ApiError> {
    state.metrics.record_request("search_query");
    let out = async {
        validate_collection(&body.collection)?;
        validate_model(body.model.as_deref())?;
        enforce_input_limits(std::slice::from_ref(&body.query), &state.limits)?;
        body.top_k = clamp_top_k(body.top_k, &state.limits);
        if let Some(rc) = &body.rerank {
            validate_model(rc.model.as_deref())?;
        }
        let svc = search_svc(&state)?;
        let _permit = acquire_slot(&state)?;
        Ok(Json(svc.query(body).await?).into_response())
    }
    .await;
    track(&state, out)
}

async fn search_edges(
    State(state): State<AppState>,
    Json(body): Json<SearchAddEdgesRequest>,
) -> Result<Response, ApiError> {
    state.metrics.record_request("search_edges");
    let out = async {
        validate_collection(&body.collection)?;
        let svc = search_svc(&state)?;
        let added = svc.add_edges(body).await?;
        Ok(Json(json!({ "added": added })).into_response())
    }
    .await;
    track(&state, out)
}

async fn search_delete(
    State(state): State<AppState>,
    Json(body): Json<SearchDeleteRequest>,
) -> Result<Response, ApiError> {
    state.metrics.record_request("search_delete");
    let out = async {
        validate_collection(&body.collection)?;
        let svc = search_svc(&state)?;
        let deleted = svc.delete(body).await?;
        Ok(Json(json!({ "deleted": deleted })).into_response())
    }
    .await;
    track(&state, out)
}

async fn search_list_collections(State(state): State<AppState>) -> Result<Response, ApiError> {
    state.metrics.record_request("search_collections");
    let out = async {
        let svc = search_svc(&state)?;
        Ok(Json(json!({ "collections": svc.list_collections().await? })).into_response())
    }
    .await;
    track(&state, out)
}

async fn search_delete_collection(
    State(state): State<AppState>,
    Path(collection): Path<String>,
) -> Result<Response, ApiError> {
    state.metrics.record_request("search_delete_collection");
    let out = async {
        validate_collection(&collection)?;
        let svc = search_svc(&state)?;
        let deleted = svc.delete_collection(&collection).await?;
        Ok(Json(json!({ "collection": collection, "deleted": deleted })).into_response())
    }
    .await;
    track(&state, out)
}

async fn docs_html() -> impl IntoResponse {
    Html(docs::docs_html_string())
}
