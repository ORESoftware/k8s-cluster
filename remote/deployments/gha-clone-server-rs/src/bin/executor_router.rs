use std::{
    collections::BTreeSet,
    env,
    sync::Arc,
};

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use hmac::{Hmac, Mac};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::{net::TcpListener, time::Duration};
use tracing::{info, warn};

const SERVICE_NAME: &str = "gha-executor-router";
const DEFAULT_PORT: u16 = 8126;
const DEFAULT_MAX_BODY_BYTES: usize = 256 * 1024;
const MAX_ROUTES: usize = 8;
const MAX_ROUTE_CONFIG_BYTES: usize = 64 * 1024;
const MAX_ROUTE_TOKEN_BYTES: usize = 1024;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    client: Client,
}

#[derive(Clone, Debug)]
struct Config {
    auth_secret: String,
    routing_secret: String,
    routes: Vec<ExecutorRoute>,
    max_body_bytes: usize,
}

impl Config {
    fn from_env() -> Result<Self, String> {
        let auth_secret = required_secret("GHA_EXECUTOR_ROUTER_AUTH_SECRET", 16)?;
        let routing_secret = required_secret("GHA_EXECUTOR_ROUTING_SECRET", 32)?;
        let routes_json = env::var("GHA_EXECUTOR_ROUTES_JSON")
            .map_err(|_| "GHA_EXECUTOR_ROUTES_JSON is required".to_string())?;
        let routes = parse_routes(&routes_json, |name| env::var(name).ok())?;
        let max_body_bytes = env_usize(
            "GHA_EXECUTOR_ROUTER_MAX_BODY_BYTES",
            DEFAULT_MAX_BODY_BYTES,
        )?;
        if max_body_bytes == 0 || max_body_bytes > 4 * 1024 * 1024 {
            return Err(
                "GHA_EXECUTOR_ROUTER_MAX_BODY_BYTES must be between 1 and 4194304".to_string(),
            );
        }
        Ok(Self {
            auth_secret,
            routing_secret,
            routes,
            max_body_bytes,
        })
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawExecutorRoute {
    name: String,
    provider: String,
    url: String,
    auth_env: String,
    priority: u16,
    profiles: Vec<String>,
    #[serde(default = "default_enabled")]
    enabled: bool,
}

fn default_enabled() -> bool {
    true
}

#[derive(Clone, Debug)]
struct ExecutorRoute {
    name: String,
    provider: String,
    url: String,
    auth: String,
    priority: u16,
    profiles: BTreeSet<String>,
}

impl ExecutorRoute {
    fn supports(&self, profile: &str) -> bool {
        self.profiles.contains("*") || self.profiles.contains(profile)
    }
}

#[derive(Clone, Debug)]
struct BuildMetadata {
    profile: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RouteTokenPayload {
    version: u8,
    route: String,
    upstream_job_id: String,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "gha_executor_router=info".into()),
        )
        .init();

    let config = Config::from_env().unwrap_or_else(|error| {
        eprintln!("{SERVICE_NAME}: configuration error: {error}");
        std::process::exit(2);
    });
    let host = env::var("GHA_EXECUTOR_ROUTER_HOST").unwrap_or_else(|_| "0.0.0.0".into());
    let port = env_u16("GHA_EXECUTOR_ROUTER_PORT", DEFAULT_PORT).unwrap_or_else(|error| {
        eprintln!("{SERVICE_NAME}: configuration error: {error}");
        std::process::exit(2);
    });
    let address = format!("{host}:{port}");
    let state = AppState {
        config: Arc::new(config),
        client: Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(65))
            .user_agent("gha-executor-router/0.1")
            .build()
            .expect("reqwest client"),
    };
    let listener = TcpListener::bind(&address)
        .await
        .unwrap_or_else(|error| panic!("failed to bind {address}: {error}"));
    info!(%address, routes = state.config.routes.len(), "listening");
    axum::serve(listener, router(state))
        .await
        .expect("executor router server");
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(descriptor))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/v1/executors", get(list_executors))
        .route("/builds", post(submit_build))
        .route("/builds/:id", get(get_build))
        .with_state(state)
}

async fn descriptor() -> Json<Value> {
    Json(json!({
        "service": SERVICE_NAME,
        "purpose": "stateless signed routing for fixed-profile build jobs across AWS and Hetzner executors",
        "safety": {
            "retryable": ["connect failure", "HTTP 429", "HTTP 502", "HTTP 503", "HTTP 504"],
            "neverRetried": ["accepted jobs", "timeouts after connection", "validation failures", "authentication failures", "other HTTP responses"]
        },
        "endpoints": {
            "executors": "GET /v1/executors",
            "submit": "POST /builds",
            "status": "GET /builds/<signed-route-id>",
            "health": "GET /healthz",
            "ready": "GET /readyz"
        }
    }))
}

async fn healthz(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "routes": state.config.routes.len()
    }))
}

async fn readyz(State(state): State<AppState>) -> Response {
    let ready = !state.config.routes.is_empty();
    (
        if ready {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(json!({
            "ok": ready,
            "service": SERVICE_NAME,
            "configuredRoutes": state.config.routes.len()
        })),
    )
        .into_response()
}

async fn list_executors(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let routes = state
        .config
        .routes
        .iter()
        .map(|route| {
            json!({
                "name": route.name,
                "provider": route.provider,
                "priority": route.priority,
                "profiles": route.profiles
            })
        })
        .collect::<Vec<_>>();
    (StatusCode::OK, Json(json!({ "executors": routes }))).into_response()
}

async fn submit_build(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    if body.len() > state.config.max_body_bytes {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({
                "error": "build request exceeds configured body limit",
                "maxBytes": state.config.max_body_bytes
            })),
        )
            .into_response();
    }
    let request: Value = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("invalid build request JSON: {error}") })),
            )
                .into_response()
        }
    };
    let metadata = match validate_build_request(&request) {
        Ok(metadata) => metadata,
        Err(error) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({ "error": error })),
            )
                .into_response()
        }
    };
    let candidates = state
        .config
        .routes
        .iter()
        .filter(|route| route.supports(&metadata.profile))
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({
                "error": "no executor route supports the requested fixed profile",
                "profile": metadata.profile
            })),
        )
            .into_response();
    }

    let mut attempts = Vec::new();
    for route in candidates {
        let response = state
            .client
            .post(format!("{}/builds", route.url))
            .header("x-build-server-auth", &route.auth)
            .header("content-type", "application/json")
            .body(body.clone())
            .send()
            .await;

        let response = match response {
            Ok(response) => response,
            Err(error) if error.is_connect() => {
                warn!(
                    route = %route.name,
                    provider = %route.provider,
                    "executor connection failed before an upstream response"
                );
                attempts.push(json!({
                    "route": route.name,
                    "provider": route.provider,
                    "result": "connect-failure"
                }));
                continue;
            }
            Err(error) => {
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({
                        "error": "executor submission failed ambiguously; request was not retried",
                        "route": route.name,
                        "provider": route.provider,
                        "detail": bounded_text(&error.to_string(), 512)
                    })),
                )
                    .into_response()
            }
        };

        let status = response.status();
        let response_body = match response.bytes().await {
            Ok(body) => body,
            Err(error) if retryable_status(status) => {
                attempts.push(json!({
                    "route": route.name,
                    "provider": route.provider,
                    "status": status.as_u16(),
                    "result": "retryable-response-body-error",
                    "detail": bounded_text(&error.to_string(), 256)
                }));
                continue;
            }
            Err(error) => {
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({
                        "error": "executor response body failed after a non-retryable response; request was not retried",
                        "route": route.name,
                        "provider": route.provider,
                        "status": status.as_u16(),
                        "detail": bounded_text(&error.to_string(), 512)
                    })),
                )
                    .into_response()
            }
        };

        if status == StatusCode::ACCEPTED {
            let mut upstream: Value = match serde_json::from_slice(&response_body) {
                Ok(value) => value,
                Err(error) => {
                    return (
                        StatusCode::BAD_GATEWAY,
                        Json(json!({
                            "error": "accepted executor response was not valid JSON; request was not retried",
                            "route": route.name,
                            "provider": route.provider,
                            "detail": error.to_string()
                        })),
                    )
                        .into_response()
                }
            };
            let upstream_id = upstream
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|id| valid_upstream_job_id(id));
            let Some(upstream_id) = upstream_id else {
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({
                        "error": "accepted executor response omitted a valid job id; request was not retried",
                        "route": route.name,
                        "provider": route.provider
                    })),
                )
                    .into_response();
            };
            let token = match encode_route_token(
                &state.config.routing_secret,
                &route.name,
                &upstream_id,
            ) {
                Ok(token) => token,
                Err(error) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({
                            "error": "failed to encode accepted executor route",
                            "detail": error
                        })),
                    )
                        .into_response()
                }
            };
            let Some(object) = upstream.as_object_mut() else {
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({
                        "error": "accepted executor response must be a JSON object; request was not retried",
                        "route": route.name,
                        "provider": route.provider
                    })),
                )
                    .into_response();
            };
            object.insert("id".into(), Value::String(token));
            object.insert("executorRoute".into(), Value::String(route.name.clone()));
            object.insert("executorProvider".into(), Value::String(route.provider.clone()));
            return (StatusCode::ACCEPTED, Json(upstream)).into_response();
        }

        if retryable_status(status) {
            attempts.push(json!({
                "route": route.name,
                "provider": route.provider,
                "status": status.as_u16(),
                "result": "retryable-capacity-response",
                "detail": bounded_bytes(&response_body, 256)
            }));
            continue;
        }

        let proxy_status = StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
        return (
            proxy_status,
            Json(json!({
                "error": "executor rejected build request; request was not failed over",
                "route": route.name,
                "provider": route.provider,
                "status": status.as_u16(),
                "upstreamBody": bounded_bytes(&response_body, 1024)
            })),
        )
            .into_response();
    }

    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "error": "all compatible executor routes were unavailable",
            "profile": metadata.profile,
            "attempts": attempts
        })),
    )
        .into_response()
}

async fn get_build(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(token): Path<String>,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let payload = match decode_route_token(&state.config.routing_secret, &token) {
        Ok(payload) => payload,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": error })),
            )
                .into_response()
        }
    };
    let Some(route) = state
        .config
        .routes
        .iter()
        .find(|route| route.name == payload.route)
    else {
        return (
            StatusCode::GONE,
            Json(json!({
                "error": "executor route encoded by this job id is no longer configured",
                "route": payload.route
            })),
        )
            .into_response();
    };

    let response = match state
        .client
        .get(format!("{}/builds/{}", route.url, payload.upstream_job_id))
        .header("x-build-server-auth", &route.auth)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "error": "executor status request failed",
                    "route": route.name,
                    "provider": route.provider,
                    "detail": bounded_text(&error.to_string(), 512)
                })),
            )
                .into_response()
        }
    };
    let status = response.status();
    let response_body = match response.bytes().await {
        Ok(body) => body,
        Err(error) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "error": "executor status response body failed",
                    "route": route.name,
                    "provider": route.provider,
                    "detail": bounded_text(&error.to_string(), 512)
                })),
            )
                .into_response()
        }
    };
    if status != StatusCode::OK {
        let proxy_status = StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
        return (
            proxy_status,
            Json(json!({
                "error": "executor status request was rejected",
                "route": route.name,
                "provider": route.provider,
                "status": status.as_u16(),
                "upstreamBody": bounded_bytes(&response_body, 1024)
            })),
        )
            .into_response();
    }
    let mut upstream: Value = match serde_json::from_slice(&response_body) {
        Ok(value) => value,
        Err(error) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "error": "executor status response was not valid JSON",
                    "route": route.name,
                    "provider": route.provider,
                    "detail": error.to_string()
                })),
            )
                .into_response()
        }
    };
    let Some(object) = upstream.as_object_mut() else {
        return (
            StatusCode::BAD_GATEWAY,
            Json(json!({
                "error": "executor status response must be a JSON object",
                "route": route.name,
                "provider": route.provider
            })),
        )
            .into_response();
    };
    object.insert("id".into(), Value::String(token));
    object.insert("executorRoute".into(), Value::String(route.name.clone()));
    object.insert("executorProvider".into(), Value::String(route.provider.clone()));
    (StatusCode::OK, Json(upstream)).into_response()
}

fn parse_routes<F>(json_text: &str, lookup_secret: F) -> Result<Vec<ExecutorRoute>, String>
where
    F: Fn(&str) -> Option<String>,
{
    if json_text.trim().is_empty() {
        return Err("GHA_EXECUTOR_ROUTES_JSON must not be empty".to_string());
    }
    if json_text.len() > MAX_ROUTE_CONFIG_BYTES {
        return Err(format!(
            "GHA_EXECUTOR_ROUTES_JSON exceeds {MAX_ROUTE_CONFIG_BYTES} bytes"
        ));
    }
    let raw_routes: Vec<RawExecutorRoute> = serde_json::from_str(json_text)
        .map_err(|error| format!("GHA_EXECUTOR_ROUTES_JSON is invalid: {error}"))?;
    if raw_routes.len() > MAX_ROUTES {
        return Err(format!("at most {MAX_ROUTES} executor routes are allowed"));
    }

    let mut names = BTreeSet::new();
    let mut routes = Vec::new();
    for raw in raw_routes.into_iter().filter(|route| route.enabled) {
        if !valid_route_name(&raw.name) {
            return Err(format!(
                "executor route name {:?} must be 1-48 ASCII letters, digits, '.', '_', or '-'",
                raw.name
            ));
        }
        if !names.insert(raw.name.clone()) {
            return Err(format!("duplicate executor route name {:?}", raw.name));
        }
        if !matches!(raw.provider.as_str(), "aws" | "hetzner") {
            return Err(format!(
                "executor route {:?} provider must be aws or hetzner",
                raw.name
            ));
        }
        let parsed_url = reqwest::Url::parse(raw.url.trim())
            .map_err(|error| format!("executor route {:?} URL is invalid: {error}", raw.name))?;
        if !matches!(parsed_url.scheme(), "http" | "https")
            || parsed_url.host_str().is_none()
            || parsed_url.username() != ""
            || parsed_url.password().is_some()
            || parsed_url.query().is_some()
            || parsed_url.fragment().is_some()
        {
            return Err(format!(
                "executor route {:?} URL must be a credential-free http(s) origin without query or fragment",
                raw.name
            ));
        }
        if !valid_env_name(&raw.auth_env) {
            return Err(format!(
                "executor route {:?} authEnv is not a valid environment variable name",
                raw.name
            ));
        }
        let auth = lookup_secret(&raw.auth_env)
            .map(|value| value.trim().to_string())
            .filter(|value| value.len() >= 16)
            .ok_or_else(|| {
                format!(
                    "executor route {:?} requires a secret of at least 16 characters in {}",
                    raw.name, raw.auth_env
                )
            })?;
        if raw.profiles.is_empty() || raw.profiles.len() > 32 {
            return Err(format!(
                "executor route {:?} must declare 1-32 fixed profiles",
                raw.name
            ));
        }
        let mut profiles = BTreeSet::new();
        for profile in raw.profiles {
            if profile != "*" && !valid_profile(&profile) {
                return Err(format!(
                    "executor route {:?} contains invalid fixed profile {:?}",
                    raw.name, profile
                ));
            }
            profiles.insert(profile);
        }
        routes.push(ExecutorRoute {
            name: raw.name,
            provider: raw.provider,
            url: raw.url.trim().trim_end_matches('/').to_string(),
            auth,
            priority: raw.priority,
            profiles,
        });
    }
    if routes.is_empty() {
        return Err("at least one enabled executor route is required".to_string());
    }
    routes.sort_by(|left, right| {
        left.priority
            .cmp(&right.priority)
            .then_with(|| left.name.cmp(&right.name))
    });
    Ok(routes)
}

fn validate_build_request(request: &Value) -> Result<BuildMetadata, String> {
    let object = request
        .as_object()
        .ok_or_else(|| "build request must be a JSON object".to_string())?;
    if object
        .get("schemaVersion")
        .and_then(Value::as_str)
        .is_some_and(|value| value != "build-server.v1")
    {
        return Err("schemaVersion must be build-server.v1".to_string());
    }
    if object.get("jobKind").and_then(Value::as_str) != Some("run-profile") {
        return Err("executor router only accepts jobKind=run-profile".to_string());
    }
    let profile = object
        .get("profile")
        .and_then(Value::as_str)
        .filter(|value| valid_profile(value))
        .ok_or_else(|| "profile must be a fixed profile identifier".to_string())?
        .to_string();
    let revision = object
        .get("gitRef")
        .and_then(Value::as_str)
        .ok_or_else(|| "gitRef must be an immutable 40-hex commit SHA".to_string())?;
    if !is_full_commit_sha(revision) {
        return Err("gitRef must be an immutable 40-hex commit SHA".to_string());
    }
    let request_id = object
        .get("requestId")
        .and_then(Value::as_str)
        .ok_or_else(|| "requestId is required for cross-executor routing".to_string())?;
    if request_id.is_empty()
        || request_id.len() > 128
        || request_id.chars().any(char::is_whitespace)
        || request_id.chars().any(char::is_control)
    {
        return Err(
            "requestId must be 1-128 non-whitespace, non-control characters".to_string(),
        );
    }
    Ok(BuildMetadata { profile })
}

fn encode_route_token(secret: &str, route: &str, upstream_job_id: &str) -> Result<String, String> {
    if !valid_route_name(route) || !valid_upstream_job_id(upstream_job_id) {
        return Err("route token contains an invalid route or upstream job id".to_string());
    }
    let payload = RouteTokenPayload {
        version: 1,
        route: route.to_string(),
        upstream_job_id: upstream_job_id.to_string(),
    };
    let bytes = serde_json::to_vec(&payload)
        .map_err(|error| format!("route token payload serialization failed: {error}"))?;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|_| "route token secret is invalid".to_string())?;
    mac.update(&bytes);
    let signature = mac.finalize().into_bytes();
    let token = format!("r1.{}.{}", hex::encode(bytes), hex::encode(signature));
    if token.len() > MAX_ROUTE_TOKEN_BYTES {
        return Err("route token exceeds the size limit".to_string());
    }
    Ok(token)
}

fn decode_route_token(secret: &str, token: &str) -> Result<RouteTokenPayload, String> {
    if token.len() > MAX_ROUTE_TOKEN_BYTES {
        return Err("signed route id exceeds the size limit".to_string());
    }
    let mut parts = token.split('.');
    let version = parts.next();
    let payload_hex = parts.next();
    let signature_hex = parts.next();
    if version != Some("r1")
        || payload_hex.is_none()
        || signature_hex.is_none()
        || parts.next().is_some()
    {
        return Err("signed route id has an invalid envelope".to_string());
    }
    let payload_bytes = hex::decode(payload_hex.expect("checked"))
        .map_err(|_| "signed route id payload is not valid hex".to_string())?;
    let signature = hex::decode(signature_hex.expect("checked"))
        .map_err(|_| "signed route id signature is not valid hex".to_string())?;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|_| "route token secret is invalid".to_string())?;
    mac.update(&payload_bytes);
    mac.verify_slice(&signature)
        .map_err(|_| "signed route id failed authentication".to_string())?;
    let payload: RouteTokenPayload = serde_json::from_slice(&payload_bytes)
        .map_err(|_| "signed route id payload is invalid".to_string())?;
    if payload.version != 1
        || !valid_route_name(&payload.route)
        || !valid_upstream_job_id(&payload.upstream_job_id)
    {
        return Err("signed route id payload failed validation".to_string());
    }
    Ok(payload)
}

fn retryable_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::TOO_MANY_REQUESTS
            | StatusCode::BAD_GATEWAY
            | StatusCode::SERVICE_UNAVAILABLE
            | StatusCode::GATEWAY_TIMEOUT
    )
}

fn require_auth(headers: &HeaderMap, state: &AppState) -> Result<(), Response> {
    let presented = headers
        .get("x-build-server-auth")
        .or_else(|| headers.get("x-server-auth"))
        .and_then(|value| value.to_str().ok());
    if presented.is_some_and(|value| digest_eq(value, &state.config.auth_secret)) {
        Ok(())
    } else {
        Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response())
    }
}

fn digest_eq(left: &str, right: &str) -> bool {
    let left = Sha256::digest(left.as_bytes());
    let right = Sha256::digest(right.as_bytes());
    left.as_slice().ct_eq(right.as_slice()).into()
}

fn valid_route_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 48
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn valid_profile(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn valid_upstream_job_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn valid_env_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first.is_ascii_uppercase() || first == b'_')
        && value.len() <= 96
        && bytes.all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

fn is_full_commit_sha(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn bounded_text(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn bounded_bytes(value: &[u8], max_chars: usize) -> String {
    bounded_text(&String::from_utf8_lossy(value), max_chars)
}

fn required_secret(name: &str, min_len: usize) -> Result<String, String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| value.len() >= min_len)
        .ok_or_else(|| format!("{name} must contain at least {min_len} characters"))
}

fn env_u16(name: &str, default: u16) -> Result<u16, String> {
    match env::var(name) {
        Ok(value) => value
            .parse::<u16>()
            .map_err(|error| format!("{name} is invalid: {error}")),
        Err(_) => Ok(default),
    }
}

fn env_usize(name: &str, default: usize) -> Result<usize, String> {
    match env::var(name) {
        Ok(value) => value
            .parse::<usize>()
            .map_err(|error| format!("{name} is invalid: {error}")),
        Err(_) => Ok(default),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use axum::{
        body::{to_bytes, Body},
        extract::State as AxumState,
        http::Request,
        routing::{get as axum_get, post as axum_post},
    };
    use tower::ServiceExt;

    use super::*;

    fn route(name: &str, provider: &str, url: &str, priority: u16) -> ExecutorRoute {
        ExecutorRoute {
            name: name.to_string(),
            provider: provider.to_string(),
            url: url.to_string(),
            auth: "upstream-secret-1234".to_string(),
            priority,
            profiles: BTreeSet::from(["rust-verify".to_string()]),
        }
    }

    fn test_state(routes: Vec<ExecutorRoute>) -> AppState {
        AppState {
            config: Arc::new(Config {
                auth_secret: "incoming-secret-1234".to_string(),
                routing_secret: "routing-secret-with-at-least-thirty-two-bytes".to_string(),
                routes,
                max_body_bytes: DEFAULT_MAX_BODY_BYTES,
            }),
            client: Client::builder()
                .connect_timeout(Duration::from_millis(300))
                .timeout(Duration::from_secs(2))
                .build()
                .unwrap(),
        }
    }

    fn build_request() -> String {
        json!({
            "schemaVersion": "build-server.v1",
            "jobKind": "run-profile",
            "repoUrl": "https://github.com/ORESoftware/k8s-cluster.git",
            "gitRef": "0123456789abcdef0123456789abcdef01234567",
            "profile": "rust-verify",
            "requestId": "gha-clone:plan:rust"
        })
        .to_string()
    }

    #[derive(Clone)]
    struct MockExecutor {
        submit_status: StatusCode,
        submit_body: Value,
        status_body: Value,
        submit_hits: Arc<AtomicUsize>,
        status_hits: Arc<AtomicUsize>,
    }

    async fn mock_submit(AxumState(state): AxumState<MockExecutor>) -> Response {
        state.submit_hits.fetch_add(1, Ordering::SeqCst);
        (state.submit_status, Json(state.submit_body)).into_response()
    }

    async fn mock_status(AxumState(state): AxumState<MockExecutor>) -> Response {
        state.status_hits.fetch_add(1, Ordering::SeqCst);
        (StatusCode::OK, Json(state.status_body)).into_response()
    }

    async fn spawn_mock(state: MockExecutor) -> String {
        let app = Router::new()
            .route("/builds", axum_post(mock_submit))
            .route("/builds/:id", axum_get(mock_status))
            .with_state(state);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{address}")
    }

    async fn call_submit(app: Router) -> Response {
        app.oneshot(
            Request::builder()
                .method("POST")
                .uri("/builds")
                .header("x-build-server-auth", "incoming-secret-1234")
                .header("content-type", "application/json")
                .body(Body::from(build_request()))
                .unwrap(),
        )
        .await
        .unwrap()
    }

    #[test]
    fn routes_are_sorted_and_filtered_deterministically() {
        let json = r#"[
          {"name":"hetzner-b","provider":"hetzner","url":"https://hetzner.example","authEnv":"HETZNER_AUTH","priority":20,"profiles":["rust-verify"]},
          {"name":"aws-a","provider":"aws","url":"https://aws.example","authEnv":"AWS_AUTH","priority":10,"profiles":["*"]},
          {"name":"aws-b","provider":"aws","url":"https://aws-b.example","authEnv":"AWS_B_AUTH","priority":10,"profiles":["node-verify"],"enabled":false}
        ]"#;
        let routes = parse_routes(json, |_| Some("0123456789abcdef".to_string())).unwrap();
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].name, "aws-a");
        assert_eq!(routes[1].name, "hetzner-b");
        assert!(routes[0].supports("python-verify"));
        assert!(!routes[1].supports("python-verify"));
    }

    #[test]
    fn route_configuration_fails_closed() {
        let duplicate = r#"[
          {"name":"same","provider":"aws","url":"https://a.example","authEnv":"AUTH_A","priority":1,"profiles":["rust-verify"]},
          {"name":"same","provider":"hetzner","url":"https://b.example","authEnv":"AUTH_B","priority":2,"profiles":["rust-verify"]}
        ]"#;
        assert!(parse_routes(duplicate, |_| Some("0123456789abcdef".into()))
            .unwrap_err()
            .contains("duplicate"));

        let credentialed_url = r#"[
          {"name":"bad","provider":"aws","url":"https://user:pass@a.example","authEnv":"AUTH_A","priority":1,"profiles":["rust-verify"]}
        ]"#;
        assert!(parse_routes(credentialed_url, |_| Some("0123456789abcdef".into()))
            .unwrap_err()
            .contains("credential-free"));

        let missing_secret = r#"[
          {"name":"aws","provider":"aws","url":"https://a.example","authEnv":"AUTH_A","priority":1,"profiles":["rust-verify"]}
        ]"#;
        assert!(parse_routes(missing_secret, |_| None)
            .unwrap_err()
            .contains("requires a secret"));
    }

    #[test]
    fn build_request_requires_fixed_profile_immutable_sha_and_idempotency_key() {
        let valid: Value = serde_json::from_str(&build_request()).unwrap();
        assert_eq!(
            validate_build_request(&valid).unwrap().profile,
            "rust-verify"
        );

        let mut branch = valid.clone();
        branch["gitRef"] = json!("main");
        assert!(validate_build_request(&branch)
            .unwrap_err()
            .contains("40-hex"));

        let mut missing_request_id = valid.clone();
        missing_request_id.as_object_mut().unwrap().remove("requestId");
        assert!(validate_build_request(&missing_request_id)
            .unwrap_err()
            .contains("requestId"));

        let mut arbitrary_job = valid;
        arbitrary_job["jobKind"] = json!("build-image");
        assert!(validate_build_request(&arbitrary_job)
            .unwrap_err()
            .contains("run-profile"));
    }

    #[test]
    fn signed_route_tokens_round_trip_and_reject_tampering() {
        let secret = "routing-secret-with-at-least-thirty-two-bytes";
        let token = encode_route_token(secret, "aws-primary", "build-123").unwrap();
        let payload = decode_route_token(secret, &token).unwrap();
        assert_eq!(payload.route, "aws-primary");
        assert_eq!(payload.upstream_job_id, "build-123");

        let mut tampered = token.into_bytes();
        let last = tampered.len() - 1;
        tampered[last] = if tampered[last] == b'a' { b'b' } else { b'a' };
        assert!(decode_route_token(secret, std::str::from_utf8(&tampered).unwrap()).is_err());
        assert!(decode_route_token("different-routing-secret-123456789", "r1.00.00").is_err());
    }

    #[test]
    fn failover_statuses_are_explicit_and_bounded() {
        for status in [
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::BAD_GATEWAY,
            StatusCode::SERVICE_UNAVAILABLE,
            StatusCode::GATEWAY_TIMEOUT,
        ] {
            assert!(retryable_status(status));
        }
        for status in [
            StatusCode::BAD_REQUEST,
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::UNPROCESSABLE_ENTITY,
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::ACCEPTED,
        ] {
            assert!(!retryable_status(status));
        }
    }

    #[tokio::test]
    async fn explicit_capacity_failure_fails_over_and_status_proxy_is_stateless() {
        let primary_submit_hits = Arc::new(AtomicUsize::new(0));
        let primary_status_hits = Arc::new(AtomicUsize::new(0));
        let primary_url = spawn_mock(MockExecutor {
            submit_status: StatusCode::SERVICE_UNAVAILABLE,
            submit_body: json!({"error":"full"}),
            status_body: json!({"id":"unused","status":"failed"}),
            submit_hits: primary_submit_hits.clone(),
            status_hits: primary_status_hits,
        })
        .await;

        let secondary_submit_hits = Arc::new(AtomicUsize::new(0));
        let secondary_status_hits = Arc::new(AtomicUsize::new(0));
        let secondary_url = spawn_mock(MockExecutor {
            submit_status: StatusCode::ACCEPTED,
            submit_body: json!({"id":"build-secondary-1","status":"queued","error":null}),
            status_body: json!({"id":"build-secondary-1","status":"succeeded","error":null}),
            submit_hits: secondary_submit_hits.clone(),
            status_hits: secondary_status_hits.clone(),
        })
        .await;

        let app = router(test_state(vec![
            route("aws-primary", "aws", &primary_url, 10),
            route("hetzner-secondary", "hetzner", &secondary_url, 20),
        ]));
        let response = call_submit(app.clone()).await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        let token = value["id"].as_str().unwrap();
        assert_eq!(value["executorProvider"], "hetzner");
        assert_eq!(primary_submit_hits.load(Ordering::SeqCst), 1);
        assert_eq!(secondary_submit_hits.load(Ordering::SeqCst), 1);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/builds/{token}"))
                    .header("x-build-server-auth", "incoming-secret-1234")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["id"], token);
        assert_eq!(value["status"], "succeeded");
        assert_eq!(secondary_status_hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn validation_failure_never_reaches_secondary_executor() {
        let primary_submit_hits = Arc::new(AtomicUsize::new(0));
        let primary_url = spawn_mock(MockExecutor {
            submit_status: StatusCode::UNPROCESSABLE_ENTITY,
            submit_body: json!({"error":"bad profile"}),
            status_body: json!({"id":"unused","status":"failed"}),
            submit_hits: primary_submit_hits.clone(),
            status_hits: Arc::new(AtomicUsize::new(0)),
        })
        .await;
        let secondary_submit_hits = Arc::new(AtomicUsize::new(0));
        let secondary_url = spawn_mock(MockExecutor {
            submit_status: StatusCode::ACCEPTED,
            submit_body: json!({"id":"should-not-run","status":"queued"}),
            status_body: json!({"id":"should-not-run","status":"queued"}),
            submit_hits: secondary_submit_hits.clone(),
            status_hits: Arc::new(AtomicUsize::new(0)),
        })
        .await;
        let app = router(test_state(vec![
            route("aws-primary", "aws", &primary_url, 10),
            route("hetzner-secondary", "hetzner", &secondary_url, 20),
        ]));
        let response = call_submit(app).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(primary_submit_hits.load(Ordering::SeqCst), 1);
        assert_eq!(secondary_submit_hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn accepted_primary_job_is_never_submitted_twice() {
        let primary_submit_hits = Arc::new(AtomicUsize::new(0));
        let primary_url = spawn_mock(MockExecutor {
            submit_status: StatusCode::ACCEPTED,
            submit_body: json!({"id":"build-primary-1","status":"queued"}),
            status_body: json!({"id":"build-primary-1","status":"queued"}),
            submit_hits: primary_submit_hits.clone(),
            status_hits: Arc::new(AtomicUsize::new(0)),
        })
        .await;
        let secondary_submit_hits = Arc::new(AtomicUsize::new(0));
        let secondary_url = spawn_mock(MockExecutor {
            submit_status: StatusCode::ACCEPTED,
            submit_body: json!({"id":"build-secondary-1","status":"queued"}),
            status_body: json!({"id":"build-secondary-1","status":"queued"}),
            submit_hits: secondary_submit_hits.clone(),
            status_hits: Arc::new(AtomicUsize::new(0)),
        })
        .await;
        let app = router(test_state(vec![
            route("aws-primary", "aws", &primary_url, 10),
            route("hetzner-secondary", "hetzner", &secondary_url, 20),
        ]));
        let response = call_submit(app).await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert_eq!(primary_submit_hits.load(Ordering::SeqCst), 1);
        assert_eq!(secondary_submit_hits.load(Ordering::SeqCst), 0);
    }
}
