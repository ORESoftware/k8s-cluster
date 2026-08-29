//! dd-embeddings-rs — multi-provider embedding gateway + RAG indexing service.
//!
//! The live Axum router and OpenAPI document are composed from the same
//! `utoipa_axum::routes!` registrations. `--export-openapi` performs no runtime
//! configuration, network, database, or telemetry initialization.

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

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{middleware, Extension, Json};
use serde::{Deserialize, Serialize};
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::normalize_path::NormalizePathLayer;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;
use utoipa::openapi::OpenApi;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::cache::EmbeddingCache;
use crate::config::Config;
use crate::docs::{ApiDocs, SharedApiDocs};
use crate::embedder::Embedder;
use crate::error::{ApiError, ApiErrorResponses};
use crate::metrics::Metrics;
use crate::providers::rerank::{RerankRegistry, RerankRequest, RerankResponse};
use crate::providers::{EmbedRequest, EmbedResponse, Registry};
use crate::rag::qdrant::Qdrant;
use crate::rag::{
    DeletePointsRequest, DeletePointsResponse, IndexRequest as RagIndexRequest,
    IndexResponse as RagIndexResponse, RagService, SearchRequest as RagSearchRequest,
    SearchResponse as RagSearchResponse,
};
use crate::search::{
    AddEdgesRequest as SearchAddEdgesRequest, DeleteRequest as SearchDeleteRequest,
    IndexRequest as SearchIndexRequest, IndexResponse as SearchIndexResponse,
    SearchRequest as SearchQueryRequest, SearchResponse as SearchQueryResponse, SearchService,
};
use crate::state::AppState;
use crate::validate::{
    check_dimensions, clamp_top_k, constant_time_eq, enforce_input_limits, validate_collection,
    validate_distance, validate_model,
};

const OPENAPI_EXPORT_FLAG: &str = "--export-openapi";
const OPENAPI_CONTENT_TYPE: &str = "application/vnd.oai.openapi+json;version=3.1";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::args().any(|arg| arg == OPENAPI_EXPORT_FLAG) {
        print!("{}", docs::canonical_json(&openapi_document())?);
        return Ok(());
    }

    #[cfg(debug_assertions)]
    let _ = dotenvy::dotenv();
    let _otel = dd_telemetry::init("dd-embeddings-rs");
    let cfg = Config::from_env()?;

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(cfg.request_timeout_secs))
        .connect_timeout(Duration::from_secs(10))
        .user_agent("dd-embeddings-rs/0.1")
        .redirect(reqwest::redirect::Policy::none())
        .build()?;

    let registry = Arc::new(Registry::from_config(&cfg, http.clone()));
    let rerank = Arc::new(RerankRegistry::from_config(&cfg, http.clone()));
    let metrics = Arc::new(Metrics::default());
    let cache = Arc::new(EmbeddingCache::new(
        cfg.cache_max_entries,
        cfg.cache_max_item_bytes,
    ));
    let embedder = Arc::new(Embedder::new(
        registry.clone(),
        cache.clone(),
        metrics.clone(),
    ));
    let qdrant = Arc::new(Qdrant::new(
        cfg.qdrant_url.clone(),
        cfg.qdrant_api_key.clone(),
        http.clone(),
    ));
    let rag = Arc::new(RagService::new(embedder.clone(), qdrant.clone()));

    let search = if let Some(url) = &cfg.database_url {
        let pool = db::connect(url).await?;
        tracing::info!(
            search_dim = cfg.search_dim,
            "postgres search subsystem enabled"
        );
        Some(Arc::new(SearchService::new(
            pool,
            embedder.clone(),
            rerank.clone(),
            cfg.search_dim,
            cfg.search_candidate_k,
            cfg.search_max_hops,
        )))
    } else {
        tracing::info!(
            "postgres search subsystem disabled (no DATABASE_URL) — /api/search/* will 503"
        );
        None
    };

    let provider_ids: Vec<&str> = registry.iter().map(|provider| provider.id()).collect();
    let rerank_ids: Vec<&str> = rerank.iter().map(|provider| provider.id()).collect();
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
            "no embedding providers configured — set provider API keys in the dd-embeddings-rs secret to enable them"
        );
    }
    if cfg.api_auth_bearer.is_none() {
        tracing::warn!(
            "EMBEDDINGS_API_AUTH_BEARER is not set — protected API and internal documentation routes will fail closed"
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
        inflight: Arc::new(tokio::sync::Semaphore::new(cfg.max_concurrency.max(1))),
    };

    let openapi = openapi_document();
    let api_docs = Arc::new(ApiDocs::new(&openapi)?);

    let public = public_router();
    let protected =
        protected_router().route_layer(middleware::from_fn_with_state(state.clone(), auth));
    let (router, runtime_openapi) = public.merge(protected).split_for_parts();
    debug_assert_eq!(
        docs::canonical_json(&openapi)?,
        docs::canonical_json(&docs::finalize(runtime_openapi))?,
        "runtime router and exported OpenAPI contract diverged"
    );

    let app = router
        .with_state(state)
        .layer(Extension(api_docs))
        .layer(CatchPanicLayer::new())
        .layer(TraceLayer::new_for_http())
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(tower_http::limit::RequestBodyLimitLayer::new(
            8 * 1024 * 1024,
        ))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(120),
        ))
        .layer(NormalizePathLayer::trim_trailing_slash());

    let listener = tokio::net::TcpListener::bind(cfg.addr).await?;
    tracing::info!(addr = %cfg.addr, "dd-embeddings-rs listening");
    axum::serve(listener, app.layer(dd_telemetry::http_trace_layer()))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

fn public_router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(healthz))
        .routes(routes!(readyz))
        .routes(routes!(metrics_endpoint))
        .routes(routes!(openapi_json))
        .routes(routes!(api_docs_json))
        .routes(routes!(api_docs_ui))
        .routes(routes!(docs_api_ui))
}

fn protected_router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(internal_openapi_json))
        .routes(routes!(internal_api_docs_ui))
        .routes(routes!(list_providers))
        .routes(routes!(embed))
        .routes(routes!(rerank_handler))
        .routes(routes!(rag_index))
        .routes(routes!(rag_search))
        .routes(routes!(rag_delete))
        .routes(routes!(rag_list_collections))
        .routes(routes!(rag_delete_collection))
        .routes(routes!(search_query))
        .routes(routes!(search_index))
        .routes(routes!(search_edges))
        .routes(routes!(search_delete))
        .routes(routes!(search_list_collections))
        .routes(routes!(search_delete_collection))
}

fn openapi_document() -> OpenApi {
    docs::finalize(public_router().merge(protected_router()).into_openapi())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => tracing::error!(error = %error, "failed to install SIGTERM handler"),
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

async fn auth(
    State(state): State<AppState>,
    request: axum::extract::Request,
    next: middleware::Next,
) -> Response {
    let Some(expected) = state.api_auth_bearer.as_ref() else {
        return ApiError::Unauthorized.into_response();
    };
    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    match presented {
        Some(token) if constant_time_eq(token.as_bytes(), expected.as_bytes()) => {
            next.run(request).await
        }
        _ => ApiError::Unauthorized.into_response(),
    }
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
struct StatusResponse {
    status: String,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
struct ProviderDescriptor {
    id: String,
    default_model: String,
    models: Vec<String>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
struct ProviderGroup {
    count: usize,
    providers: Vec<ProviderDescriptor>,
    aliases: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
struct ProvidersResponse {
    embedding: ProviderGroup,
    rerank: ProviderGroup,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
struct CollectionsResponse {
    collections: Vec<String>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
struct CollectionDeletionResponse {
    collection: String,
    deleted: bool,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
struct SearchCollectionDeletionResponse {
    collection: String,
    deleted: u64,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
struct AddedEdgesResponse {
    added: usize,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
struct DeletedDocumentsResponse {
    deleted: u64,
}

#[utoipa::path(
    get,
    path = "/healthz",
    operation_id = "getHealth",
    tag = "operations",
    security(()),
    responses((status = 200, description = "Process is alive", body = String, content_type = "text/plain"))
)]
async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

#[utoipa::path(
    get,
    path = "/readyz",
    operation_id = "getReadiness",
    tag = "operations",
    security(()),
    responses(
        (status = 200, description = "Qdrant and the optional search database are reachable", body = StatusResponse),
        (status = 503, description = "A required dependency is unavailable", body = StatusResponse)
    )
)]
async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    if let Err(error) = state.rag.qdrant_health().await {
        tracing::warn!(error = %error, "readiness: qdrant unreachable");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(StatusResponse {
                status: "degraded".to_string(),
            }),
        );
    }
    if let Some(search) = &state.search {
        if let Err(error) = search.health().await {
            tracing::warn!(error = %error, "readiness: search database unreachable");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(StatusResponse {
                    status: "degraded".to_string(),
                }),
            );
        }
    }
    (
        StatusCode::OK,
        Json(StatusResponse {
            status: "ready".to_string(),
        }),
    )
}

#[utoipa::path(
    get,
    path = "/metrics",
    operation_id = "getPrometheusMetrics",
    tag = "operations",
    security(()),
    responses((status = 200, description = "Prometheus text exposition", body = String, content_type = "text/plain"))
)]
async fn metrics_endpoint(State(state): State<AppState>) -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        state.metrics.render(),
    )
}

#[utoipa::path(
    get,
    path = "/openapi.json",
    operation_id = "getPublicOpenApiDocument",
    tag = "documentation",
    security(()),
    responses((status = 200, description = "Fail-closed public OpenAPI 3.1 contract", content_type = "application/vnd.oai.openapi+json;version=3.1"))
)]
async fn openapi_json(Extension(docs): Extension<SharedApiDocs>) -> Response {
    public_openapi_response(docs)
}

#[utoipa::path(
    get,
    path = "/api/docs.json",
    operation_id = "getPublicOpenApiDocumentCompatibilityAlias",
    tag = "documentation",
    security(()),
    responses((status = 200, description = "Compatibility alias for the fail-closed public OpenAPI 3.1 contract", content_type = "application/vnd.oai.openapi+json;version=3.1"))
)]
async fn api_docs_json(Extension(docs): Extension<SharedApiDocs>) -> Response {
    public_openapi_response(docs)
}

#[utoipa::path(
    get,
    path = "/api/docs",
    operation_id = "getPublicApiReference",
    tag = "documentation",
    security(()),
    responses((status = 200, description = "Interactive Scalar reference for the fail-closed public contract", body = String, content_type = "text/html"))
)]
async fn api_docs_ui(Extension(docs): Extension<SharedApiDocs>) -> Response {
    public_scalar_response(docs)
}

#[utoipa::path(
    get,
    path = "/docs/api",
    operation_id = "getPublicApiReferenceCompatibilityAlias",
    tag = "documentation",
    security(()),
    responses((status = 200, description = "Compatibility alias for the public Scalar API reference", body = String, content_type = "text/html"))
)]
async fn docs_api_ui(Extension(docs): Extension<SharedApiDocs>) -> Response {
    public_scalar_response(docs)
}

#[utoipa::path(
    get,
    path = "/internal/openapi.json",
    operation_id = "getInternalOpenApiDocument",
    tag = "documentation",
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Complete typed OpenAPI 3.1 contract for trusted service-to-service callers", content_type = "application/vnd.oai.openapi+json;version=3.1"),
        ApiErrorResponses
    )
)]
async fn internal_openapi_json(Extension(docs): Extension<SharedApiDocs>) -> Response {
    internal_openapi_response(docs)
}

#[utoipa::path(
    get,
    path = "/internal/docs/api",
    operation_id = "getInternalApiReference",
    tag = "documentation",
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Interactive Scalar reference for the complete internal contract", body = String, content_type = "text/html"),
        ApiErrorResponses
    )
)]
async fn internal_api_docs_ui(Extension(docs): Extension<SharedApiDocs>) -> Response {
    internal_scalar_response(docs)
}

fn public_openapi_response(docs: SharedApiDocs) -> Response {
    (
        [(header::CONTENT_TYPE, OPENAPI_CONTENT_TYPE)],
        docs.public_json.clone(),
    )
        .into_response()
}

fn internal_openapi_response(docs: SharedApiDocs) -> Response {
    (
        [(header::CONTENT_TYPE, OPENAPI_CONTENT_TYPE)],
        docs.internal_json.clone(),
    )
        .into_response()
}

fn public_scalar_response(docs: SharedApiDocs) -> Response {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        docs.public_scalar_html.clone(),
    )
        .into_response()
}

fn internal_scalar_response(docs: SharedApiDocs) -> Response {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        docs.internal_scalar_html.clone(),
    )
        .into_response()
}

fn acquire_slot(state: &AppState) -> Result<tokio::sync::OwnedSemaphorePermit, ApiError> {
    state
        .inflight
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::Overloaded)
}

fn track<T>(state: &AppState, result: Result<T, ApiError>) -> Result<T, ApiError> {
    if result.is_err() {
        state.metrics.record_error();
    }
    result
}

#[utoipa::path(
    get,
    path = "/api/providers",
    operation_id = "listProviders",
    tag = "providers",
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Configured embedding and rerank providers", body = ProvidersResponse),
        ApiErrorResponses
    )
)]
async fn list_providers(State(state): State<AppState>) -> Json<ProvidersResponse> {
    state.metrics.record_request("providers");
    let embedding = state
        .registry
        .iter()
        .map(|provider| ProviderDescriptor {
            id: provider.id().to_string(),
            default_model: provider.default_model().to_string(),
            models: provider
                .known_models()
                .iter()
                .map(|model| (*model).to_string())
                .collect(),
        })
        .collect::<Vec<_>>();
    let rerank = state
        .rerank
        .iter()
        .map(|provider| ProviderDescriptor {
            id: provider.id().to_string(),
            default_model: provider.default_model().to_string(),
            models: provider
                .known_models()
                .iter()
                .map(|model| (*model).to_string())
                .collect(),
        })
        .collect::<Vec<_>>();

    Json(ProvidersResponse {
        embedding: ProviderGroup {
            count: embedding.len(),
            providers: embedding,
            aliases: state.registry.aliases().clone(),
        },
        rerank: ProviderGroup {
            count: rerank.len(),
            providers: rerank,
            aliases: state.rerank.aliases().clone(),
        },
    })
}

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
struct EmbedApiRequest {
    /// Provider id or alias (`openai`, `voyage`, `anthropic`, `gemini`, ...).
    provider: String,
    #[serde(flatten)]
    req: EmbedRequest,
}

#[utoipa::path(
    post,
    path = "/api/embeddings",
    operation_id = "createEmbeddings",
    tag = "embeddings",
    security(("bearer_auth" = [])),
    request_body = EmbedApiRequest,
    responses(
        (status = 200, description = "Normalized embeddings from the selected provider", body = EmbedResponse),
        ApiErrorResponses
    )
)]
async fn embed(
    State(state): State<AppState>,
    Json(body): Json<EmbedApiRequest>,
) -> Result<Json<EmbedResponse>, ApiError> {
    state.metrics.record_request("embed");
    let result = async {
        enforce_input_limits(&body.req.input, &state.limits)?;
        check_dimensions(body.req.dimensions, &state.limits)?;
        validate_model(body.req.model.as_deref())?;
        let _permit = acquire_slot(&state)?;
        let response = state.embedder.embed(&body.provider, &body.req).await?;
        Ok(Json(response))
    }
    .await;
    track(&state, result)
}

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
struct RerankApiRequest {
    /// Rerank provider id or alias (`cohere`, `jina`, `voyage`, `anthropic`).
    provider: String,
    #[serde(flatten)]
    req: RerankRequest,
}

#[utoipa::path(
    post,
    path = "/api/rerank",
    operation_id = "rerankDocuments",
    tag = "reranking",
    security(("bearer_auth" = [])),
    request_body = RerankApiRequest,
    responses(
        (status = 200, description = "Documents ordered by relevance", body = RerankResponse),
        ApiErrorResponses
    )
)]
async fn rerank_handler(
    State(state): State<AppState>,
    Json(body): Json<RerankApiRequest>,
) -> Result<Json<RerankResponse>, ApiError> {
    state.metrics.record_request("rerank");
    let result = async {
        let RerankApiRequest { provider, mut req } = body;
        enforce_input_limits(std::slice::from_ref(&req.query), &state.limits)?;
        enforce_input_limits(&req.documents, &state.limits)?;
        validate_model(req.model.as_deref())?;
        if let Some(top_n) = req.top_n {
            req.top_n = Some(clamp_top_k(top_n, &state.limits));
        }
        let _permit = acquire_slot(&state)?;
        let selected = state.rerank.resolve(&provider)?;
        let response = selected.rerank(&req).await?;
        state
            .metrics
            .record_provider(&format!("rerank:{}", response.provider));
        Ok(Json(response))
    }
    .await;
    track(&state, result)
}

#[utoipa::path(
    post,
    path = "/api/rag/index",
    operation_id = "indexRagDocuments",
    tag = "rag",
    security(("bearer_auth" = [])),
    request_body = RagIndexRequest,
    responses(
        (status = 200, description = "Documents embedded and upserted", body = RagIndexResponse),
        ApiErrorResponses
    )
)]
async fn rag_index(
    State(state): State<AppState>,
    Json(body): Json<RagIndexRequest>,
) -> Result<Json<RagIndexResponse>, ApiError> {
    state.metrics.record_request("rag_index");
    let result = async {
        validate_collection(&body.collection)?;
        validate_distance(&body.distance)?;
        validate_model(body.model.as_deref())?;
        check_dimensions(body.dimensions, &state.limits)?;
        let texts: Vec<String> = body
            .documents
            .iter()
            .map(|document| document.text.clone())
            .collect();
        enforce_input_limits(&texts, &state.limits)?;
        let _permit = acquire_slot(&state)?;
        Ok(Json(state.rag.index(body).await?))
    }
    .await;
    track(&state, result)
}

#[utoipa::path(
    post,
    path = "/api/rag/search",
    operation_id = "searchRagCollection",
    tag = "rag",
    security(("bearer_auth" = [])),
    request_body = RagSearchRequest,
    responses(
        (status = 200, description = "Nearest vector matches", body = RagSearchResponse),
        ApiErrorResponses
    )
)]
async fn rag_search(
    State(state): State<AppState>,
    Json(mut body): Json<RagSearchRequest>,
) -> Result<Json<RagSearchResponse>, ApiError> {
    state.metrics.record_request("rag_search");
    let result = async {
        validate_collection(&body.collection)?;
        validate_model(body.model.as_deref())?;
        check_dimensions(body.dimensions, &state.limits)?;
        enforce_input_limits(std::slice::from_ref(&body.query), &state.limits)?;
        body.top_k = clamp_top_k(body.top_k, &state.limits);
        let _permit = acquire_slot(&state)?;
        Ok(Json(state.rag.search(body).await?))
    }
    .await;
    track(&state, result)
}

#[utoipa::path(
    post,
    path = "/api/rag/delete",
    operation_id = "deleteRagPoints",
    tag = "rag",
    security(("bearer_auth" = [])),
    request_body = DeletePointsRequest,
    responses(
        (status = 200, description = "Points deleted", body = DeletePointsResponse),
        ApiErrorResponses
    )
)]
async fn rag_delete(
    State(state): State<AppState>,
    Json(body): Json<DeletePointsRequest>,
) -> Result<Json<DeletePointsResponse>, ApiError> {
    state.metrics.record_request("rag_delete");
    let result = async {
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
        Ok(Json(state.rag.delete_points(body).await?))
    }
    .await;
    track(&state, result)
}

#[utoipa::path(
    get,
    path = "/api/rag/collections",
    operation_id = "listRagCollections",
    tag = "rag",
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Vector-store collection names", body = CollectionsResponse),
        ApiErrorResponses
    )
)]
async fn rag_list_collections(
    State(state): State<AppState>,
) -> Result<Json<CollectionsResponse>, ApiError> {
    state.metrics.record_request("rag_collections");
    let result = async {
        Ok(Json(CollectionsResponse {
            collections: state.rag.list_collections().await?,
        }))
    }
    .await;
    track(&state, result)
}

#[utoipa::path(
    delete,
    path = "/api/rag/collections/{collection}",
    operation_id = "deleteRagCollection",
    tag = "rag",
    security(("bearer_auth" = [])),
    params(("collection" = String, Path, description = "Validated Qdrant collection name")),
    responses(
        (status = 200, description = "Collection deletion completed idempotently", body = CollectionDeletionResponse),
        ApiErrorResponses
    )
)]
async fn rag_delete_collection(
    State(state): State<AppState>,
    Path(collection): Path<String>,
) -> Result<Json<CollectionDeletionResponse>, ApiError> {
    state.metrics.record_request("rag_delete_collection");
    let result = async {
        validate_collection(&collection)?;
        state.rag.delete_collection(&collection).await?;
        Ok(Json(CollectionDeletionResponse {
            collection,
            deleted: true,
        }))
    }
    .await;
    track(&state, result)
}

fn search_svc(state: &AppState) -> Result<&Arc<SearchService>, ApiError> {
    state.search.as_ref().ok_or(ApiError::SearchDisabled)
}

#[utoipa::path(
    post,
    path = "/api/search/index",
    operation_id = "indexSearchDocuments",
    tag = "search",
    security(("bearer_auth" = [])),
    request_body = SearchIndexRequest,
    responses(
        (status = 200, description = "Documents and graph edges indexed", body = SearchIndexResponse),
        ApiErrorResponses
    )
)]
async fn search_index(
    State(state): State<AppState>,
    Json(body): Json<SearchIndexRequest>,
) -> Result<Json<SearchIndexResponse>, ApiError> {
    state.metrics.record_request("search_index");
    let result = async {
        validate_collection(&body.collection)?;
        validate_model(body.model.as_deref())?;
        let texts: Vec<String> = body
            .documents
            .iter()
            .map(|document| document.content.clone())
            .collect();
        enforce_input_limits(&texts, &state.limits)?;
        let service = search_svc(&state)?;
        let _permit = acquire_slot(&state)?;
        Ok(Json(service.index(body).await?))
    }
    .await;
    track(&state, result)
}

#[utoipa::path(
    post,
    path = "/api/search",
    operation_id = "searchDocuments",
    tag = "search",
    security(("bearer_auth" = [])),
    request_body = SearchQueryRequest,
    responses(
        (status = 200, description = "Fused and optionally reranked search hits", body = SearchQueryResponse),
        ApiErrorResponses
    )
)]
async fn search_query(
    State(state): State<AppState>,
    Json(mut body): Json<SearchQueryRequest>,
) -> Result<Json<SearchQueryResponse>, ApiError> {
    state.metrics.record_request("search_query");
    let result = async {
        validate_collection(&body.collection)?;
        validate_model(body.model.as_deref())?;
        enforce_input_limits(std::slice::from_ref(&body.query), &state.limits)?;
        body.top_k = clamp_top_k(body.top_k, &state.limits);
        if let Some(rerank) = &body.rerank {
            validate_model(rerank.model.as_deref())?;
        }
        let service = search_svc(&state)?;
        let _permit = acquire_slot(&state)?;
        Ok(Json(service.query(body).await?))
    }
    .await;
    track(&state, result)
}

#[utoipa::path(
    post,
    path = "/api/search/edges",
    operation_id = "addSearchEdges",
    tag = "search",
    security(("bearer_auth" = [])),
    request_body = SearchAddEdgesRequest,
    responses(
        (status = 200, description = "Graph edges added", body = AddedEdgesResponse),
        ApiErrorResponses
    )
)]
async fn search_edges(
    State(state): State<AppState>,
    Json(body): Json<SearchAddEdgesRequest>,
) -> Result<Json<AddedEdgesResponse>, ApiError> {
    state.metrics.record_request("search_edges");
    let result = async {
        validate_collection(&body.collection)?;
        let service = search_svc(&state)?;
        Ok(Json(AddedEdgesResponse {
            added: service.add_edges(body).await?,
        }))
    }
    .await;
    track(&state, result)
}

#[utoipa::path(
    post,
    path = "/api/search/delete",
    operation_id = "deleteSearchDocuments",
    tag = "search",
    security(("bearer_auth" = [])),
    request_body = SearchDeleteRequest,
    responses(
        (status = 200, description = "Documents deleted", body = DeletedDocumentsResponse),
        ApiErrorResponses
    )
)]
async fn search_delete(
    State(state): State<AppState>,
    Json(body): Json<SearchDeleteRequest>,
) -> Result<Json<DeletedDocumentsResponse>, ApiError> {
    state.metrics.record_request("search_delete");
    let result = async {
        validate_collection(&body.collection)?;
        let service = search_svc(&state)?;
        Ok(Json(DeletedDocumentsResponse {
            deleted: service.delete(body).await?,
        }))
    }
    .await;
    track(&state, result)
}

#[utoipa::path(
    get,
    path = "/api/search/collections",
    operation_id = "listSearchCollections",
    tag = "search",
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Postgres search collection names", body = CollectionsResponse),
        ApiErrorResponses
    )
)]
async fn search_list_collections(
    State(state): State<AppState>,
) -> Result<Json<CollectionsResponse>, ApiError> {
    state.metrics.record_request("search_collections");
    let result = async {
        let service = search_svc(&state)?;
        Ok(Json(CollectionsResponse {
            collections: service.list_collections().await?,
        }))
    }
    .await;
    track(&state, result)
}

#[utoipa::path(
    delete,
    path = "/api/search/collections/{collection}",
    operation_id = "deleteSearchCollection",
    tag = "search",
    security(("bearer_auth" = [])),
    params(("collection" = String, Path, description = "Validated search collection name")),
    responses(
        (status = 200, description = "Search collection deletion result", body = SearchCollectionDeletionResponse),
        ApiErrorResponses
    )
)]
async fn search_delete_collection(
    State(state): State<AppState>,
    Path(collection): Path<String>,
) -> Result<Json<SearchCollectionDeletionResponse>, ApiError> {
    state.metrics.record_request("search_delete_collection");
    let result = async {
        validate_collection(&collection)?;
        let service = search_svc(&state)?;
        let deleted = service.delete_collection(&collection).await?;
        Ok(Json(SearchCollectionDeletionResponse {
            collection,
            deleted,
        }))
    }
    .await;
    track(&state, result)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Arc;

    use axum::body::to_bytes;
    use axum::http::StatusCode;
    use axum::Extension;
    use serde_json::Value;

    use super::*;

    const EXPECTED_PATHS: &[&str] = &[
        "/api/docs",
        "/api/docs.json",
        "/api/embeddings",
        "/api/providers",
        "/api/rag/collections",
        "/api/rag/collections/{collection}",
        "/api/rag/delete",
        "/api/rag/index",
        "/api/rag/search",
        "/api/rerank",
        "/api/search",
        "/api/search/collections",
        "/api/search/collections/{collection}",
        "/api/search/delete",
        "/api/search/edges",
        "/api/search/index",
        "/docs/api",
        "/healthz",
        "/metrics",
        "/internal/docs/api",
        "/internal/openapi.json",
        "/openapi.json",
        "/readyz",
    ];

    #[test]
    fn openapi_is_deterministic_complete_and_sdk_safe() {
        let first = docs::canonical_json(&openapi_document()).expect("serialize first contract");
        let second = docs::canonical_json(&openapi_document()).expect("serialize second contract");
        assert_eq!(
            first, second,
            "OpenAPI generation must be byte deterministic"
        );

        let document: Value = serde_json::from_str(&first).expect("parse generated OpenAPI");
        assert_eq!(document["openapi"], "3.1.0");
        assert_eq!(document["info"]["title"], "dd-embeddings-rs API");
        assert!(document["components"]["securitySchemes"]["bearer_auth"].is_object());
        assert!(document["components"]["schemas"]["ErrorResponse"].is_object());

        let paths = document["paths"].as_object().expect("paths object");
        let actual = paths.keys().map(String::as_str).collect::<BTreeSet<_>>();
        let expected = EXPECTED_PATHS.iter().copied().collect::<BTreeSet<_>>();
        assert_eq!(actual, expected, "router/spec path parity changed");

        let public = BTreeSet::from([
            "/api/docs",
            "/api/docs.json",
            "/docs/api",
            "/healthz",
            "/metrics",
            "/openapi.json",
            "/readyz",
        ]);
        let mut operation_ids = BTreeSet::new();
        let mut operations_by_path = BTreeMap::new();
        for (path, item) in paths {
            let item = item.as_object().expect("path item object");
            for (method, operation) in item {
                if ![
                    "get", "post", "put", "patch", "delete", "head", "options", "trace",
                ]
                .contains(&method.as_str())
                {
                    continue;
                }
                let operation_id = operation["operationId"]
                    .as_str()
                    .unwrap_or_else(|| panic!("{method} {path} has no operationId"));
                assert!(
                    operation_ids.insert(operation_id.to_string()),
                    "duplicate operationId {operation_id}"
                );
                assert!(
                    operation["responses"]
                        .as_object()
                        .is_some_and(|responses| !responses.is_empty()),
                    "{method} {path} has no responses"
                );
                if public.contains(path.as_str()) {
                    let security = operation.get("security");
                    assert!(
                        security.is_none()
                            || security.is_some_and(|value| {
                                value.as_array().is_some_and(|items| {
                                    items.is_empty()
                                        || items.iter().any(|item| {
                                            item.as_object().is_some_and(|object| object.is_empty())
                                        })
                                })
                            }),
                        "public operation {method} {path} unexpectedly requires auth"
                    );
                } else {
                    assert!(
                        operation["security"]
                            .as_array()
                            .is_some_and(|requirements| !requirements.is_empty()),
                        "functional operation {method} {path} must declare bearer security"
                    );
                }
                if ["post", "put", "patch"].contains(&method.as_str()) {
                    assert!(
                        operation["requestBody"].is_object(),
                        "{method} {path} has no typed request body"
                    );
                }
                operations_by_path.insert((path.clone(), method.clone()), operation_id.to_string());
            }
        }
        assert_eq!(operation_ids.len(), 23);
        assert_eq!(operations_by_path.len(), 23);
    }

    #[tokio::test]
    async fn runtime_docs_separate_public_and_internal_contracts_exactly() {
        let openapi = openapi_document();
        let canonical = docs::canonical_json(&openapi).expect("canonical internal JSON");
        let public = docs::public_json();
        assert_ne!(
            public, canonical,
            "public and internal contracts must differ"
        );

        let public_document: Value = serde_json::from_str(public).expect("parse public OpenAPI");
        assert_eq!(public_document["x-dd-contract-scope"], "public");
        assert!(public_document["paths"]["/api/embeddings"].is_null());
        assert!(public_document["components"]["schemas"]["EmbedApiRequest"].is_null());

        let shared = Arc::new(ApiDocs::new(&openapi).expect("runtime docs"));

        for response in [
            openapi_json(Extension(shared.clone())).await,
            api_docs_json(Extension(shared.clone())).await,
        ] {
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response.headers()[header::CONTENT_TYPE],
                OPENAPI_CONTENT_TYPE
            );
            let body = to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("read public OpenAPI body");
            assert_eq!(body.as_ref(), public.as_bytes());
        }

        let response = internal_openapi_json(Extension(shared.clone())).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            OPENAPI_CONTENT_TYPE
        );
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read internal OpenAPI body");
        assert_eq!(body.as_ref(), canonical.as_bytes());

        for response in [
            api_docs_ui(Extension(shared.clone())).await,
            docs_api_ui(Extension(shared.clone())).await,
        ] {
            assert_eq!(response.status(), StatusCode::OK);
            let body = to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("read public Scalar body");
            let html = String::from_utf8(body.to_vec()).expect("UTF-8 Scalar HTML");
            assert!(html.to_ascii_lowercase().contains("scalar"));
            assert!(html.contains("dd-embeddings-rs API (public)"));
        }

        let response = internal_api_docs_ui(Extension(shared)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read internal Scalar body");
        let html = String::from_utf8(body.to_vec()).expect("UTF-8 Scalar HTML");
        assert!(html.to_ascii_lowercase().contains("scalar"));
        assert!(html.contains("dd-embeddings-rs API"));
        assert!(!html.contains("dd-embeddings-rs API (public)"));
    }
}
