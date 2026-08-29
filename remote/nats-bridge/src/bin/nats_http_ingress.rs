//! Hardened external HTTP -> JetStream queue ingress.
//!
//! External callers authenticate as a configured client and POST JSON to a
//! named queue route. They never choose or receive NATS subjects, stream names,
//! server URLs, credentials, or raw upstream errors.

use async_nats::{HeaderMap as NatsHeaders, HeaderValue};
use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Path, Request, State},
    http::{HeaderMap as HttpHeaders, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    fs,
    net::SocketAddr,
    path::{Path as FsPath, PathBuf},
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::Semaphore;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

const DEFAULT_PORT: u16 = 3004;
const DEFAULT_MAX_BODY_BYTES: usize = 256 * 1024;
const ABSOLUTE_MAX_BODY_BYTES: usize = 1024 * 1024;
const DEFAULT_MAX_IN_FLIGHT: usize = 128;
const DEFAULT_PUBLISH_TIMEOUT_MS: u64 = 5_000;
const MIN_TOKEN_BYTES: usize = 32;
const MAX_TOKEN_BYTES: usize = 4096;
const MIN_IDEMPOTENCY_BYTES: usize = 8;
const MAX_IDEMPOTENCY_BYTES: usize = 128;
const MAX_IDENTIFIER_BYTES: usize = 32;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RouteConfig {
    subject: String,
    stream: String,
    #[serde(default = "default_route_max_body")]
    max_body_bytes: usize,
}

fn default_route_max_body() -> usize {
    DEFAULT_MAX_BODY_BYTES
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClientConfig {
    token: String,
    routes: BTreeSet<String>,
}

type Routes = BTreeMap<String, RouteConfig>;
type Clients = BTreeMap<String, ClientConfig>;

struct Counters {
    accepted: AtomicU64,
    rejected: AtomicU64,
    overloaded: AtomicU64,
    upstream_failed: AtomicU64,
}

struct AppState {
    nats: async_nats::Client,
    jetstream: async_nats::jetstream::Context,
    routes: Routes,
    clients: Clients,
    in_flight: Arc<Semaphore>,
    max_in_flight: usize,
    publish_timeout: Duration,
    counters: Counters,
}

#[derive(Debug, Serialize)]
struct ApiErrorBody {
    ok: bool,
    code: &'static str,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    code: &'static str,
}

impl ApiError {
    const fn new(status: StatusCode, code: &'static str) -> Self {
        Self { status, code }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ApiErrorBody {
                ok: false,
                code: self.code,
            }),
        )
            .into_response()
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "nats_http_ingress=info,info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let routes_path = env_path("BRIDGE_ROUTES_FILE", "/etc/nats-bridge/routes.json");
    let clients_path = env_path(
        "BRIDGE_CLIENTS_FILE",
        "/var/run/secrets/nats-bridge/clients.json",
    );
    let routes: Routes = load_json_file(&routes_path, "route configuration");
    let clients: Clients = load_json_file(&clients_path, "client configuration");
    if let Err(error) = validate_configuration(&routes, &clients) {
        fatal(&format!("invalid bridge configuration: {error}"));
    }

    let nats_url =
        std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".to_string());
    let mut connect_options = async_nats::ConnectOptions::new()
        .name("dd-nats-http-ingress")
        .retry_on_initial_connect()
        .max_reconnects(None);
    if let Ok(token) = std::env::var("NATS_TOKEN") {
        if !token.trim().is_empty() {
            connect_options = connect_options.token(token.trim().to_string());
        }
    } else if let (Ok(user), Ok(password)) =
        (std::env::var("NATS_USER"), std::env::var("NATS_PASSWORD"))
    {
        connect_options = connect_options.user_and_password(user, password);
    }

    let nats = connect_options
        .connect(&nats_url)
        .await
        .unwrap_or_else(|_| fatal("could not connect to internal NATS"));
    let jetstream = async_nats::jetstream::new(nats.clone());
    let max_in_flight = env_usize("BRIDGE_MAX_IN_FLIGHT", DEFAULT_MAX_IN_FLIGHT, 1, 4096);
    let timeout_ms = env_u64(
        "BRIDGE_PUBLISH_TIMEOUT_MS",
        DEFAULT_PUBLISH_TIMEOUT_MS,
        100,
        60_000,
    );

    let state = Arc::new(AppState {
        nats,
        jetstream,
        routes,
        clients,
        in_flight: Arc::new(Semaphore::new(max_in_flight)),
        max_in_flight,
        publish_timeout: Duration::from_millis(timeout_ms),
        counters: Counters {
            accepted: AtomicU64::new(0),
            rejected: AtomicU64::new(0),
            overloaded: AtomicU64::new(0),
            upstream_failed: AtomicU64::new(0),
        },
    });

    tracing::info!(
        route_count = state.routes.len(),
        client_count = state.clients.len(),
        max_in_flight,
        timeout_ms,
        "external queue ingress configured"
    );

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
        .route("/v1/queues/:route", post(enqueue))
        .with_state(state);

    let port = env_u16("PORT", DEFAULT_PORT);
    let address = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .unwrap_or_else(|error| fatal(&format!("could not bind HTTP listener: {error}")));
    tracing::info!(port, "nats HTTP ingress listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .unwrap_or_else(|error| fatal(&format!("HTTP server failed: {error}")));
}

async fn healthz(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(json!({
        "ok": true,
        "nats_connected": nats_connected(&state),
    }))
}

async fn readyz(State(state): State<Arc<AppState>>) -> Result<&'static str, StatusCode> {
    if nats_connected(&state) {
        Ok("ok")
    } else {
        Err(StatusCode::SERVICE_UNAVAILABLE)
    }
}

fn nats_connected(state: &AppState) -> bool {
    matches!(
        state.nats.connection_state(),
        async_nats::connection::State::Connected
    )
}

async fn metrics(State(state): State<Arc<AppState>>) -> Response {
    let in_flight = state
        .max_in_flight
        .saturating_sub(state.in_flight.available_permits());
    let body = format!(
        concat!(
            "# TYPE nats_http_ingress_accepted_total counter\n",
            "nats_http_ingress_accepted_total {}\n",
            "# TYPE nats_http_ingress_rejected_total counter\n",
            "nats_http_ingress_rejected_total {}\n",
            "# TYPE nats_http_ingress_overloaded_total counter\n",
            "nats_http_ingress_overloaded_total {}\n",
            "# TYPE nats_http_ingress_upstream_failed_total counter\n",
            "nats_http_ingress_upstream_failed_total {}\n",
            "# TYPE nats_http_ingress_in_flight gauge\n",
            "nats_http_ingress_in_flight {}\n"
        ),
        state.counters.accepted.load(Ordering::Relaxed),
        state.counters.rejected.load(Ordering::Relaxed),
        state.counters.overloaded.load(Ordering::Relaxed),
        state.counters.upstream_failed.load(Ordering::Relaxed),
        in_flight,
    );
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        body,
    )
        .into_response()
}

async fn enqueue(
    Path(route_name): Path<String>,
    State(state): State<Arc<AppState>>,
    request: Request,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let result = enqueue_inner(&route_name, &state, request).await;
    match result {
        Ok(public_message_id) => {
            state.counters.accepted.fetch_add(1, Ordering::Relaxed);
            tracing::info!(route = %route_name, "queue message accepted");
            Ok((
                StatusCode::ACCEPTED,
                Json(json!({
                    "ok": true,
                    "route": route_name,
                    "message_id": public_message_id,
                })),
            ))
        }
        Err(error) => {
            state.counters.rejected.fetch_add(1, Ordering::Relaxed);
            if error.code == "overloaded" {
                state.counters.overloaded.fetch_add(1, Ordering::Relaxed);
            }
            if matches!(error.code, "publish_failed" | "publish_timeout") {
                state
                    .counters
                    .upstream_failed
                    .fetch_add(1, Ordering::Relaxed);
            }
            tracing::warn!(route = %route_name, code = error.code, "queue message rejected");
            Err(error)
        }
    }
}

async fn enqueue_inner(
    route_name: &str,
    state: &Arc<AppState>,
    request: Request,
) -> Result<String, ApiError> {
    let (parts, body) = request.into_parts();
    let (client_id, client) = authenticate_client(&parts.headers, &state.clients)?;
    if !valid_identifier(route_name) {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "route_not_found"));
    }
    let route = state
        .routes
        .get(route_name)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "route_not_found"))?;
    if !client.routes.contains(route_name) {
        return Err(ApiError::new(StatusCode::FORBIDDEN, "route_forbidden"));
    }
    if !json_content_type(&parts.headers) {
        return Err(ApiError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "content_type_required",
        ));
    }

    let idempotency_key = required_header(
        &parts.headers,
        "idempotency-key",
        StatusCode::BAD_REQUEST,
        "missing_idempotency_key",
    )?;
    validate_idempotency_key(idempotency_key)?;
    let body = to_bytes(body, route.max_body_bytes)
        .await
        .map_err(|_| ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "body_too_large"))?;
    if body.is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "empty_body"));
    }
    if !matches!(serde_json::from_slice::<Value>(&body), Ok(Value::Object(_))) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_json_object",
        ));
    }

    let _permit = state
        .in_flight
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::new(StatusCode::TOO_MANY_REQUESTS, "overloaded"))?;
    let internal_message_id = format!("{client_id}:{route_name}:{idempotency_key}");
    let mut headers = NatsHeaders::new();
    insert_nats_header(&mut headers, "Nats-Msg-Id", &internal_message_id)?;
    insert_nats_header(&mut headers, "Nats-Expected-Stream", &route.stream)?;
    insert_nats_header(&mut headers, "X-Bridge-Client", client_id)?;
    insert_nats_header(&mut headers, "X-Bridge-Route", route_name)?;

    let publish = async {
        let acknowledgement = state
            .jetstream
            .publish_with_headers(route.subject.clone(), headers, body)
            .await
            .map_err(|_| ApiError::new(StatusCode::BAD_GATEWAY, "publish_failed"))?;
        acknowledgement
            .await
            .map_err(|_| ApiError::new(StatusCode::BAD_GATEWAY, "publish_failed"))?;
        Ok::<(), ApiError>(())
    };
    tokio::time::timeout(state.publish_timeout, publish)
        .await
        .map_err(|_| ApiError::new(StatusCode::GATEWAY_TIMEOUT, "publish_timeout"))??;

    Ok(idempotency_key.to_string())
}

fn authenticate_client<'a>(
    headers: &'a HttpHeaders,
    clients: &'a Clients,
) -> Result<(&'a str, &'a ClientConfig), ApiError> {
    let client_id = required_header(
        headers,
        "x-bridge-client",
        StatusCode::UNAUTHORIZED,
        "unauthorized",
    )?;
    if !valid_identifier(client_id) {
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    let client = clients
        .get(client_id)
        .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "unauthorized"))?;
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "unauthorized"))?;
    if !constant_time_eq(token, client.token.as_str()) {
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    Ok((client_id, client))
}

fn required_header<'a>(
    headers: &'a HttpHeaders,
    name: &str,
    status: StatusCode,
    code: &'static str,
) -> Result<&'a str, ApiError> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::new(status, code))
}

fn json_content_type(headers: &HttpHeaders) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"))
}

fn insert_nats_header(
    headers: &mut NatsHeaders,
    name: &'static str,
    value: &str,
) -> Result<(), ApiError> {
    let value = HeaderValue::from_str(value)
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "configuration_error"))?;
    headers.insert(name, value);
    Ok(())
}

fn validate_configuration(routes: &Routes, clients: &Clients) -> Result<(), String> {
    if routes.is_empty() {
        return Err("at least one route is required".into());
    }
    if clients.is_empty() {
        return Err("at least one client is required".into());
    }

    for (name, route) in routes {
        if !valid_identifier(name) {
            return Err(format!("route name {name:?} is invalid"));
        }
        validate_subject(&route.subject)?;
        validate_stream(&route.stream)?;
        if !(1..=ABSOLUTE_MAX_BODY_BYTES).contains(&route.max_body_bytes) {
            return Err(format!(
                "route {name:?} max_body_bytes must be 1-{ABSOLUTE_MAX_BODY_BYTES}"
            ));
        }
    }

    let mut unique_tokens = HashSet::new();
    for (name, client) in clients {
        if !valid_identifier(name) {
            return Err(format!("client name {name:?} is invalid"));
        }
        if !(MIN_TOKEN_BYTES..=MAX_TOKEN_BYTES).contains(&client.token.len()) {
            return Err(format!(
                "client {name:?} token must be {MIN_TOKEN_BYTES}-{MAX_TOKEN_BYTES} bytes"
            ));
        }
        if !unique_tokens.insert(client.token.as_str()) {
            return Err("client tokens must be unique".into());
        }
        if client.routes.is_empty() {
            return Err(format!("client {name:?} must have at least one route"));
        }
        for route_name in &client.routes {
            if !routes.contains_key(route_name) {
                return Err(format!(
                    "client {name:?} references unknown route {route_name:?}"
                ));
            }
        }
    }
    Ok(())
}

fn validate_subject(subject: &str) -> Result<(), String> {
    if subject.is_empty() || subject.len() > 255 || subject.starts_with('$') {
        return Err("route subject must be a non-system exact subject".into());
    }
    for token in subject.split('.') {
        if token.is_empty()
            || token == "*"
            || token == ">"
            || !token.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
            })
        {
            return Err("route subject contains an invalid token".into());
        }
    }
    Ok(())
}

fn validate_stream(stream: &str) -> Result<(), String> {
    if stream.is_empty()
        || stream.len() > 128
        || !stream
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        return Err("expected stream name is invalid".into());
    }
    Ok(())
}

fn validate_idempotency_key(key: &str) -> Result<(), ApiError> {
    if !(MIN_IDEMPOTENCY_BYTES..=MAX_IDEMPOTENCY_BYTES).contains(&key.len())
        || !key.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':')
        })
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_idempotency_key",
        ));
    }
    Ok(())
}

fn valid_identifier(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_IDENTIFIER_BYTES
        && bytes[0].is_ascii_lowercase()
        && bytes.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

fn load_json_file<T: for<'de> Deserialize<'de>>(path: &FsPath, label: &str) -> T {
    let contents = fs::read_to_string(path).unwrap_or_else(|error| {
        fatal(&format!(
            "could not read {label} at {}: {error}",
            path.display()
        ))
    });
    serde_json::from_str(&contents)
        .unwrap_or_else(|error| fatal(&format!("could not parse {label}: {error}")))
}

fn env_path(name: &str, default: &str) -> PathBuf {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(default))
}

fn env_usize(name: &str, default: usize, minimum: usize, maximum: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| (*value >= minimum) && (*value <= maximum))
        .unwrap_or(default)
}

fn env_u64(name: &str, default: u64, minimum: u64, maximum: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| (*value >= minimum) && (*value <= maximum))
        .unwrap_or(default)
}

fn env_u16(name: &str, default: u16) -> u16 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn fatal(message: &str) -> ! {
    eprintln!("Fatal: {message}");
    std::process::exit(1)
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("install SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => {},
            _ = terminate.recv() => {},
        }
    }
    #[cfg(not(unix))]
    {
        let _ = ctrl_c.await;
    }
    tracing::info!("shutting down nats HTTP ingress");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(subject: &str, stream: &str) -> RouteConfig {
        RouteConfig {
            subject: subject.to_string(),
            stream: stream.to_string(),
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
        }
    }

    fn config() -> (Routes, Clients) {
        let routes = BTreeMap::from([(
            "vapi-task".to_string(),
            route("dd.vapi.tasks.external", "DD_VAPI_TASKS"),
        )]);
        let clients = BTreeMap::from([(
            "external-vapi".to_string(),
            ClientConfig {
                token: "x".repeat(MIN_TOKEN_BYTES),
                routes: BTreeSet::from(["vapi-task".to_string()]),
            },
        )]);
        (routes, clients)
    }

    #[test]
    fn accepts_valid_configuration() {
        let (routes, clients) = config();
        assert!(validate_configuration(&routes, &clients).is_ok());
    }

    #[test]
    fn rejects_wildcard_system_and_unknown_routes() {
        let (mut routes, clients) = config();
        routes.insert("bad".into(), route("$JS.API.>", "DD_VAPI_TASKS"));
        assert!(validate_configuration(&routes, &clients).is_err());

        let (routes, mut clients) = config();
        clients.get_mut("external-vapi").unwrap().routes = BTreeSet::from(["missing".into()]);
        assert!(validate_configuration(&routes, &clients).is_err());
    }

    #[test]
    fn rejects_duplicate_or_short_client_tokens() {
        let (routes, mut clients) = config();
        clients.insert(
            "second-client".into(),
            ClientConfig {
                token: "x".repeat(MIN_TOKEN_BYTES),
                routes: BTreeSet::from(["vapi-task".into()]),
            },
        );
        assert!(validate_configuration(&routes, &clients).is_err());

        let (routes, mut clients) = config();
        clients.get_mut("external-vapi").unwrap().token = "short".into();
        assert!(validate_configuration(&routes, &clients).is_err());
    }

    #[test]
    fn validates_bounded_idempotency_keys() {
        assert!(validate_idempotency_key("request-12345678").is_ok());
        assert!(validate_idempotency_key("short").is_err());
        assert!(validate_idempotency_key("contains space").is_err());
        assert!(validate_idempotency_key(&"x".repeat(MAX_IDEMPOTENCY_BYTES + 1)).is_err());
    }

    #[test]
    fn identifiers_and_subjects_are_strict() {
        assert!(valid_identifier("external-vapi"));
        assert!(!valid_identifier("ExternalVapi"));
        assert!(!valid_identifier("external.vapi"));
        assert!(validate_subject("dd.vapi.tasks.external").is_ok());
        assert!(validate_subject("dd.vapi.tasks.*").is_err());
        assert!(validate_subject("dd.vapi.tasks.").is_err());
    }

    #[test]
    fn token_comparison_is_exact() {
        assert!(constant_time_eq(
            "abcdefghijklmnopqrstuvwxyz123456",
            "abcdefghijklmnopqrstuvwxyz123456"
        ));
        assert!(!constant_time_eq(
            "abcdefghijklmnopqrstuvwxyz123456",
            "abcdefghijklmnopqrstuvwxyz123457"
        ));
        assert!(!constant_time_eq(
            "short",
            "abcdefghijklmnopqrstuvwxyz123456"
        ));
    }
}
