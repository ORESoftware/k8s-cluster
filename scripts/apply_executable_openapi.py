#!/usr/bin/env python3
from pathlib import Path


def replace_once(path: str, before: str, after: str, label: str) -> None:
    file = Path(path)
    source = file.read_text()
    count = source.count(before)
    if count != 1:
        raise SystemExit(f"{path}: {label}: expected one anchor, found {count}")
    file.write_text(source.replace(before, after, 1))


replace_once(
    "Cargo.toml",
    'tower-http = { version = "0.7", features = ["trace"] }\nmaud = "0.27"',
    'tower-http = { version = "0.7", features = ["trace"] }\nutoipa = "=5.5.0"\nutoipa-axum = "=0.2.0"\nutoipa-scalar = "=0.3.0"\nmaud = "0.27"',
    "OpenAPI dependencies",
)

replace_once(
    "src/web.rs",
    '''use axum::response::{Html, IntoResponse, Json, Response};
use axum::routing::get;
use axum::Router;
use maud::{html, Markup, PreEscaped, DOCTYPE};''',
    '''use axum::response::{Html, IntoResponse, Json, Response};
use axum::{Extension, Router};
use maud::{html, Markup, PreEscaped, DOCTYPE};
use serde::Serialize;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::openapi::{Components, OpenApi};
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};
use utoipa_scalar::Scalar;''',
    "OpenAPI imports",
)

replace_once(
    "src/web.rs",
    "type AppState = Arc<WebConfig>;\n\npub async fn run(cfg: Arc<WebConfig>) -> Result<()> {",
    r'''type AppState = Arc<WebConfig>;

type SharedApiDocs = Arc<ApiDocs>;

#[derive(Clone)]
struct ApiDocs {
    json: axum::body::Bytes,
    html: axum::body::Bytes,
}

impl ApiDocs {
    fn new(openapi: &OpenApi) -> Result<Self, serde_json::Error> {
        Ok(Self {
            json: axum::body::Bytes::from(canonical_json(openapi)?),
            html: axum::body::Bytes::from(Scalar::new(openapi.clone()).to_html()),
        })
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
struct RelayStatus {
    name: String,
    addr: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
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

pub fn openapi_document() -> OpenApi {
    finalize(openapi_router().into_openapi())
}

pub fn canonical_json(openapi: &OpenApi) -> Result<String, serde_json::Error> {
    let mut json = serde_json::to_string_pretty(openapi)?;
    json.push('\n');
    Ok(json)
}

fn finalize(mut openapi: OpenApi) -> OpenApi {
    openapi.info.title = "tor-server dashboard API".to_owned();
    openapi.info.version = env!("CARGO_PKG_VERSION").to_owned();
    openapi.info.description = Some(
        "Executable HTTP dashboard, status, fetch-preview, WebSocket handshake, documentation, and PAC contract generated from the exact Axum handlers mounted by the client process."
            .to_owned(),
    );
    let components = openapi.components.get_or_insert_with(Components::new);
    components.add_security_scheme(
        "bearer_auth",
        SecurityScheme::Http(
            HttpBuilder::new()
                .scheme(HttpAuthScheme::Bearer)
                .bearer_format("TOR_UI_TOKEN")
                .description(Some(
                    "Set Authorization: Bearer <TOR_UI_TOKEN> for the fetch proxy primitive when dashboard authentication is configured."
                        .to_owned(),
                ))
                .build(),
        ),
    );
    openapi
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
}

pub async fn run(cfg: Arc<WebConfig>) -> Result<()> {''',
    "docs state and typed status",
)

replace_once(
    "src/web.rs",
    '''    let app = Router::new()
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
        .with_state(cfg.clone());''',
    '''    let openapi = openapi_document();
    let docs = Arc::new(ApiDocs::new(&openapi)?);
    let (router, runtime_openapi) = openapi_router().split_for_parts();
    debug_assert_eq!(
        canonical_json(&openapi)?,
        canonical_json(&finalize(runtime_openapi))?,
        "runtime dashboard router and exported OpenAPI contract diverged"
    );
    let app = router
        .layer(middleware::from_fn_with_state(cfg.clone(), require_token))
        .layer(TraceLayer::new_for_http())
        .with_state(cfg.clone())
        .layer(Extension(docs));''',
    "runtime OpenApiRouter",
)

replace_once(
    "src/web.rs",
    '''fn status_value(cfg: &WebConfig) -> serde_json::Value {
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
}''',
    '''fn status_value(cfg: &WebConfig) -> StatusResponse {
    let s = cfg.stats.snapshot();
    let relays: Vec<RelayStatus> = cfg
        .directory
        .as_ref()
        .map(|directory| {
            directory
                .relays
                .iter()
                .map(|relay| RelayStatus {
                    name: relay.name.clone(),
                    addr: relay.addr.to_string(),
                })
                .collect()
        })
        .unwrap_or_default();
    StatusResponse {
        backend: cfg.connector.backend().to_owned(),
        socks_listen: cfg.socks_listen.clone(),
        hops: cfg.hops,
        relay_count: relays.len(),
        relays,
        circuits_built: s.circuits_built,
        circuits_failed: s.circuits_failed,
        circuits_active: s.circuits_active,
    }
}

#[utoipa::path(
    get,
    path = "/api/status",
    operation_id = "getTorDashboardStatus",
    tag = "status",
    security(()),
    responses((status = 200, description = "Dashboard configuration and live circuit counters", body = StatusResponse))
)]
async fn status(State(cfg): State<AppState>) -> Json<StatusResponse> {
    Json(status_value(&cfg))
}''',
    "typed status contract",
)
replace_once(
    "src/web.rs",
    "        let payload = status_value(&cfg).to_string();",
    '''        let payload = serde_json::to_string(&status_value(&cfg))
            .expect("typed dashboard status must serialize");''',
    "WebSocket typed payload",
)

replace_once(
    "src/web.rs",
    "async fn ws_stats(\n    State(cfg): State<AppState>,",
    '''#[utoipa::path(
    get,
    path = "/ws/stats",
    operation_id = "openTorDashboardStatsWebSocket",
    tag = "status",
    security(()),
    responses(
        (status = 101, description = "WebSocket upgrade; text frames contain StatusResponse JSON"),
        (status = 403, description = "Cross-origin WebSocket rejected")
    )
)]
async fn ws_stats(
    State(cfg): State<AppState>,''',
    "WebSocket contract",
)
replace_once(
    "src/web.rs",
    "async fn index(State(cfg): State<AppState>) -> Html<String> {",
    '''#[utoipa::path(
    get,
    path = "/",
    operation_id = "getTorDashboard",
    tag = "dashboard",
    security(()),
    responses((status = 200, description = "Server-rendered dashboard", body = String, content_type = "text/html"))
)]
async fn index(State(cfg): State<AppState>) -> Html<String> {''',
    "dashboard contract",
)
replace_once(
    "src/web.rs",
    '''async fn fetch(
    State(cfg): State<AppState>,''',
    '''#[utoipa::path(
    get,
    path = "/api/fetch",
    operation_id = "fetchHttpThroughOnionCircuit",
    tag = "proxy",
    params(
        ("url" = String, Query, description = "Plaintext http:// URL to fetch through a fresh circuit"),
        ("token" = Option<String>, Query, description = "TOR_UI_TOKEN alternative for htmx clients")
    ),
    security(("bearer_auth" = [])),
    responses((status = 200, description = "Escaped htmx HTML result or bounded error fragment", body = String, content_type = "text/html"))
)]
async fn fetch(
    State(cfg): State<AppState>,''',
    "fetch contract",
)
replace_once(
    "src/web.rs",
    "async fn vendor(AxPath(file): AxPath<String>) -> Response {",
    '''#[utoipa::path(
    get,
    path = "/vendor/{file}",
    operation_id = "getTorDashboardVendorAsset",
    tag = "dashboard",
    params(("file" = String, Path, description = "Allowlisted vendored asset filename")),
    security(()),
    responses(
        (status = 200, description = "Vendored browser asset", body = String),
        (status = 404, description = "Asset is not allowlisted")
    )
)]
async fn vendor(AxPath(file): AxPath<String>) -> Response {''',
    "vendor contract",
)
replace_once(
    "src/web.rs",
    "async fn docs_index(State(cfg): State<AppState>) -> Html<String> {",
    '''#[utoipa::path(
    get,
    path = "/docs",
    operation_id = "getTorDocumentationIndex",
    tag = "documentation",
    security(()),
    responses((status = 200, description = "Server-rendered documentation index", body = String, content_type = "text/html"))
)]
async fn docs_index(State(cfg): State<AppState>) -> Html<String> {''',
    "docs index contract",
)
replace_once(
    "src/web.rs",
    "async fn docs_page(State(cfg): State<AppState>, AxPath(name): AxPath<String>) -> Response {",
    '''#[utoipa::path(
    get,
    path = "/docs/{name}",
    operation_id = "getTorDocumentationPage",
    tag = "documentation",
    params(("name" = String, Path, description = "Sanitized markdown documentation slug")),
    security(()),
    responses(
        (status = 200, description = "Rendered markdown documentation", body = String, content_type = "text/html"),
        (status = 400, description = "Invalid documentation slug"),
        (status = 404, description = "Documentation page does not exist")
    )
)]
async fn docs_page(State(cfg): State<AppState>, AxPath(name): AxPath<String>) -> Response {''',
    "docs page contract",
)
replace_once(
    "src/web.rs",
    "async fn proxy_pac(State(cfg): State<AppState>) -> Response {",
    '''#[utoipa::path(
    get,
    path = "/proxy.pac",
    operation_id = "getTorProxyAutoConfig",
    tag = "proxy",
    security(()),
    responses((status = 200, description = "Browser proxy auto-configuration", body = String, content_type = "application/x-ns-proxy-autoconfig"))
)]
async fn proxy_pac(State(cfg): State<AppState>) -> Response {''',
    "PAC contract",
)

insert_anchor = "async fn index(State(cfg): State<AppState>) -> Html<String> {"
source = Path("src/web.rs").read_text()
index = source.index("#[utoipa::path(\n    get,\n    path = \"/\",", source.index(insert_anchor) - 500)
docs_handlers = r'''#[utoipa::path(
    get,
    path = "/healthz",
    operation_id = "getTorDashboardHealth",
    tag = "operations",
    security(()),
    responses((status = 200, description = "Dashboard process liveness", body = String, content_type = "text/plain"))
)]
async fn healthz() -> &'static str {
    "ok"
}

fn openapi_response(docs: &ApiDocs) -> Response {
    (
        [(
            header::CONTENT_TYPE,
            "application/vnd.oai.openapi+json;version=3.1",
        )],
        docs.json.clone(),
    )
        .into_response()
}

fn api_docs_response(docs: &ApiDocs) -> Response {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        docs.html.clone(),
    )
        .into_response()
}

#[utoipa::path(
    get,
    path = "/openapi.json",
    operation_id = "getTorDashboardOpenApi",
    tag = "documentation",
    security(()),
    responses((status = 200, description = "Executable OpenAPI 3.1 document", body = String, content_type = "application/vnd.oai.openapi+json;version=3.1"))
)]
async fn openapi_json(Extension(docs): Extension<SharedApiDocs>) -> Response {
    openapi_response(&docs)
}

#[utoipa::path(
    get,
    path = "/api/docs.json",
    operation_id = "getTorDashboardOpenApiCompatibilityAlias",
    tag = "documentation",
    security(()),
    responses((status = 200, description = "Compatibility alias for the executable OpenAPI document", body = String, content_type = "application/vnd.oai.openapi+json;version=3.1"))
)]
async fn api_docs_json(Extension(docs): Extension<SharedApiDocs>) -> Response {
    openapi_response(&docs)
}

#[utoipa::path(
    get,
    path = "/api/docs",
    operation_id = "getTorDashboardApiReference",
    tag = "documentation",
    security(()),
    responses((status = 200, description = "Scalar API reference", body = String, content_type = "text/html"))
)]
async fn api_docs_ui(Extension(docs): Extension<SharedApiDocs>) -> Response {
    api_docs_response(&docs)
}

#[utoipa::path(
    get,
    path = "/docs/api",
    operation_id = "getTorDashboardApiReferenceCompatibilityAlias",
    tag = "documentation",
    security(()),
    responses((status = 200, description = "Compatibility alias for the Scalar API reference", body = String, content_type = "text/html"))
)]
async fn docs_api_ui(Extension(docs): Extension<SharedApiDocs>) -> Response {
    api_docs_response(&docs)
}

'''
Path("src/web.rs").write_text(source[:index] + docs_handlers + source[index:])

replace_once(
    "src/main.rs",
    '''async fn main() -> Result<()> {
    let role = std::env::args()''',
    '''async fn main() -> Result<()> {
    if std::env::args().any(|argument| argument == "--export-openapi") {
        print!("{}", web::canonical_json(&web::openapi_document())?);
        return Ok(());
    }

    let role = std::env::args()''',
    "side-effect-free OpenAPI export",
)

workflow = r'''name: executable dashboard OpenAPI

on:
  pull_request:
    paths:
      - '.github/workflows/openapi-contract.yml'
      - 'Cargo.toml'
      - 'Cargo.lock'
      - 'generated/openapi.json'
      - 'src/main.rs'
      - 'src/web.rs'
  push:
    branches:
      - main
    paths:
      - '.github/workflows/openapi-contract.yml'
      - 'Cargo.toml'
      - 'Cargo.lock'
      - 'generated/openapi.json'
      - 'src/main.rs'
      - 'src/web.rs'
  workflow_dispatch:

permissions:
  contents: read

concurrency:
  group: tor-openapi-${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true

jobs:
  contract:
    runs-on: ubuntu-24.04
    timeout-minutes: 35
    steps:
      - name: Checkout
        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false
          show-progress: false

      - name: Setup Rust 1.88
        uses: dtolnay/rust-toolchain@4cda84d5c5c54efe2404f9d843567869ab1699d4
        with:
          toolchain: '1.88.0'
          components: rustfmt, clippy

      - name: Compile and test
        run: |
          set -euo pipefail
          cargo fmt --all -- --check
          cargo check --locked
          cargo clippy --locked --all-targets -- -D warnings
          cargo test --locked --all-targets

      - name: Prove deterministic executable export
        run: |
          set -euo pipefail
          cargo run --locked --quiet -- --export-openapi > /tmp/openapi.1.json
          cargo run --locked --quiet -- --export-openapi > /tmp/openapi.2.json
          cmp /tmp/openapi.1.json /tmp/openapi.2.json
          cmp /tmp/openapi.1.json generated/openapi.json

      - name: Validate paths, security, and WebSocket declaration
        run: |
          python3 - <<'PY'
          import json
          from pathlib import Path

          document = json.loads(Path('generated/openapi.json').read_text())
          expected = {
              '/', '/api/status', '/api/fetch', '/ws/stats', '/vendor/{file}',
              '/docs', '/docs/{name}', '/proxy.pac', '/healthz',
              '/openapi.json', '/api/docs.json', '/api/docs', '/docs/api',
          }
          assert set(document['paths']) == expected
          assert 'bearer_auth' in document['components']['securitySchemes']
          assert document['paths']['/api/fetch']['get']['security'] == [{'bearer_auth': []}]
          assert '101' in document['paths']['/ws/stats']['get']['responses']
          operation_ids = [
              operation['operationId']
              for item in document['paths'].values()
              for method, operation in item.items()
              if method.lower() in {'get', 'post', 'put', 'patch', 'delete'}
          ]
          assert len(operation_ids) == len(set(operation_ids))
          PY

      - name: Reject drift
        run: |
          set -euo pipefail
          git diff --check
          test -z "$(git status --short)"
          ! grep -R -n -E '^(<<<<<<<|=======|>>>>>>>)' src generated Cargo.toml
'''
Path(".github/workflows/openapi-contract.yml").write_text(workflow)
Path("docs/openapi-contract.md").write_text(
    """# Executable dashboard OpenAPI\n\n"
    "All dashboard HTTP routes are registered and documented together through "
    "`utoipa_axum::routes!`. The WebSocket handshake is represented as a 101 "
    "upgrade operation; text frames contain the same typed `StatusResponse` used "
    "by `GET /api/status`.\n\n"
    "Standard documentation routes are `/openapi.json`, `/api/docs.json`, "
    "`/api/docs`, and `/docs/api`. Export the exact contract without starting "
    "telemetry, sockets, relays, SOCKS, or the dashboard with:\n\n"
    "```bash\n"
    "cargo run --locked -- --export-openapi\n"
    "```\n"
)
