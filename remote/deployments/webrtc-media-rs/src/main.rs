use std::env;
use std::error::Error;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::TimeoutLayer;

/// Maximum time a single request handler is allowed to run before the server
/// returns 408, bounding slow-client / resource-exhaustion exposure.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Hard cap on request body size. All routes are GET today, so this is purely
/// defense-in-depth against unexpected large payloads.
const MAX_BODY_BYTES: usize = 64 * 1024;

const DEFAULT_PORT: u16 = 8125;
const SERVICE_NAME: &str = "dd-webrtc-media";

#[derive(Clone)]
struct AppState {
    config: Arc<MediaConfig>,
    metrics: Arc<Metrics>,
}

#[derive(Default)]
struct Metrics {
    requests_total: AtomicU64,
    config_requests_total: AtomicU64,
    ice_requests_total: AtomicU64,
    errors_total: AtomicU64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Capabilities {
    turn: bool,
    sfu: bool,
    media_relay: bool,
}

#[derive(Clone)]
struct MediaConfig {
    service: String,
    mode: String,
    capabilities: Capabilities,
    stun_urls: Vec<String>,
    turn_urls: Vec<String>,
    turn_username: Option<String>,
    turn_credential: Option<String>,
    sfu_endpoint: Option<String>,
    media_relay_endpoint: Option<String>,
    public_host: Option<String>,
    udp_port_range: Option<String>,
    turn_udp_port: Option<u16>,
    turns_tls_port: Option<u16>,
    warnings: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HealthResponse {
    ok: bool,
    service: String,
    mode: String,
    ready: bool,
    capabilities: Capabilities,
    warnings: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfigResponse {
    service: String,
    mode: String,
    ready: bool,
    capabilities: Capabilities,
    stun_urls: Vec<String>,
    turn_urls: Vec<String>,
    turn_username_configured: bool,
    turn_credential_configured: bool,
    sfu_endpoint: Option<String>,
    media_relay_endpoint: Option<String>,
    public_host: Option<String>,
    udp_port_range: Option<String>,
    turn_udp_port: Option<u16>,
    turns_tls_port: Option<u16>,
    warnings: Vec<String>,
    note: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct IceConfigResponse {
    service: String,
    mode: String,
    ready: bool,
    ice_servers: Vec<IceServer>,
    capabilities: Capabilities,
    sfu_endpoint: Option<String>,
    media_relay_endpoint: Option<String>,
    warnings: Vec<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct IceServer {
    urls: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    credential: Option<String>,
}

static ROOT_HTML: &str = r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>dd WebRTC media</title>
    <style>
      body { font-family: system-ui, sans-serif; margin: 2rem; color: #17202a; background: #f7f8fa; }
      main { max-width: 820px; }
      code { background: #eef2f6; padding: 0.1rem 0.25rem; border-radius: 4px; }
      a { color: #165a72; }
    </style>
  </head>
  <body>
    <main>
      <h1>dd WebRTC media</h1>
      <p>Optional WebRTC media-plane configuration service. It advertises ICE, TURN, SFU, and media-relay capability metadata for clients and signaling services.</p>
      <p>This process does not relay UDP media by itself. TURN/SFU/media modes require a backing data-plane service with public UDP/TCP networking.</p>
      <p>
        <a href="/webrtc-media/healthz"><code>/webrtc-media/healthz</code></a>
        <a href="/webrtc-media/config"><code>/webrtc-media/config</code></a>
        <a href="/webrtc-media/ice"><code>/webrtc-media/ice</code></a>
        <a href="/webrtc-media/metrics"><code>/webrtc-media/metrics</code></a>
      </p>
    </main>
  </body>
</html>"#;

fn env_value(name: &str, default: &str) -> String {
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn env_optional(name: &str) -> Option<String> {
    env::var(name).ok().and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn env_list(name: &str, default: &[&str]) -> Vec<String> {
    let raw = env::var(name).ok();
    let values: Vec<String> = raw
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    if values.is_empty() {
        default.iter().map(|value| (*value).to_string()).collect()
    } else {
        values
    }
}

fn env_u16(name: &str) -> Option<u16> {
    env_optional(name).and_then(|value| value.parse::<u16>().ok())
}

fn parse_capabilities(mode: &str) -> (Capabilities, Vec<String>) {
    let mut capabilities = Capabilities {
        turn: false,
        sfu: false,
        media_relay: false,
    };
    let mut warnings = Vec::new();
    let normalized = mode.trim().to_ascii_lowercase();
    if normalized.is_empty() || matches!(normalized.as_str(), "disabled" | "off" | "none") {
        return (capabilities, warnings);
    }
    for token in normalized
        .split(|ch| matches!(ch, ',' | '+' | '/' | ' '))
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        match token {
            "disabled" | "off" | "none" | "stun" => {}
            "turn" => capabilities.turn = true,
            "sfu" => capabilities.sfu = true,
            "media" | "relay" | "media-relay" | "media_relay" => {
                capabilities.media_relay = true
            }
            "all" | "full" => {
                capabilities.turn = true;
                capabilities.sfu = true;
                capabilities.media_relay = true;
            }
            other => warnings.push(format!(
                "unknown WEBRTC_MEDIA_MODE token '{other}'; expected disabled, stun, turn, sfu, media, or all"
            )),
        }
    }
    (capabilities, warnings)
}

fn build_config() -> MediaConfig {
    let mode = env_value("WEBRTC_MEDIA_MODE", "disabled");
    let (capabilities, mut warnings) = parse_capabilities(&mode);
    let stun_urls = env_list("WEBRTC_STUN_URLS", &["stun:stun.l.google.com:19302"]);
    let turn_urls = env_list("WEBRTC_TURN_URLS", &[]);
    let turn_username = env_optional("WEBRTC_TURN_USERNAME");
    let turn_credential = env_optional("WEBRTC_TURN_CREDENTIAL");
    let sfu_endpoint = env_optional("WEBRTC_SFU_ENDPOINT");
    let media_relay_endpoint = env_optional("WEBRTC_MEDIA_RELAY_ENDPOINT");

    if capabilities.turn {
        if turn_urls.is_empty() {
            warnings.push("TURN mode requires WEBRTC_TURN_URLS".to_string());
        }
        if turn_username.is_none() {
            warnings.push("TURN mode requires WEBRTC_TURN_USERNAME".to_string());
        }
        if turn_credential.is_none() {
            warnings.push("TURN mode requires WEBRTC_TURN_CREDENTIAL".to_string());
        }
    }
    if capabilities.sfu && sfu_endpoint.is_none() {
        warnings.push("SFU mode requires WEBRTC_SFU_ENDPOINT".to_string());
    }
    if capabilities.media_relay && media_relay_endpoint.is_none() {
        warnings.push("media relay mode requires WEBRTC_MEDIA_RELAY_ENDPOINT".to_string());
    }

    MediaConfig {
        service: SERVICE_NAME.to_string(),
        mode,
        capabilities,
        stun_urls,
        turn_urls,
        turn_username,
        turn_credential,
        sfu_endpoint,
        media_relay_endpoint,
        public_host: env_optional("WEBRTC_PUBLIC_HOST"),
        udp_port_range: env_optional("WEBRTC_UDP_PORT_RANGE"),
        turn_udp_port: env_u16("WEBRTC_TURN_UDP_PORT"),
        turns_tls_port: env_u16("WEBRTC_TURNS_TLS_PORT"),
        warnings,
    }
}

fn is_ready(config: &MediaConfig) -> bool {
    config.warnings.is_empty()
}

fn health_response(config: &MediaConfig) -> HealthResponse {
    HealthResponse {
        ok: is_ready(config),
        service: config.service.clone(),
        mode: config.mode.clone(),
        ready: is_ready(config),
        capabilities: config.capabilities.clone(),
        warnings: config.warnings.clone(),
    }
}

fn config_response(config: &MediaConfig) -> ConfigResponse {
    ConfigResponse {
        service: config.service.clone(),
        mode: config.mode.clone(),
        ready: is_ready(config),
        capabilities: config.capabilities.clone(),
        stun_urls: config.stun_urls.clone(),
        turn_urls: config.turn_urls.clone(),
        turn_username_configured: config.turn_username.is_some(),
        turn_credential_configured: config.turn_credential.is_some(),
        sfu_endpoint: config.sfu_endpoint.clone(),
        media_relay_endpoint: config.media_relay_endpoint.clone(),
        public_host: config.public_host.clone(),
        udp_port_range: config.udp_port_range.clone(),
        turn_udp_port: config.turn_udp_port,
        turns_tls_port: config.turns_tls_port,
        warnings: config.warnings.clone(),
        note: "This service advertises media-plane configuration; TURN/SFU/media traffic needs a backing data-plane deployment.",
    }
}

fn ice_response(config: &MediaConfig) -> IceConfigResponse {
    let mut ice_servers = Vec::new();
    if !config.stun_urls.is_empty() {
        ice_servers.push(IceServer {
            urls: config.stun_urls.clone(),
            username: None,
            credential: None,
        });
    }
    if config.capabilities.turn && !config.turn_urls.is_empty() {
        ice_servers.push(IceServer {
            urls: config.turn_urls.clone(),
            username: config.turn_username.clone(),
            credential: config.turn_credential.clone(),
        });
    }
    IceConfigResponse {
        service: config.service.clone(),
        mode: config.mode.clone(),
        ready: is_ready(config),
        ice_servers,
        capabilities: config.capabilities.clone(),
        sfu_endpoint: config.sfu_endpoint.clone(),
        media_relay_endpoint: config.media_relay_endpoint.clone(),
        warnings: config.warnings.clone(),
    }
}

fn record_request(state: &AppState) {
    state.metrics.requests_total.fetch_add(1, Ordering::Relaxed);
}

async fn root(State(state): State<AppState>) -> Html<&'static str> {
    record_request(&state);
    Html(ROOT_HTML)
}

async fn healthz(State(state): State<AppState>) -> Response {
    record_request(&state);
    let health = health_response(&state.config);
    let status = if health.ready {
        StatusCode::OK
    } else {
        state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(health)).into_response()
}

async fn media_config(State(state): State<AppState>) -> impl IntoResponse {
    record_request(&state);
    state
        .metrics
        .config_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(config_response(&state.config))
}

async fn capabilities(State(state): State<AppState>) -> impl IntoResponse {
    record_request(&state);
    Json(state.config.capabilities.clone())
}

async fn ice(State(state): State<AppState>) -> impl IntoResponse {
    record_request(&state);
    state
        .metrics
        .ice_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(ice_response(&state.config))
}

/// Escape a string for safe use as a Prometheus exposition label value.
/// Per the text exposition format, backslash, double-quote, and newline must be
/// escaped; otherwise an operator-supplied `mode` containing these characters
/// would produce malformed (or injectable) metrics output.
fn escape_metric_label(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    record_request(&state);
    let ready = u8::from(is_ready(&state.config));
    let turn = u8::from(state.config.capabilities.turn);
    let sfu = u8::from(state.config.capabilities.sfu);
    let media_relay = u8::from(state.config.capabilities.media_relay);
    let body = format!(
        "# HELP dd_webrtc_media_http_requests_total HTTP requests observed by the WebRTC media config service.\n\
         # TYPE dd_webrtc_media_http_requests_total counter\n\
         dd_webrtc_media_http_requests_total {}\n\
         # HELP dd_webrtc_media_config_requests_total Media config endpoint requests.\n\
         # TYPE dd_webrtc_media_config_requests_total counter\n\
         dd_webrtc_media_config_requests_total {}\n\
         # HELP dd_webrtc_media_ice_requests_total ICE config endpoint requests.\n\
         # TYPE dd_webrtc_media_ice_requests_total counter\n\
         dd_webrtc_media_ice_requests_total {}\n\
         # HELP dd_webrtc_media_errors_total Misconfiguration or handler errors observed by the service.\n\
         # TYPE dd_webrtc_media_errors_total counter\n\
         dd_webrtc_media_errors_total {}\n\
         # HELP dd_webrtc_media_ready Whether the configured media-plane mode is ready.\n\
         # TYPE dd_webrtc_media_ready gauge\n\
         dd_webrtc_media_ready{{mode=\"{}\"}} {}\n\
         # HELP dd_webrtc_media_capability_enabled Enabled media-plane capabilities by kind.\n\
         # TYPE dd_webrtc_media_capability_enabled gauge\n\
         dd_webrtc_media_capability_enabled{{kind=\"turn\"}} {}\n\
         dd_webrtc_media_capability_enabled{{kind=\"sfu\"}} {}\n\
         dd_webrtc_media_capability_enabled{{kind=\"media_relay\"}} {}\n",
        state.metrics.requests_total.load(Ordering::Relaxed),
        state.metrics.config_requests_total.load(Ordering::Relaxed),
        state.metrics.ice_requests_total.load(Ordering::Relaxed),
        state.metrics.errors_total.load(Ordering::Relaxed),
        escape_metric_label(&state.config.mode),
        ready,
        turn,
        sfu,
        media_relay,
    );
    ([("content-type", "text/plain; version=0.0.4")], body)
}

async fn api_docs_html() -> Html<&'static str> {
    Html(include_str!("../generated/api-docs.html"))
}

async fn api_docs_json() -> impl IntoResponse {
    (
        [("content-type", "application/json; charset=utf-8")],
        include_str!("../generated/api-docs.json"),
    )
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let host = env_value("HOST", "0.0.0.0");
    let port = env_value("PORT", &DEFAULT_PORT.to_string()).parse::<u16>()?;
    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    let config = Arc::new(build_config());
    let state = AppState {
        config: config.clone(),
        metrics: Arc::new(Metrics::default()),
    };

    let app = Router::new()
        .route("/", get(root))
        .route("/webrtc-media", get(root))
        .route("/webrtc-media/", get(root))
        .route("/healthz", get(healthz))
        .route("/readyz", get(healthz))
        .route("/webrtc-media/healthz", get(healthz))
        .route("/webrtc-media/readyz", get(healthz))
        .route("/metrics", get(metrics))
        .route("/webrtc-media/metrics", get(metrics))
        .route("/config", get(media_config))
        .route("/webrtc-media/config", get(media_config))
        .route("/capabilities", get(capabilities))
        .route("/webrtc-media/capabilities", get(capabilities))
        .route("/ice", get(ice))
        .route("/webrtc-media/ice", get(ice))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .with_state(state)
        .merge(dd_runtime_config_client::router())
        // Hardening middleware applied to every route, including the merged
        // runtime-config endpoints.
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            REQUEST_TIMEOUT,
        ))
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
        // Credential/topology-bearing responses (e.g. /ice, /config) must not be
        // cached by browsers or intermediaries.
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ));

    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    println!(
        "{SERVICE_NAME} listening on http://{addr} mode={} ready={}",
        config.mode,
        is_ready(&config)
    );
    if !config.warnings.is_empty() {
        eprintln!("{SERVICE_NAME} configuration warnings: {:?}", config.warnings);
    }
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
