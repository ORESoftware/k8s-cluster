#!/usr/bin/env python3
from pathlib import Path


def replace_once(source: str, before: str, after: str, label: str) -> str:
    count = source.count(before)
    if count != 1:
        raise SystemExit(f"{label}: expected one anchor, found {count}")
    return source.replace(before, after, 1)


cargo_path = Path("Cargo.toml")
cargo = cargo_path.read_text()
cargo = replace_once(
    cargo,
    'maud = "0.27"\npulldown-cmark = { version = "0.12", default-features = false, features = ["html"] }',
    'maud = "0.27"\npulldown-cmark = { version = "0.12", default-features = false, features = ["html"] }\nutoipa = "=5.5.0"\nutoipa-axum = "=0.2.0"\nutoipa-scalar = "=0.3.0"',
    "Cargo OpenAPI dependencies",
)
cargo_path.write_text(cargo)

main_path = Path("src/main.rs")
main = main_path.read_text()
main = replace_once(
    main,
    '''    let role = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("TOR_ROLE").ok())
        .unwrap_or_default();
    let telemetry_service = match role.as_str() {
''',
    '''    let role = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("TOR_ROLE").ok())
        .unwrap_or_default();
    if let Some(scope) = role.strip_prefix("--export-openapi=") {
        print!("{}", web::export_openapi(scope)?);
        return Ok(());
    }
    let telemetry_service = match role.as_str() {
''',
    "side-effect-free OpenAPI export",
)
main_path.write_text(main)

web_path = Path("src/web.rs")
web = web_path.read_text()
web = replace_once(web, "use std::collections::HashMap;\n", "", "remove dynamic fetch query map")
web = replace_once(
    web,
    '''use axum::extract::{Path as AxPath, Query, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Json, Response};
use axum::routing::get;
use axum::Router;
''',
    '''use axum::extract::{Path as AxPath, Query, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Json, Response};
use axum::Extension;
use serde::{Deserialize, Serialize};
use utoipa::openapi::OpenApi;
use utoipa_axum::{router::OpenApiRouter, routes};
''',
    "Axum and OpenAPI imports",
)
web = replace_once(
    web,
    '''use crate::stats::Stats;

/// Cap on the response body''',
    '''use crate::stats::Stats;

mod openapi;
use openapi::{ApiDocs, SharedApiDocs};

const OPENAPI_CONTENT_TYPE: &str = "application/vnd.oai.openapi+json;version=3.1";

/// Cap on the response body''',
    "OpenAPI module declaration",
)
web = replace_once(
    web,
    '''type AppState = Arc<WebConfig>;

pub async fn run(cfg: Arc<WebConfig>) -> Result<()> {
    let app = Router::new()
        .route("/", get(index))
        .route("/api/status", get(status))
        .route("/api/fetch", get(fetch))
        .route("/ws/stats", get(ws_stats))
        .route("/vendor/{file}", get(vendor))
        .route("/docs", get(docs_index))
        .route("/docs/{name}", get(docs_page))
        .route("/proxy.pac", get(proxy_pac))
        .route("/healthz", get(|| async { "ok" }))
        .layer(middleware::from_fn_with_state(cfg.clone(), require_token))
        .layer(TraceLayer::new_for_http())
        .with_state(cfg.clone());

    let listener = TcpListener::bind(&cfg.ui_listen).await?;
    info!(listen = %cfg.ui_listen, "web dashboard listening");
    axum::serve(listener, app.into_make_service()).await?;
    return Ok(());
}

/// Config + live counters as JSON, shared by `/api/status` and the WebSocket.
fn status_value(cfg: &WebConfig) -> serde_json::Value {
    let s = cfg.stats.snapshot();
    let relays: Vec<serde_json::Value> = cfg
        .directory
        .as_ref()
        .map(|d| {
            d.relays
                .iter()
                .map(|r| serde_json::json!({ "name": r.name, "addr": r.addr }))
                .collect()
        })
        .unwrap_or_default();
    return serde_json::json!({
        "backend": cfg.connector.backend(),
        "socks_listen": cfg.socks_listen,
        "hops": cfg.hops,
        "relay_count": relays.len(),
        "relays": relays,
        "circuits_built": s.circuits_built,
        "circuits_failed": s.circuits_failed,
        "circuits_active": s.circuits_active,
    });
}

async fn status(State(cfg): State<AppState>) -> Json<serde_json::Value> {
    return Json(status_value(&cfg));
}
''',
    '''type AppState = Arc<WebConfig>;

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
struct RelayStatus {
    name: String,
    addr: String,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
struct StatusResponse {
    backend: String,
    socks_listen: String,
    hops: usize,
    relay_count: usize,
    relays: Vec<RelayStatus>,
    circuits_built: u64,
    circuits_failed: u64,
    circuits_active: u64,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct FetchQuery {
    /// Plaintext HTTP URL fetched through a fresh onion circuit.
    url: Option<String>,
    /// Compatibility query token. Prefer `Authorization: Bearer`.
    token: Option<String>,
}

pub async fn run(cfg: Arc<WebConfig>) -> Result<()> {
    let internal = openapi_document()?;
    let docs = Arc::new(ApiDocs::new(&internal)?);
    let (router, runtime_openapi) = openapi_router().split_for_parts();
    let runtime_openapi = openapi::finalize(runtime_openapi)?;
    debug_assert_eq!(
        openapi::canonical_json(&internal)?,
        openapi::canonical_json(&runtime_openapi)?,
        "runtime router and exported OpenAPI contract diverged"
    );
    let app = router
        .layer(Extension(docs))
        .layer(middleware::from_fn_with_state(cfg.clone(), require_token))
        .layer(TraceLayer::new_for_http())
        .with_state(cfg.clone());

    let listener = TcpListener::bind(&cfg.ui_listen).await?;
    info!(listen = %cfg.ui_listen, "web dashboard listening");
    axum::serve(listener, app.into_make_service()).await?;
    return Ok(());
}

fn openapi_router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(index))
        .routes(routes!(status))
        .routes(routes!(fetch))
        .routes(routes!(ws_stats))
        .routes(routes!(vendor))
        .routes(routes!(docs_index))
        .routes(routes!(docs_page))
        .routes(routes!(proxy_pac))
        .routes(routes!(healthz))
        .routes(routes!(openapi_json))
        .routes(routes!(api_docs_json))
        .routes(routes!(api_docs_ui))
        .routes(routes!(docs_api_ui))
        .routes(routes!(internal_openapi_json))
        .routes(routes!(internal_docs_api_ui))
}

pub fn openapi_document() -> Result<OpenApi> {
    openapi::finalize(openapi_router().into_openapi())
}

pub fn export_openapi(scope: &str) -> Result<String> {
    let internal = openapi_document()?;
    let document = openapi::document_for_scope(&internal, scope)?;
    return Ok(openapi::canonical_json(&document)?);
}

/// Config + live counters as JSON, shared by `/api/status` and the WebSocket.
fn status_value(cfg: &WebConfig) -> StatusResponse {
    let s = cfg.stats.snapshot();
    let relays: Vec<RelayStatus> = cfg
        .directory
        .as_ref()
        .map(|d| {
            d.relays
                .iter()
                .map(|r| RelayStatus {
                    name: r.name.clone(),
                    addr: r.addr.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    return StatusResponse {
        backend: cfg.connector.backend().to_owned(),
        socks_listen: cfg.socks_listen.clone(),
        hops: cfg.hops,
        relay_count: relays.len(),
        relays,
        circuits_built: s.circuits_built,
        circuits_failed: s.circuits_failed,
        circuits_active: s.circuits_active,
    };
}

#[utoipa::path(
    get,
    path = "/api/status",
    operation_id = "getTorDashboardStatus",
    tag = "dashboard",
    responses((status = 200, description = "Live dashboard status and relay inventory", body = StatusResponse)),
    security(("ui_token" = []))
)]
async fn status(State(cfg): State<AppState>) -> Json<StatusResponse> {
    return Json(status_value(&cfg));
}
''',
    "router and typed status contract",
)

web = replace_once(
    web,
    '''async fn stats_socket(mut socket: WebSocket, cfg: AppState) {
    loop {
        let payload = status_value(&cfg).to_string();
''',
    '''async fn stats_socket(mut socket: WebSocket, cfg: AppState) {
    loop {
        let payload = serde_json::to_string(&status_value(&cfg))
            .unwrap_or_else(|_| "{\\"error\\":\\"status serialization failed\\"}".to_owned());
''',
    "typed WebSocket status serialization",
)

annotations = [
    (
        'async fn index(State(cfg): State<AppState>) -> Html<String> {',
        '''#[utoipa::path(
    get,
    path = "/",
    operation_id = "getTorDashboard",
    tag = "dashboard",
    responses((status = 200, description = "Interactive Tor dashboard", body = String, content_type = "text/html")),
    security(("ui_token" = []))
)]
async fn index(State(cfg): State<AppState>) -> Html<String> {''',
        "dashboard annotation",
    ),
    (
        '''async fn ws_stats(
    State(cfg): State<AppState>,''',
        '''#[utoipa::path(
    get,
    path = "/ws/stats",
    operation_id = "streamTorDashboardStats",
    tag = "dashboard",
    responses((status = 101, description = "WebSocket upgrade for live counters")),
    security(("ui_token" = []))
)]
async fn ws_stats(
    State(cfg): State<AppState>,''',
        "WebSocket annotation",
    ),
    (
        '''async fn fetch(
    State(cfg): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Html<String> {''',
        '''#[utoipa::path(
    get,
    path = "/api/fetch",
    operation_id = "fetchThroughTorCircuit",
    tag = "dashboard",
    params(FetchQuery),
    responses(
        (status = 200, description = "Escaped htmx result fragment", body = String, content_type = "text/html"),
        (status = 401, description = "Dashboard token missing or invalid", body = String, content_type = "text/plain")
    ),
    security(("ui_token" = []))
)]
async fn fetch(
    State(cfg): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<FetchQuery>,
) -> Html<String> {''',
        "fetch annotation and query DTO",
    ),
    (
        'async fn docs_index(State(cfg): State<AppState>) -> Html<String> {',
        '''#[utoipa::path(
    get,
    path = "/docs",
    operation_id = "listTorDocumentation",
    tag = "documentation",
    responses((status = 200, description = "Markdown documentation index", body = String, content_type = "text/html"))
)]
async fn docs_index(State(cfg): State<AppState>) -> Html<String> {''',
        "docs index annotation",
    ),
    (
        'async fn docs_page(State(cfg): State<AppState>, AxPath(name): AxPath<String>) -> Response {',
        '''#[utoipa::path(
    get,
    path = "/docs/{name}",
    operation_id = "getTorDocumentationPage",
    tag = "documentation",
    params(("name" = String, Path, description = "Safe markdown document slug")),
    responses(
        (status = 200, description = "Rendered markdown documentation", body = String, content_type = "text/html"),
        (status = 400, description = "Invalid document slug", body = String),
        (status = 404, description = "Document not found", body = String)
    )
)]
async fn docs_page(State(cfg): State<AppState>, AxPath(name): AxPath<String>) -> Response {''',
        "docs page annotation",
    ),
    (
        'async fn proxy_pac(State(cfg): State<AppState>) -> Response {',
        '''#[utoipa::path(
    get,
    path = "/proxy.pac",
    operation_id = "getTorProxyAutoConfig",
    tag = "proxy",
    responses((status = 200, description = "Browser proxy auto-configuration", body = String, content_type = "application/x-ns-proxy-autoconfig"))
)]
async fn proxy_pac(State(cfg): State<AppState>) -> Response {''',
        "PAC annotation",
    ),
    (
        'async fn vendor(AxPath(file): AxPath<String>) -> Response {',
        '''#[utoipa::path(
    get,
    path = "/vendor/{file}",
    operation_id = "getTorDashboardAsset",
    tag = "assets",
    params(("file" = String, Path, description = "Allowlisted embedded asset name")),
    responses(
        (status = 200, description = "Embedded dashboard asset", body = String),
        (status = 404, description = "Asset not found", body = String)
    )
)]
async fn vendor(AxPath(file): AxPath<String>) -> Response {''',
        "vendor annotation",
    ),
]
for before, after, label in annotations:
    web = replace_once(web, before, after, label)

web = replace_once(
    web,
    '''    let url = match q.get("url") {
        Some(u) if !u.is_empty() => u.clone(),
''',
    '''    let url = match q.url.as_deref() {
        Some(u) if !u.is_empty() => u.to_owned(),
''',
    "typed fetch URL access",
)
web = replace_once(
    web,
    'fn authorized(cfg: &WebConfig, headers: &HeaderMap, q: &HashMap<String, String>) -> bool {',
    'fn authorized(cfg: &WebConfig, headers: &HeaderMap, q: &FetchQuery) -> bool {',
    "typed authorization query",
)
web = replace_once(
    web,
    '    let provided = q.get("token").map(|s| s.as_str()).or_else(|| {',
    '    let provided = q.token.as_deref().or_else(|| {',
    "typed token access",
)
web = replace_once(
    web,
    '''async fn require_token(State(cfg): State<AppState>, req: Request, next: Next) -> Response {
    if let Some(expected) = cfg.ui_token.as_deref() {
        let path = req.uri().path();
        let sensitive = matches!(path, "/" | "/api/status" | "/ws/stats" | "/api/fetch");
        if sensitive && !request_token_ok(&req, expected) {
''',
    '''fn requires_ui_token(path: &str) -> bool {
    return matches!(
        path,
        "/"
            | "/api/status"
            | "/ws/stats"
            | "/api/fetch"
            | "/internal/openapi.json"
            | "/internal/docs/api"
    );
}

async fn require_token(State(cfg): State<AppState>, req: Request, next: Next) -> Response {
    if let Some(expected) = cfg.ui_token.as_deref() {
        let path = req.uri().path();
        if requires_ui_token(path) && !request_token_ok(&req, expected) {
''',
    "shared sensitivity classifier",
)

insert_anchor = '/// Config + live counters as JSON, shared by `/api/status` and the WebSocket.\n'
docs_handlers = '''#[utoipa::path(
    get,
    path = "/healthz",
    operation_id = "getTorDashboardHealth",
    tag = "operations",
    responses((status = 200, description = "Dashboard process liveness", body = String, content_type = "text/plain"))
)]
async fn healthz() -> &'static str {
    "ok"
}

fn public_json_response(docs: SharedApiDocs) -> Response {
    return (
        [(header::CONTENT_TYPE, OPENAPI_CONTENT_TYPE)],
        docs.public_json.clone(),
    )
        .into_response();
}

fn public_scalar_response(docs: SharedApiDocs) -> Response {
    return (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        docs.public_scalar_html.clone(),
    )
        .into_response();
}

#[utoipa::path(
    get,
    path = "/openapi.json",
    operation_id = "getTorPublicOpenApiDocument",
    tag = "documentation",
    responses((status = 200, description = "Fail-closed public OpenAPI 3.1 document", content_type = "application/vnd.oai.openapi+json;version=3.1"))
)]
async fn openapi_json(Extension(docs): Extension<SharedApiDocs>) -> Response {
    public_json_response(docs)
}

#[utoipa::path(
    get,
    path = "/api/docs.json",
    operation_id = "getTorPublicOpenApiDocumentAlias",
    tag = "documentation",
    responses((status = 200, description = "Compatibility alias for the public OpenAPI document", content_type = "application/vnd.oai.openapi+json;version=3.1"))
)]
async fn api_docs_json(Extension(docs): Extension<SharedApiDocs>) -> Response {
    public_json_response(docs)
}

#[utoipa::path(
    get,
    path = "/api/docs",
    operation_id = "getTorPublicApiReference",
    tag = "documentation",
    responses((status = 200, description = "Interactive Scalar reference", body = String, content_type = "text/html"))
)]
async fn api_docs_ui(Extension(docs): Extension<SharedApiDocs>) -> Response {
    public_scalar_response(docs)
}

#[utoipa::path(
    get,
    path = "/docs/api",
    operation_id = "getTorPublicApiReferenceAlias",
    tag = "documentation",
    responses((status = 200, description = "Compatibility alias for the Scalar reference", body = String, content_type = "text/html"))
)]
async fn docs_api_ui(Extension(docs): Extension<SharedApiDocs>) -> Response {
    public_scalar_response(docs)
}

#[utoipa::path(
    get,
    path = "/internal/openapi.json",
    operation_id = "getTorInternalOpenApiDocument",
    tag = "documentation",
    responses((status = 200, description = "Complete private OpenAPI 3.1 document", content_type = "application/vnd.oai.openapi+json;version=3.1")),
    security(("ui_token" = []))
)]
async fn internal_openapi_json(Extension(docs): Extension<SharedApiDocs>) -> Response {
    return (
        [(header::CONTENT_TYPE, OPENAPI_CONTENT_TYPE)],
        docs.internal_json.clone(),
    )
        .into_response();
}

#[utoipa::path(
    get,
    path = "/internal/docs/api",
    operation_id = "getTorInternalApiReference",
    tag = "documentation",
    responses((status = 200, description = "Interactive private Scalar reference", body = String, content_type = "text/html")),
    security(("ui_token" = []))
)]
async fn internal_docs_api_ui(Extension(docs): Extension<SharedApiDocs>) -> Response {
    return (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        docs.internal_scalar_html.clone(),
    )
        .into_response();
}

'''
web = replace_once(web, insert_anchor, docs_handlers + insert_anchor, "documentation handlers")

# Add the missing dashboard annotation after status helpers are in place.
web = replace_once(
    web,
    'async fn index(State(cfg): State<AppState>) -> Html<String> {',
    '''#[utoipa::path(
    get,
    path = "/",
    operation_id = "getTorDashboard",
    tag = "dashboard",
    responses((status = 200, description = "Interactive Tor dashboard", body = String, content_type = "text/html")),
    security(("ui_token" = []))
)]
async fn index(State(cfg): State<AppState>) -> Html<String> {''',
    "dashboard annotation final",
)

web_path.write_text(web)
