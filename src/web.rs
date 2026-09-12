//! Web dashboard + docs server (client mode).
//!
//! Runs alongside the SOCKS5 listener and exposes, on `TOR_UI_LISTEN`:
//!   GET /                 dashboard UI (Maud-rendered, htmx-driven)
//!   GET /api/status       JSON: config + live circuit counters
//!   GET /api/fetch?url=   build a fresh onion circuit, GET the URL, return an
//!                         htmx HTML fragment (server-rendered, escaped)
//!   GET /ws/stats         WebSocket: live circuit counters pushed to the grid
//!   GET /vendor/{file}    vendored static assets (htmx) — no CDN at runtime
//!   GET /docs             index of the markdown docs
//!   GET /docs/{name}      one rendered markdown doc
//!   GET /proxy.pac        browser proxy auto-config pointing at the SOCKS port
//!   GET /healthz          liveness probe
//!
//! The `/api/fetch` endpoint proves end-to-end reachability from a browser and
//! reveals the exit's public IP. It handles plaintext HTTP only; for HTTPS,
//! point the browser/curl at the SOCKS proxy (which tunnels TLS end to end).
//!
//! Rendering is done with Maud (compile-time HTML, auto-escaping), so untrusted
//! values — a fetched server's status line, directory relay names — are escaped
//! at the source rather than by ad-hoc client-side string handling.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use subtle::ConstantTimeEq;

use anyhow::{bail, Result};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as AxPath, Query, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Json, Response};
use axum::Extension;
use maud::{html, Markup, PreEscaped, DOCTYPE};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::time::{sleep, timeout, Duration};
use tower_http::trace::TraceLayer;
use tracing::info;
use utoipa::openapi::OpenApi;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::config::Directory;
use crate::connector::Connector;
use crate::stats::Stats;

mod openapi;
use openapi::{ApiDocs, SharedApiDocs};

const OPENAPI_CONTENT_TYPE: &str = "application/vnd.oai.openapi+json;version=3.1";

/// Cap on the response body we buffer for the in-browser fetch preview.
const FETCH_MAX_BODY: usize = 512 * 1024;
const FETCH_TIMEOUT: Duration = Duration::from_secs(25);
/// Interval between live stat pushes on the `/ws/stats` socket.
const WS_PUSH_INTERVAL: Duration = Duration::from_secs(2);

pub struct WebConfig {
    pub ui_listen: String,
    pub socks_listen: String,
    pub connector: Arc<Connector>,
    /// Present only for the overlay backend; used to display the relay list.
    pub directory: Option<Directory>,
    pub hops: usize,
    pub docs_dir: PathBuf,
    pub stats: Arc<Stats>,
    /// If set, the `/api/fetch` proxy primitive requires this token (via
    /// `?token=` or `Authorization: Bearer`). Guards the dashboard when it is
    /// bound to a non-loopback address.
    pub ui_token: Option<String>,
}

type AppState = Arc<WebConfig>;

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

#[utoipa::path(
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

/// WebSocket that pushes the live counters to the dashboard grid every couple of
/// seconds. The initial values are also server-rendered into the page, so the
/// dashboard is correct even before the socket delivers its first frame.
#[utoipa::path(
    get,
    path = "/ws/stats",
    operation_id = "streamTorDashboardStats",
    tag = "dashboard",
    responses((status = 101, description = "WebSocket upgrade for live counters")),
    security(("ui_token" = []))
)]
async fn ws_stats(
    State(cfg): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    // WebSockets are not subject to the same-origin read protection that blocks
    // a cross-site page from reading `/api/status`. Without this check a page the
    // user visits could open ws://<dashboard>/ws/stats and exfiltrate the relay
    // list + counters. Browsers always send Origin on a WS handshake; reject when
    // it is present and does not match Host. Non-browser clients (no Origin) pass.
    if !same_origin(&headers) {
        return (StatusCode::FORBIDDEN, "cross-origin websocket rejected").into_response();
    }
    return ws.on_upgrade(move |socket| stats_socket(socket, cfg));
}

/// True unless the request carries an `Origin` whose host:port differs from the
/// `Host` header (cross-site WebSocket hijacking).
fn same_origin(headers: &HeaderMap) -> bool {
    let origin = match headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        Some(o) => o,
        None => return true, // non-browser client
    };
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let origin_hostport = origin
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(origin);
    return !host.is_empty() && origin_hostport == host;
}

async fn stats_socket(mut socket: WebSocket, cfg: AppState) {
    loop {
        let payload = serde_json::to_string(&status_value(&cfg))
            .unwrap_or_else(|_| "{\"error\":\"status serialization failed\"}".to_owned());
        if socket.send(Message::Text(payload.into())).await.is_err() {
            break;
        }
        sleep(WS_PUSH_INTERVAL).await;
    }
}

#[utoipa::path(
    get,
    path = "/",
    operation_id = "getTorDashboard",
    tag = "dashboard",
    responses((status = 200, description = "Interactive Tor dashboard", body = String, content_type = "text/html")),
    security(("ui_token" = []))
)]
async fn index(State(cfg): State<AppState>) -> Html<String> {
    let s = cfg.stats.snapshot();
    let relays: Vec<(String, String)> = cfg
        .directory
        .as_ref()
        .map(|d| {
            d.relays
                .iter()
                .map(|r| (r.name.clone(), r.addr.clone()))
                .collect()
        })
        .unwrap_or_default();
    let socks_display = cfg.socks_listen.replace("0.0.0.0", "127.0.0.1");

    let markup = html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width,initial-scale=1";
                title { "tor-server · onion proxy" }
                style { (PreEscaped(DASH_CSS)) }
                script src="/vendor/htmx.min.js" {}
            }
            body {
                header {
                    h1 { "tor-server" }
                    span class="badge" id="backend-badge" data-testid="backend-badge" {
                        (cfg.connector.backend()) " backend"
                    }
                    span class="muted" style="margin-left:auto" {
                        a href="/docs" data-testid="docs-link" { "Docs" }
                    }
                }
                main {
                    div class="grid" data-testid="status-grid" {
                        div class="card" { div class="k" { "Circuits built" } div class="v" id="s-built" data-testid="stat-built" { (s.circuits_built) } }
                        div class="card" { div class="k" { "Active" } div class="v" id="s-active" data-testid="stat-active" { (s.circuits_active) } }
                        div class="card" { div class="k" { "Failed" } div class="v" id="s-failed" data-testid="stat-failed" { (s.circuits_failed) } }
                        div class="card" { div class="k" { "Hops / circuit" } div class="v" id="s-hops" data-testid="stat-hops" { (cfg.hops) } }
                        div class="card" { div class="k" { "Relays" } div class="v" id="s-relays" data-testid="stat-relays" { (relays.len()) } }
                    }

                    section {
                        h2 { "Browse through the onion network" }
                        p class="muted" {
                            "Fetches a plaintext " code { "http://" } " URL through a fresh multi-hop circuit — a quick way to confirm traffic exits via the relays (check your exit IP). For HTTPS and real browsing, point your app at the SOCKS proxy below."
                        }
                        div class="row" {
                            input type="text" id="url" name="url" data-testid="url-input" placeholder="http://example.com/" value="http://example.com/";
                            button id="go" data-testid="fetch-btn"
                                hx-get="/api/fetch" hx-include="[name='url']"
                                hx-target="[data-testid='result-meta']" hx-swap="innerHTML" { "Fetch" }
                        }
                        div class="examples" {
                            button onclick="document.getElementById('url').value='http://example.com/'" { "example.com" }
                            button onclick="document.getElementById('url').value='http://icanhazip.com/'" { "my exit IP" }
                            button onclick="document.getElementById('url').value='http://neverssl.com/'" { "neverssl.com" }
                        }
                        div id="result-meta" class="muted" data-testid="result-meta" style="margin-top:1rem" {}
                        div id="fetch-body" {}
                    }

                    section {
                        h2 { "Point your apps at the proxy" }
                        p { "SOCKS5 proxy (remote DNS): " code id="socks-addr" data-testid="socks-addr" { (cfg.socks_listen) } }
                        ul {
                            li { strong { "curl:" } " " code { "curl -x socks5h://" span class="socks" { (socks_display) } " https://example.com" } }
                            li { strong { "Firefox/Chromium:" } " use the SOCKS host/port above, or a " a href="/proxy.pac" data-testid="pac-link" { "proxy.pac" } " auto-config." }
                            li { strong { "HTTP CONNECT:" } " if " code { "TOR_HTTP_LISTEN" } " is set, point an HTTP proxy at that address." }
                        }
                    }

                    section {
                        h2 { "Circuit relays" }
                        div id="relays" data-testid="relays" {
                            @for (name, addr) in &relays {
                                span class="pill" data-testid="relay-pill" { (name) " · " (addr) }
                            }
                        }
                    }
                }
                script { (PreEscaped(WS_SCRIPT)) }
            }
        }
    };
    return Html(markup.into_string());
}

#[utoipa::path(
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
) -> Html<String> {
    if !authorized(&cfg, &headers, &q) {
        return Html(
            render_fetch_error(
                "unauthorized: /api/fetch requires the TOR_UI_TOKEN (pass ?token= or Authorization: Bearer)",
            )
            .into_string(),
        );
    }
    let url = match q.url.as_deref() {
        Some(u) if !u.is_empty() => u.to_owned(),
        _ => return Html(render_fetch_error("missing ?url= parameter").into_string()),
    };
    let started = Instant::now();
    match onion_get(&cfg, &url).await {
        Ok(mut v) => {
            v["elapsed_ms"] = serde_json::json!(started.elapsed().as_millis() as u64);
            Html(render_fetch_ok(&v).into_string())
        }
        Err(e) => {
            cfg.stats.failed();
            Html(render_fetch_error(&format!("{e:#}")).into_string())
        }
    }
}

/// htmx fragment for a successful fetch: the status line goes into `#result-meta`
/// (the swap target); the headers/preview are OOB-swapped into `#fetch-body`.
fn render_fetch_ok(v: &serde_json::Value) -> Markup {
    let status_line = v["status_line"].as_str().unwrap_or("");
    let backend = v["backend"].as_str().unwrap_or("?");
    let body_len = v["body_len"].as_u64().unwrap_or(0);
    let elapsed = v["elapsed_ms"].as_u64().unwrap_or(0);
    let headers = v["headers"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|h| h.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let preview = v["body_preview"].as_str().unwrap_or("");
    let content_type = v["content_type"].as_str().unwrap_or("");
    let is_html = v["is_text"].as_bool().unwrap_or(false)
        && content_type.to_ascii_lowercase().contains("html");
    return html! {
        span data-testid="result-status" { (status_line) } " · via "
        span class="pill" { (backend) } " · " (body_len) " bytes · " (elapsed) " ms"
        div id="fetch-body" hx-swap-oob="innerHTML" {
            pre data-testid="result" { (headers) "\n\n" (preview) }
            @if is_html {
                // Fully sandboxed: no scripts, no same-origin — a safe preview.
                iframe data-testid="rendered" sandbox="" srcdoc=(preview) {}
            }
        }
    };
}

fn render_fetch_error(message: &str) -> Markup {
    return html! {
        span class="err" { "Error: " (message) }
        div id="fetch-body" hx-swap-oob="innerHTML" {}
    };
}

/// Whether a request may use the `/api/fetch` proxy primitive. Open when no
/// `TOR_UI_TOKEN` is configured; otherwise the token must match (checked in
/// constant time).
fn authorized(cfg: &WebConfig, headers: &HeaderMap, q: &FetchQuery) -> bool {
    let expected = match &cfg.ui_token {
        None => return true,
        Some(t) => t,
    };
    let provided = q.token.as_deref().or_else(|| {
        headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
    });
    return match provided {
        Some(p) => ct_str_eq(p, expected),
        None => false,
    };
}

/// Constant-time string comparison (differing lengths return false immediately —
/// the token length is not itself secret).
fn ct_str_eq(a: &str, b: &str) -> bool {
    return bool::from(a.as_bytes().ct_eq(b.as_bytes()));
}

/// When a UI token is configured, require it for the endpoints that expose the
/// relay directory / live counters or drive the fetch proxy — not just
/// `/api/fetch`. Otherwise a non-loopback dashboard leaks the relay list via `/`,
/// `/api/status`, and `/ws/stats` to any unauthenticated client. `/docs`,
/// `/proxy.pac`, `/vendor/*`, and `/healthz` carry nothing sensitive and stay
/// open so navigation, the PAC file, the JS asset, and liveness probes work.
fn requires_ui_token(path: &str) -> bool {
    return matches!(
        path,
        "/" | "/api/status"
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
            return (
                StatusCode::UNAUTHORIZED,
                "unauthorized: dashboard requires TOR_UI_TOKEN (?token= or Authorization: Bearer)",
            )
                .into_response();
        }
    }
    return next.run(req).await;
}

/// Token from `?token=` (percent-decoded) or `Authorization: Bearer`, compared in
/// constant time.
fn request_token_ok(req: &Request, expected: &str) -> bool {
    if let Some(query) = req.uri().query() {
        for pair in query.split('&') {
            if let Some(raw) = pair.strip_prefix("token=") {
                if ct_str_eq(&percent_decode(raw), expected) {
                    return true;
                }
            }
        }
    }
    if let Some(value) = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
    {
        if ct_str_eq(value, expected) {
            return true;
        }
    }
    return false;
}

/// Minimal application/x-www-form-urlencoded percent-decoding for the token
/// query parameter (avoids pulling a dependency for one field).
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => match (hex_val(b[i + 1]), hex_val(b[i + 2])) {
                (Some(h), Some(l)) => {
                    out.push(h * 16 + l);
                    i += 3;
                }
                _ => {
                    out.push(b[i]);
                    i += 1;
                }
            },
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    return String::from_utf8_lossy(&out).into_owned();
}

fn hex_val(c: u8) -> Option<u8> {
    return match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    };
}

/// Open a stream through the active backend and perform one plaintext HTTP GET.
async fn onion_get(cfg: &WebConfig, url: &str) -> Result<serde_json::Value> {
    let (host, port, path) = parse_http_url(url)?;

    let mut stream = cfg.connector.connect(&host, port).await?;
    cfg.stats.built();

    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: tor-server-ui/0.1\r\nAccept: */*\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).await?;

    let raw = timeout(FETCH_TIMEOUT, read_all_capped(&mut stream, FETCH_MAX_BODY))
        .await
        .map_err(|_| anyhow::anyhow!("fetch timed out after {}s", FETCH_TIMEOUT.as_secs()))??;

    if raw.is_empty() {
        bail!("no response from exit — destination unreachable, refused, or blocked by the exit policy");
    }

    let (status_line, status_code, headers, body) = split_http_response(&raw);
    let content_type = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    let is_text = content_type.is_empty()
        || content_type.starts_with("text/")
        || content_type.contains("json")
        || content_type.contains("xml")
        || content_type.contains("javascript");
    let body_preview = if is_text {
        String::from_utf8_lossy(&body)
            .chars()
            .take(200_000)
            .collect::<String>()
    } else {
        format!(
            "[{} bytes of {}]",
            body.len(),
            if content_type.is_empty() {
                "binary data"
            } else {
                &content_type
            }
        )
    };

    return Ok(serde_json::json!({
        "ok": true,
        "url": url,
        "target": format!("{host}:{port}"),
        "backend": cfg.connector.backend(),
        "status_line": status_line,
        "status_code": status_code,
        "headers": headers.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>(),
        "content_type": content_type,
        "is_text": is_text,
        "body_len": body.len(),
        "body_preview": body_preview,
    }));
}

async fn read_all_capped<R>(r: &mut R, cap: usize) -> Result<Vec<u8>>
where
    R: AsyncReadExt + Unpin,
{
    let mut out = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    loop {
        let n = r.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
        if out.len() >= cap {
            break;
        }
    }
    return Ok(out);
}

/// Parse `http://host[:port]/path`. HTTPS is intentionally rejected here.
fn parse_http_url(url: &str) -> Result<(String, u16, String)> {
    let rest = if let Some(r) = url.strip_prefix("http://") {
        r
    } else if url.starts_with("https://") {
        bail!("the built-in fetch supports http:// only; for https point your browser/curl at the SOCKS proxy");
    } else {
        // Assume bare host means http.
        url
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if authority.is_empty() {
        bail!("could not parse host from URL");
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (
            h.to_string(),
            p.parse().map_err(|_| anyhow::anyhow!("bad port"))?,
        ),
        None => (authority.to_string(), 80u16),
    };
    let path = if path.is_empty() {
        "/".to_string()
    } else {
        path.to_string()
    };

    // The host and path are interpolated into a raw HTTP request; any control
    // character (notably CR/LF) would allow header injection / request
    // smuggling to the target through the proxy. A space in the path would
    // break the request line. Reject all of them.
    if host.is_empty() || host.bytes().any(|b| b.is_ascii_control() || b == b' ') {
        bail!("invalid host in URL");
    }
    if path.bytes().any(|b| b.is_ascii_control() || b == b' ') {
        bail!("invalid characters in URL path (control or space)");
    }
    return Ok((host, port, path));
}

fn split_http_response(raw: &[u8]) -> (String, u16, Vec<(String, String)>, Vec<u8>) {
    let sep = b"\r\n\r\n";
    let split_at = raw
        .windows(4)
        .position(|w| w == sep)
        .map(|i| i + 4)
        .unwrap_or(raw.len());
    let head = String::from_utf8_lossy(&raw[..split_at.min(raw.len())]);
    let body = if split_at <= raw.len() {
        raw[split_at..].to_vec()
    } else {
        Vec::new()
    };

    let mut lines = head.lines();
    let status_line = lines.next().unwrap_or("").trim().to_string();
    let status_code = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| {
            l.split_once(':')
                .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        })
        .collect();
    return (status_line, status_code, headers, body);
}

#[utoipa::path(
    get,
    path = "/docs",
    operation_id = "listTorDocumentation",
    tag = "documentation",
    responses((status = 200, description = "Markdown documentation index", body = String, content_type = "text/html"))
)]
async fn docs_index(State(cfg): State<AppState>) -> Html<String> {
    let mut names = list_docs(&cfg.docs_dir);
    names.sort();
    let markup = html! {
        (DOCTYPE)
        html {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width,initial-scale=1";
                title { "tor-server docs" }
                style { (PreEscaped(DOC_CSS)) }
            }
            body {
                h1 { "Documentation" }
                ul class="doclist" {
                    @if names.is_empty() { li { em { "no docs found" } } }
                    @for n in &names { li { a href={ "/docs/" (n) } { (n) } } }
                }
                p { a href="/" { "← Dashboard" } }
            }
        }
    };
    return Html(markup.into_string());
}

#[utoipa::path(
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
async fn docs_page(State(cfg): State<AppState>, AxPath(name): AxPath<String>) -> Response {
    // Sanitize: only a bare doc slug, no path traversal.
    if !is_doc_slug(&name) {
        return (StatusCode::BAD_REQUEST, "invalid doc name").into_response();
    }
    let file = cfg.docs_dir.join(format!("{name}.md"));
    let md = match std::fs::read_to_string(&file) {
        Ok(s) => s,
        Err(_) => return (StatusCode::NOT_FOUND, "doc not found").into_response(),
    };
    // Neutralize raw HTML embedded in a doc: pulldown-cmark passes it through
    // verbatim, and we emit the result PreEscaped, so a mounted/untrusted docs
    // volume could otherwise inject script into the dashboard origin. Map raw
    // HTML events to escaped text; genuine markdown still renders normally.
    let mut rendered = String::new();
    let opts =
        pulldown_cmark::Options::ENABLE_TABLES | pulldown_cmark::Options::ENABLE_STRIKETHROUGH;
    let parser = pulldown_cmark::Parser::new_ext(&md, opts).map(|event| match event {
        pulldown_cmark::Event::Html(html) => pulldown_cmark::Event::Text(html),
        pulldown_cmark::Event::InlineHtml(html) => pulldown_cmark::Event::Text(html),
        other => other,
    });
    pulldown_cmark::html::push_html(&mut rendered, parser);
    let markup = html! {
        (DOCTYPE)
        html {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width,initial-scale=1";
                title { "tor-server docs" }
                style { (PreEscaped(DOC_CSS)) }
            }
            body {
                p class="nav" {
                    a href="/docs" { "← All docs" } " · " a href="/" { "Dashboard" }
                }
                article { (PreEscaped(rendered)) }
            }
        }
    };
    return Html(markup.into_string()).into_response();
}

/// A safe documentation slug: a non-empty name of `[A-Za-z0-9_-]` only. This is
/// the single gate for both serving a doc and listing it, so no filename can
/// escape the docs directory (path traversal) or carry HTML into the index.
fn is_doc_slug(name: &str) -> bool {
    return !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
}

fn list_docs(dir: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) == Some("md") {
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    // Only list docs that are also serveable by `docs_page`;
                    // this keeps unescaped filenames out of the HTML index.
                    if is_doc_slug(stem) {
                        out.push(stem.to_string());
                    }
                }
            }
        }
    }
    return out;
}

#[utoipa::path(
    get,
    path = "/proxy.pac",
    operation_id = "getTorProxyAutoConfig",
    tag = "proxy",
    responses((status = 200, description = "Browser proxy auto-configuration", body = String, content_type = "application/x-ns-proxy-autoconfig"))
)]
async fn proxy_pac(State(cfg): State<AppState>) -> Response {
    // The client's SOCKS listener may bind 0.0.0.0; browsers need a real host.
    let hostport = cfg.socks_listen.replace("0.0.0.0", "127.0.0.1");
    let pac = format!(
        "function FindProxyForURL(url, host) {{\n  return \"SOCKS5 {hostport}; SOCKS {hostport}\";\n}}\n"
    );
    return (
        [(header::CONTENT_TYPE, "application/x-ns-proxy-autoconfig")],
        pac,
    )
        .into_response();
}

/// Serve vendored front-end assets from the binary. A strict allowlist (no path
/// component) means this cannot read anything but the files baked in here.
#[utoipa::path(
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
async fn vendor(AxPath(file): AxPath<String>) -> Response {
    return match file.as_str() {
        "htmx.min.js" => (
            [(
                header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            )],
            HTMX_JS,
        )
            .into_response(),
        _ => (StatusCode::NOT_FOUND, "not found").into_response(),
    };
}

const HTMX_JS: &str = include_str!("web/vendor/htmx.min.js");

const DASH_CSS: &str = r#"
  :root { color-scheme: dark; }
  * { box-sizing: border-box; }
  body { background:#0d1117; color:#c9d1d9; font:15px/1.55 -apple-system,Segoe UI,Roboto,Helvetica,sans-serif; margin:0; }
  header { padding:1.5rem 2rem; border-bottom:1px solid #21262d; display:flex; align-items:center; gap:.8rem; }
  header h1 { font-size:1.25rem; margin:0; color:#e6edf3; }
  .badge { font-size:.7rem; background:#1f6feb33; color:#58a6ff; border:1px solid #1f6feb55; padding:.15rem .5rem; border-radius:999px; }
  main { max-width:1000px; margin:0 auto; padding:2rem; }
  a { color:#58a6ff; }
  .grid { display:grid; grid-template-columns:repeat(auto-fit,minmax(150px,1fr)); gap:1rem; margin-bottom:2rem; }
  .card { background:#161b22; border:1px solid #21262d; border-radius:10px; padding:1rem; }
  .card .k { font-size:.75rem; color:#8b949e; text-transform:uppercase; letter-spacing:.04em; }
  .card .v { font-size:1.6rem; color:#e6edf3; font-variant-numeric:tabular-nums; margin-top:.25rem; }
  section { background:#161b22; border:1px solid #21262d; border-radius:10px; padding:1.25rem 1.5rem; margin-bottom:1.5rem; }
  h2 { font-size:1rem; color:#e6edf3; margin:0 0 1rem; }
  input[type=text] { width:100%; padding:.6rem .7rem; background:#0d1117; border:1px solid #30363d; border-radius:8px; color:#e6edf3; font:inherit; }
  .row { display:flex; gap:.6rem; }
  button { padding:.6rem 1.1rem; background:#238636; border:1px solid #2ea04366; color:#fff; border-radius:8px; font:inherit; cursor:pointer; }
  button:hover { background:#2ea043; }
  button.secondary { background:#21262d; border-color:#30363d; }
  pre { background:#0d1117; border:1px solid #21262d; border-radius:8px; padding:1rem; overflow:auto; max-height:420px; white-space:pre-wrap; word-break:break-word; }
  .muted { color:#8b949e; font-size:.85rem; }
  .pill { display:inline-block; background:#0d1117; border:1px solid #30363d; border-radius:999px; padding:.1rem .6rem; margin:.15rem; font-size:.8rem; }
  .examples { margin-top:.6rem; }
  .examples button { background:#21262d; border-color:#30363d; padding:.35rem .7rem; font-size:.8rem; margin-right:.4rem; }
  code { background:#0d1117; padding:.1em .4em; border-radius:4px; }
  .ok { color:#3fb950; } .err { color:#f85149; }
  iframe { width:100%; height:340px; border:1px solid #21262d; border-radius:8px; background:#fff; margin-top:.8rem; }
"#;

const DOC_CSS: &str = r#"
  body{background:#0d1117;color:#c9d1d9;font:16px/1.6 -apple-system,Segoe UI,Roboto,sans-serif;max-width:820px;margin:0 auto;padding:2rem}
  a{color:#58a6ff}h1,h2,h3{color:#e6edf3}code{background:#161b22;padding:.15em .4em;border-radius:4px}
  pre{background:#161b22;padding:1rem;border-radius:8px;overflow:auto}pre code{background:none;padding:0}
  table{border-collapse:collapse}th,td{border:1px solid #30363d;padding:.4rem .7rem}.doclist li{margin:.3rem 0}.nav{color:#8b949e}
"#;

/// Live-stats WebSocket client. Updates the grid via textContent (no innerHTML,
/// so directory-supplied names cannot inject markup), reconnects on drop, and
/// forwards a dashboard `?token=` to the guarded `/api/fetch` via htmx.
const WS_SCRIPT: &str = r#"
(function () {
  function set(id, v) { var e = document.getElementById(id); if (e != null && v != null) e.textContent = v; }
  function connect() {
    var ws;
    try {
      var tok = new URLSearchParams(location.search).get('token');
      var base = (location.protocol === 'https:' ? 'wss://' : 'ws://') + location.host + '/ws/stats';
      ws = new WebSocket(tok ? base + '?token=' + encodeURIComponent(tok) : base);
    }
    catch (e) { return; }
    ws.onmessage = function (ev) {
      var s; try { s = JSON.parse(ev.data); } catch (e) { return; }
      set('s-built', s.circuits_built); set('s-active', s.circuits_active); set('s-failed', s.circuits_failed);
      set('s-hops', s.hops); set('s-relays', s.relay_count);
      var b = document.getElementById('backend-badge'); if (b && s.backend) b.textContent = s.backend + ' backend';
      var sa = document.getElementById('socks-addr'); if (sa && s.socks_listen) sa.textContent = s.socks_listen;
      document.querySelectorAll('.socks').forEach(function (e) { if (s.socks_listen) e.textContent = s.socks_listen.replace('0.0.0.0', '127.0.0.1'); });
      var r = document.getElementById('relays');
      if (r && Array.isArray(s.relays)) {
        r.textContent = '';
        s.relays.forEach(function (x) {
          var span = document.createElement('span');
          span.className = 'pill'; span.setAttribute('data-testid', 'relay-pill');
          span.textContent = x.name + ' · ' + x.addr;
          r.appendChild(span);
        });
      }
    };
    ws.onclose = function () { setTimeout(connect, 3000); };
  }
  connect();
  document.body.addEventListener('htmx:configRequest', function (e) {
    var t = new URLSearchParams(location.search).get('token');
    if (t) e.detail.headers['Authorization'] = 'Bearer ' + t;
  });
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_http_url_accepts_valid() {
        let (h, p, path) = parse_http_url("http://example.com/foo?bar=1").unwrap();
        assert_eq!(h, "example.com");
        assert_eq!(p, 80);
        assert_eq!(path, "/foo?bar=1");
        let (h, p, _) = parse_http_url("http://example.com:8080/").unwrap();
        assert_eq!((h.as_str(), p), ("example.com", 8080));
    }

    #[test]
    fn parse_http_url_rejects_crlf_injection() {
        assert!(parse_http_url("http://example.com/x\r\nX-Injected: 1").is_err());
        assert!(parse_http_url("http://exa\r\nmple.com/").is_err());
        assert!(parse_http_url("http://example.com/a b").is_err());
        assert!(parse_http_url("https://example.com/").is_err());
    }

    #[test]
    fn ct_str_eq_matches_and_differs() {
        assert!(ct_str_eq("s3cret", "s3cret"));
        assert!(!ct_str_eq("s3cret", "s3creT"));
        assert!(!ct_str_eq("s3cret", "s3cret-longer"));
        assert!(!ct_str_eq("", "x"));
    }

    #[test]
    fn doc_slug_rejects_traversal_and_markup() {
        assert!(is_doc_slug("security"));
        assert!(is_doc_slug("tor-interop"));
        assert!(is_doc_slug("cloud_vpn_obfuscation"));
        assert!(!is_doc_slug(""));
        assert!(!is_doc_slug("../secret"));
        assert!(!is_doc_slug("a/b"));
        assert!(!is_doc_slug("<img src=x>"));
        assert!(!is_doc_slug("a.b"));
    }

    #[test]
    fn same_origin_guard_blocks_cross_site_ws() {
        use axum::http::HeaderValue;
        let mut h = HeaderMap::new();
        // No Origin (curl/websocat): allowed.
        assert!(same_origin(&h));
        // Matching Origin/Host: allowed.
        h.insert(header::HOST, HeaderValue::from_static("127.0.0.1:9060"));
        h.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://127.0.0.1:9060"),
        );
        assert!(same_origin(&h));
        // Cross-site Origin: rejected.
        h.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://evil.example"),
        );
        assert!(!same_origin(&h));
    }

    #[test]
    fn fetch_ok_fragment_escapes_and_carries_status() {
        // A malicious status line must be escaped, and the status text must be
        // present for the dashboard/e2e to read.
        let v = serde_json::json!({
            "status_line": "HTTP/1.1 200 <script>alert(1)</script>",
            "backend": "overlay", "body_len": 10, "elapsed_ms": 5,
            "headers": ["Content-Type: text/html"], "body_preview": "<b>hi</b>",
            "content_type": "text/html", "is_text": true,
        });
        let out = render_fetch_ok(&v).into_string();
        assert!(out.contains("HTTP/1.1 200"));
        assert!(!out.contains("<script>alert(1)</script>"));
        assert!(out.contains("&lt;script&gt;"));
    }
}
