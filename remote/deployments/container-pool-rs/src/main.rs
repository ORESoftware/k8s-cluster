use std::{
    collections::{BTreeMap, HashMap, HashSet},
    env,
    io::BufReader,
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use futures_util::StreamExt;
use reqwest::header::{HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::{
    process::Command,
    sync::Mutex,
    time::{sleep, timeout},
};

const SERVICE_NAME: &str = "dd-container-pool";
const DEFAULT_PORT: u16 = 8102;
const MAX_HTTP_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_NATS_PAYLOAD_BYTES: usize = 2 * 1024 * 1024;
const MAX_WORKER_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone)]
struct AppState {
    config: Arc<ServiceConfig>,
    registry: Arc<Mutex<PoolRegistry>>,
    http: reqwest::Client,
    nats: Option<async_nats::Client>,
    metrics: Arc<Metrics>,
}

#[derive(Clone)]
struct ServiceConfig {
    nerdctl_bin: String,
    containerd_namespace: String,
    network: String,
    pull_policy: Option<String>,
    database_url: Option<String>,
    app_config_key: String,
    app_config_scope: String,
    nats_url: Option<String>,
    nats_subject: String,
    nats_result_subject: String,
    nats_max_payload_bytes: usize,
    worker_response_max_bytes: usize,
    config_refresh: Duration,
    reconcile_interval: Duration,
    command_timeout: Duration,
    container_start_timeout: Duration,
    health_check_interval: Duration,
    health_check_timeout: Duration,
    unhealthy_grace: Duration,
    unhealthy_failure_threshold: u64,
    port_start: u16,
    port_end: u16,
    cleanup_on_start: bool,
    server_auth_secret: Option<String>,
    container_memory: Option<String>,
    container_cpus: Option<String>,
    forward_env_keys: Vec<String>,
    pids_limit: u64,
    nofile_limit: u64,
}

#[derive(Default)]
struct Metrics {
    http_requests_total: AtomicU64,
    dispatch_total: AtomicU64,
    dispatch_failures_total: AtomicU64,
    nats_messages_total: AtomicU64,
    nats_failures_total: AtomicU64,
    containers_started_total: AtomicU64,
    containers_removed_total: AtomicU64,
    containers_unhealthy_total: AtomicU64,
    config_refresh_total: AtomicU64,
    config_refresh_failures_total: AtomicU64,
    container_health_checks_total: AtomicU64,
    container_health_check_failures_total: AtomicU64,
}

#[derive(Default)]
struct PoolRegistry {
    configs: HashMap<String, PoolConfig>,
    slug_to_id: HashMap<String, String>,
    containers: HashMap<String, WarmContainer>,
    affinity: HashMap<String, String>,
    next_port: u16,
    last_config_error: Option<String>,
    last_config_refresh_ms: Option<u128>,
}

#[derive(Debug, Clone)]
struct PoolConfig {
    id: String,
    slug: String,
    display_name: String,
    image: String,
    command: Vec<String>,
    env: BTreeMap<String, String>,
    request_path: String,
    health_path: String,
    container_port: u16,
    min_warm: usize,
    max_warm: usize,
    max_concurrency_per_container: usize,
    request_timeout: Duration,
    idle_ttl: Duration,
    nats_subject: Option<String>,
    read_only: bool,
    user: String,
    labels: Value,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum ContainerStatus {
    Starting,
    Idle,
    Busy,
    Draining,
    Unhealthy,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct WarmContainer {
    name: String,
    pool_id: String,
    pool_slug: String,
    affinity_key: Option<String>,
    port: u16,
    status: ContainerStatus,
    in_flight: usize,
    launched_at_ms: u128,
    last_used_at_ms: u128,
    last_health_at_ms: Option<u128>,
    last_healthy_at_ms: Option<u128>,
    health_failure_count: u64,
    last_health_error: Option<String>,
    request_count: u64,
    failure_count: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DispatchRequest {
    request_id: Option<String>,
    pool_id: Option<String>,
    pool_slug: Option<String>,
    affinity_key: Option<String>,
    path: Option<String>,
    headers: Option<BTreeMap<String, String>>,
    payload: Option<Value>,
    body: Option<Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DispatchResponse {
    ok: bool,
    request_id: String,
    pool_id: String,
    pool_slug: String,
    affinity_key: Option<String>,
    container_name: String,
    container_port: u16,
    target_url: String,
    status: u16,
    body: Value,
    elapsed_ms: u128,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HealthResponse {
    ok: bool,
    service: &'static str,
    postgres_configured: bool,
    nats_configured: bool,
    auth_configured: bool,
    pool_count: usize,
    warm_container_count: usize,
    last_config_refresh_ms: Option<u128>,
    last_config_error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PoolsResponse {
    ok: bool,
    generated_at_ms: u128,
    pools: Vec<PoolSummary>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PoolSummary {
    id: String,
    slug: String,
    display_name: String,
    image: String,
    request_path: String,
    health_path: String,
    container_port: u16,
    min_warm: usize,
    max_warm: usize,
    max_concurrency_per_container: usize,
    request_timeout_ms: u64,
    idle_ttl_seconds: u64,
    nats_subject: Option<String>,
    env_keys: Vec<String>,
    labels: Value,
    active_containers: usize,
    idle_containers: usize,
    busy_containers: usize,
    unhealthy_containers: usize,
    containers: Vec<WarmContainer>,
}

#[derive(Debug, Clone)]
struct ContainerLease {
    pool: PoolConfig,
    container: WarmContainer,
}

fn first_env(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        env::var(key)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

fn env_value(key: &str, fallback: &str) -> String {
    first_env(&[key]).unwrap_or_else(|| fallback.to_string())
}

fn env_bool(key: &str, fallback: bool) -> bool {
    first_env(&[key])
        .map(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(fallback)
}

fn env_u64(key: &str, fallback: u64) -> u64 {
    first_env(&[key])
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn env_u16(key: &str, fallback: u16) -> u16 {
    first_env(&[key])
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn env_usize(key: &str, fallback: usize) -> usize {
    first_env(&[key])
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn service_config_from_env() -> ServiceConfig {
    let port_start = env_u16("CONTAINER_POOL_PORT_START", 12_000);
    let port_end = env_u16("CONTAINER_POOL_PORT_END", 12_999).max(port_start);
    let network = env_value("CONTAINER_POOL_NETWORK", "host");
    ServiceConfig {
        nerdctl_bin: env_value("CONTAINER_POOL_NERDCTL_BIN", "/usr/local/bin/nerdctl"),
        containerd_namespace: env_value("CONTAINER_POOL_CONTAINERD_NAMESPACE", "k8s.io"),
        network: if safe_network_name(&network) {
            network
        } else {
            "host".to_string()
        },
        pull_policy: first_env(&["CONTAINER_POOL_PULL_POLICY"]).and_then(|value| {
            matches!(value.as_str(), "always" | "missing" | "never").then_some(value)
        }),
        database_url: first_env(&[
            "CONTAINER_POOL_DATABASE_URL",
            "AGENT_TASKS_RDS_DATABASE_URL",
            "RDS_DATABASE_URL",
            "DATABASE_URL",
        ]),
        app_config_key: env_value(
            "CONTAINER_POOL_APP_CONFIG_KEY",
            "container-pool.runtime-pools.v1",
        ),
        app_config_scope: env_value("CONTAINER_POOL_APP_CONFIG_SCOPE", "default"),
        nats_url: first_env(&["NATS_URL"]),
        nats_subject: env_value(
            "CONTAINER_POOL_NATS_SUBJECT",
            "dd.remote.container_pool.requests",
        ),
        nats_result_subject: env_value(
            "CONTAINER_POOL_NATS_RESULT_SUBJECT",
            "dd.remote.container_pool.results",
        ),
        nats_max_payload_bytes: env_usize(
            "CONTAINER_POOL_NATS_MAX_PAYLOAD_BYTES",
            MAX_NATS_PAYLOAD_BYTES,
        )
        .min(16 * 1024 * 1024),
        worker_response_max_bytes: env_usize(
            "CONTAINER_POOL_WORKER_RESPONSE_MAX_BYTES",
            MAX_WORKER_RESPONSE_BYTES,
        )
        .min(16 * 1024 * 1024),
        config_refresh: Duration::from_secs(env_u64("CONTAINER_POOL_CONFIG_REFRESH_SECONDS", 30)),
        reconcile_interval: Duration::from_secs(env_u64("CONTAINER_POOL_RECONCILE_SECONDS", 10)),
        command_timeout: Duration::from_secs(env_u64("CONTAINER_POOL_COMMAND_TIMEOUT_SECONDS", 30)),
        container_start_timeout: Duration::from_secs(env_u64(
            "CONTAINER_POOL_START_TIMEOUT_SECONDS",
            15,
        )),
        health_check_interval: Duration::from_secs(env_u64(
            "CONTAINER_POOL_HEALTH_CHECK_SECONDS",
            10,
        )),
        health_check_timeout: Duration::from_millis(env_u64(
            "CONTAINER_POOL_HEALTH_TIMEOUT_MS",
            1_000,
        )),
        unhealthy_grace: Duration::from_secs(env_u64("CONTAINER_POOL_UNHEALTHY_GRACE_SECONDS", 5)),
        unhealthy_failure_threshold: env_u64("CONTAINER_POOL_UNHEALTHY_FAILURE_THRESHOLD", 2)
            .clamp(1, 10),
        port_start,
        port_end,
        cleanup_on_start: env_bool("CONTAINER_POOL_CLEANUP_ON_START", true),
        server_auth_secret: first_env(&[
            "CONTAINER_POOL_AUTH_SECRET",
            "SERVER_AUTH_SECRET",
            "REMOTE_DEV_SERVER_SECRET",
        ]),
        container_memory: first_env(&["CONTAINER_POOL_CONTAINER_MEMORY"])
            .filter(|value| safe_resource_value(value)),
        container_cpus: first_env(&["CONTAINER_POOL_CONTAINER_CPUS"])
            .filter(|value| safe_resource_value(value)),
        forward_env_keys: forwarded_worker_env_keys(),
        pids_limit: env_u64("CONTAINER_POOL_PIDS_LIMIT", 128).clamp(16, 4096),
        nofile_limit: env_u64("CONTAINER_POOL_NOFILE_LIMIT", 128).clamp(32, 8192),
    }
}

fn forwarded_worker_env_keys() -> Vec<String> {
    let configured = first_env(&["CONTAINER_POOL_FORWARD_ENV_KEYS"]).unwrap_or_else(|| {
        [
            "SERVER_AUTH_SECRET",
            "REMOTE_DEV_SERVER_SECRET",
            "GH_DEPLOY_KEY",
            "GH_PAT",
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_API_KEYS_JSON",
            "CLAUDE_API_KEYS_JSON",
            "OPENAI_API_KEY",
            "OPENAI_API_KEYS_JSON",
            "GOOGLE_API_KEY",
            "GOOGLE_API_KEYS_JSON",
            "GEMINI_API_KEY",
            "GEMINI_API_KEYS_JSON",
            "OPENCODE_API_KEY",
            "OPENCODE_API_KEYS_JSON",
            "OPENCODE_BASE_URL",
            "OPENCODE_MODELS",
            "EVENT_INGEST_URL",
            "EVENT_INGEST_SECRET",
            "GLEAM_WORKER_WS_SECRET",
            "WORKER_FANOUT_WS_SECRET",
            "WORKER_FANOUT_WS_BASE_URL",
        ]
        .join(",")
    });
    configured
        .split(',')
        .map(str::trim)
        .filter(|key| safe_env_key(key))
        .map(str::to_string)
        .collect()
}

fn request_is_authorized(headers: &HeaderMap, secret: &str) -> bool {
    headers
        .get("x-server-auth")
        .or_else(|| headers.get("x-container-pool-auth"))
        .or_else(|| headers.get("x-agent-auth"))
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == secret)
}

fn require_auth(headers: &HeaderMap, state: &AppState) -> Result<(), Response> {
    let Some(secret) = state.config.server_auth_secret.as_deref() else {
        return Err(json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "SERVER_AUTH_SECRET is not configured",
        ));
    };
    if request_is_authorized(headers, secret) {
        Ok(())
    } else {
        Err(json_error(StatusCode::UNAUTHORIZED, "unauthorized"))
    }
}

fn json_error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "ok": false, "error": message }))).into_response()
}

fn safe_slug(input: &str) -> bool {
    let bytes = input.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 120
        && bytes[0].is_ascii_lowercase()
        && bytes[bytes.len() - 1].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

fn safe_env_key(input: &str) -> bool {
    let mut chars = input.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn safe_local_path(input: &str) -> bool {
    input.starts_with('/')
        && !input.starts_with("//")
        && !input.contains("://")
        && !input.contains('?')
        && !input.contains('#')
        && input.len() <= 256
        && !input
            .bytes()
            .any(|byte| byte <= 0x20 || byte == 0x7f || byte == b'\\')
        && !input
            .split('/')
            .any(|segment| matches!(segment, "." | ".."))
}

fn safe_container_image(input: &str) -> bool {
    let bytes = input.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 512
        && bytes[0].is_ascii_alphanumeric()
        && bytes.iter().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'/' | b'@' | b'-')
        })
}

fn safe_config_id(input: &str) -> bool {
    let bytes = input.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 120
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

fn safe_network_name(input: &str) -> bool {
    let bytes = input.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 128
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

fn safe_resource_value(input: &str) -> bool {
    let bytes = input.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 32
        && bytes[0].is_ascii_digit()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'.')
}

fn safe_nats_subject(input: &str) -> bool {
    let bytes = input.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 256
        && bytes[0].is_ascii_alphanumeric()
        && bytes.iter().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'*' | b'>')
        })
}

fn safe_env_value(input: &str) -> bool {
    input.len() <= 8192 && !input.contains('\0')
}

fn string_vec_from_json(value: Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn command_vec_from_json(value: Value) -> Vec<String> {
    string_vec_from_json(value)
        .into_iter()
        .filter(|value| !value.contains('\0') && value.len() <= 512)
        .take(32)
        .collect()
}

fn json_string_field(value: &Value, camel_key: &str, snake_key: &str) -> Option<String> {
    value
        .get(camel_key)
        .or_else(|| value.get(snake_key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn json_u64_field(value: &Value, camel_key: &str, snake_key: &str) -> Option<u64> {
    value
        .get(camel_key)
        .or_else(|| value.get(snake_key))
        .and_then(Value::as_u64)
}

fn json_bool_field(value: &Value, camel_key: &str, snake_key: &str) -> Option<bool> {
    value
        .get(camel_key)
        .or_else(|| value.get(snake_key))
        .and_then(Value::as_bool)
}

fn pool_config_from_json(value: &Value) -> Result<PoolConfig, String> {
    let slug = json_string_field(value, "slug", "slug")
        .ok_or_else(|| "container pool config is missing slug".to_string())?;
    if !safe_slug(&slug) {
        return Err(format!("invalid app_config container pool slug: {slug}"));
    }
    let image = json_string_field(value, "image", "image")
        .ok_or_else(|| format!("container pool {slug} is missing image"))?;
    if !safe_container_image(&image) {
        return Err(format!("container pool {slug} has invalid image"));
    }
    let min_warm = json_u64_field(value, "minWarm", "min_warm")
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(1)
        .min(64);
    let max_warm = json_u64_field(value, "maxWarm", "max_warm")
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(min_warm.max(1))
        .max(min_warm)
        .min(128);
    let request_path = json_string_field(value, "requestPath", "request_path")
        .filter(|path| safe_local_path(path))
        .unwrap_or_else(|| "/invoke".to_string());
    let health_path = json_string_field(value, "healthPath", "health_path")
        .filter(|path| safe_local_path(path))
        .unwrap_or_else(|| "/healthz".to_string());
    let id = json_string_field(value, "id", "id").unwrap_or_else(|| slug.clone());
    if !safe_config_id(&id) {
        return Err(format!("container pool {slug} has invalid id"));
    }
    let nats_subject = json_string_field(value, "natsSubject", "nats_subject");
    if let Some(subject) = nats_subject.as_deref() {
        if !safe_nats_subject(subject) {
            return Err(format!("container pool {slug} has invalid nats_subject"));
        }
    }
    Ok(PoolConfig {
        id,
        slug: slug.clone(),
        display_name: json_string_field(value, "displayName", "display_name")
            .unwrap_or_else(|| slug.clone()),
        image,
        command: command_vec_from_json(value.get("command").cloned().unwrap_or_else(|| json!([]))),
        env: env_map_from_json(value.get("env").cloned().unwrap_or_else(|| json!({}))),
        request_path,
        health_path,
        container_port: json_u64_field(value, "containerPort", "container_port")
            .and_then(|value| u16::try_from(value).ok())
            .unwrap_or(8080),
        min_warm,
        max_warm,
        max_concurrency_per_container: json_u64_field(
            value,
            "maxConcurrencyPerContainer",
            "max_concurrency_per_container",
        )
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(1)
        .clamp(1, 128),
        request_timeout: Duration::from_millis(
            json_u64_field(value, "requestTimeoutMs", "request_timeout_ms")
                .unwrap_or(30_000)
                .clamp(100, 900_000),
        ),
        idle_ttl: Duration::from_secs(
            json_u64_field(value, "idleTtlSeconds", "idle_ttl_seconds")
                .unwrap_or(900)
                .clamp(10, 86_400),
        ),
        nats_subject,
        read_only: json_bool_field(value, "readOnly", "read_only").unwrap_or(true),
        user: json_string_field(value, "user", "user")
            .filter(|value| {
                value.len() <= 64
                    && value
                        .chars()
                        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, ':' | '_' | '-'))
            })
            .unwrap_or_else(|| "10001:10001".to_string()),
        labels: value.get("labels").cloned().unwrap_or_else(|| json!([])),
    })
}

fn pool_configs_from_app_config_value(value: Value) -> Result<Vec<PoolConfig>, String> {
    let pools = value
        .get("pools")
        .and_then(Value::as_array)
        .ok_or_else(|| "container pool app_config value must contain a pools array".to_string())?;
    pools.iter().map(pool_config_from_json).collect()
}

fn env_map_from_json(value: Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .map(|object| {
            object
                .iter()
                .filter_map(|(key, value)| {
                    if !safe_env_key(key) {
                        return None;
                    }
                    let value = value
                        .as_str()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| value.to_string());
                    safe_env_value(&value).then(|| (key.to_string(), value))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn row_string(row: &tokio_postgres::Row, column: &str) -> String {
    row.try_get::<_, String>(column).unwrap_or_default()
}

fn row_opt_string(row: &tokio_postgres::Row, column: &str) -> Option<String> {
    row.try_get::<_, Option<String>>(column)
        .ok()
        .flatten()
        .filter(|value| !value.trim().is_empty())
}

fn row_i32(row: &tokio_postgres::Row, column: &str, fallback: i32) -> i32 {
    row.try_get::<_, i32>(column).unwrap_or(fallback)
}

fn row_value(row: &tokio_postgres::Row, column: &str, fallback: Value) -> Value {
    row.try_get::<_, Value>(column).unwrap_or(fallback)
}

fn add_rds_root_certificates(root_store: &mut rustls::RootCertStore) -> Result<(), String> {
    let mut reader = BufReader::new(&include_bytes!("../certs/rds-us-east-1-bundle.pem")[..]);
    let mut added = 0usize;

    for cert in rustls_pemfile::certs(&mut reader) {
        let cert = cert.map_err(|error| format!("failed to parse RDS CA certificate: {error}"))?;
        if root_store.add(cert).is_ok() {
            added += 1;
        }
    }

    if added == 0 {
        return Err("no RDS CA certificates loaded".to_string());
    }

    Ok(())
}

fn duration_millis_u64(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn clamp_i32_to_usize(value: i32, fallback: usize, min: usize, max: usize) -> usize {
    usize::try_from(value)
        .ok()
        .filter(|value| *value >= min)
        .unwrap_or(fallback)
        .min(max)
}

fn row_to_pool_config(row: &tokio_postgres::Row) -> Result<PoolConfig, String> {
    let id = row_string(row, "id");
    let slug = row_string(row, "slug");
    if !safe_slug(&slug) {
        return Err(format!("invalid container pool slug: {slug}"));
    }
    let image = row_string(row, "image");
    if !safe_container_image(&image) {
        return Err(format!("container pool {slug} has invalid image"));
    }
    if !safe_config_id(&id) {
        return Err(format!("container pool {slug} has invalid id"));
    }

    let min_warm = clamp_i32_to_usize(row_i32(row, "min_warm", 1), 1, 0, 64);
    let max_warm =
        clamp_i32_to_usize(row_i32(row, "max_warm", 1), min_warm.max(1), 1, 128).max(min_warm);
    let request_timeout_ms = row_i32(row, "request_timeout_ms", 30_000).clamp(100, 900_000);
    let idle_ttl_seconds = row_i32(row, "idle_ttl_seconds", 900).clamp(10, 86_400);
    let container_port = row_i32(row, "container_port", 8080).clamp(1, u16::MAX as i32) as u16;
    let max_concurrency_per_container =
        clamp_i32_to_usize(row_i32(row, "max_concurrency_per_container", 1), 1, 1, 128);
    let request_path = row_opt_string(row, "request_path").unwrap_or_else(|| "/invoke".to_string());
    if !safe_local_path(&request_path) {
        return Err(format!("container pool {slug} has invalid request_path"));
    }
    let health_path = row_opt_string(row, "health_path").unwrap_or_else(|| "/healthz".to_string());
    if !safe_local_path(&health_path) {
        return Err(format!("container pool {slug} has invalid health_path"));
    }

    let nats_subject = row_opt_string(row, "nats_subject");
    if let Some(subject) = nats_subject.as_deref() {
        if !safe_nats_subject(subject) {
            return Err(format!("container pool {slug} has invalid nats_subject"));
        }
    }

    Ok(PoolConfig {
        id,
        slug,
        display_name: row_opt_string(row, "display_name").unwrap_or_else(|| image.clone()),
        image,
        command: command_vec_from_json(row_value(row, "command", json!([]))),
        env: env_map_from_json(row_value(row, "env", json!({}))),
        request_path,
        health_path,
        container_port,
        min_warm,
        max_warm,
        max_concurrency_per_container,
        request_timeout: Duration::from_millis(request_timeout_ms as u64),
        idle_ttl: Duration::from_secs(idle_ttl_seconds as u64),
        nats_subject,
        read_only: true,
        user: "10001:10001".to_string(),
        labels: row_value(row, "labels", json!([])),
    })
}

async fn connect_postgres(config: &ServiceConfig) -> Result<tokio_postgres::Client, String> {
    let database_url = config
        .database_url
        .as_deref()
        .ok_or_else(|| "container pool database URL is not configured".to_string())?;
    let mut root_store = rustls::RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    add_rds_root_certificates(&mut root_store)?;
    let tls_config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    let tls = tokio_postgres_rustls::MakeRustlsConnect::new(tls_config);
    let (client, connection) = tokio_postgres::connect(database_url, tls)
        .await
        .map_err(|error| error.to_string())?;

    tokio::spawn(async move {
        if let Err(error) = connection.await {
            eprintln!("container pool postgres connection error: {error}");
        }
    });
    Ok(client)
}

async fn fetch_pool_configs_from_postgres(
    config: &ServiceConfig,
) -> Result<Vec<PoolConfig>, String> {
    let client = connect_postgres(config).await?;
    match fetch_pool_configs_from_app_config(&client, config).await {
        Ok(Some(configs)) => return Ok(configs),
        Ok(None) => {}
        Err(error) => {
            eprintln!(
                "container pool app_config lookup failed, falling back to container_pool_configs: {error}"
            );
        }
    }
    fetch_pool_configs_from_table(&client).await
}

async fn fetch_pool_configs_from_app_config(
    client: &tokio_postgres::Client,
    config: &ServiceConfig,
) -> Result<Option<Vec<PoolConfig>>, String> {
    let rows = client
        .query(
            r#"
            select value
            from app_config
            where scope = $1
              and key = $2
              and status = 'active'
              and is_soft_deleted = false
            order by updated_at desc
            limit 1
            "#,
            &[&config.app_config_scope, &config.app_config_key],
        )
        .await
        .map_err(|error| error.to_string())?;
    let Some(row) = rows.first() else {
        return Ok(None);
    };
    let value = row_value(row, "value", json!({}));
    let configs = pool_configs_from_app_config_value(value)?;
    Ok(Some(configs))
}

async fn fetch_pool_configs_from_table(
    client: &tokio_postgres::Client,
) -> Result<Vec<PoolConfig>, String> {
    let rows = client
        .query(
            r#"
            select
              id::text as id,
              slug,
              display_name,
              image,
              command,
              env,
              request_path,
              health_path,
              container_port,
              min_warm,
              max_warm,
              max_concurrency_per_container,
              request_timeout_ms,
              idle_ttl_seconds,
              nats_subject,
              labels
            from container_pool_configs
            where status = 'active'
              and is_soft_deleted = false
            order by slug asc
            "#,
            &[],
        )
        .await
        .map_err(|error| error.to_string())?;

    let mut configs = Vec::with_capacity(rows.len());
    for row in rows {
        configs.push(row_to_pool_config(&row)?);
    }
    Ok(configs)
}

fn fallback_pool_configs_from_env() -> Result<Vec<PoolConfig>, String> {
    let Some(raw) = first_env(&["CONTAINER_POOL_CONFIG_JSON"]) else {
        return Ok(Vec::new());
    };
    let value = serde_json::from_str::<Value>(&raw).map_err(|error| error.to_string())?;
    let items = value
        .as_array()
        .ok_or_else(|| "CONTAINER_POOL_CONFIG_JSON must be a JSON array".to_string())?;
    let mut configs = Vec::with_capacity(items.len());
    for item in items {
        configs.push(pool_config_from_json(item)?);
    }
    Ok(configs)
}

async fn fetch_pool_configs(config: &ServiceConfig) -> Result<Vec<PoolConfig>, String> {
    if config.database_url.is_some() {
        fetch_pool_configs_from_postgres(config).await
    } else {
        fallback_pool_configs_from_env()
    }
}

async fn refresh_pool_configs(state: &AppState) -> Result<(), String> {
    let configs = fetch_pool_configs(&state.config).await?;
    let mut next_configs = HashMap::new();
    let mut next_slugs = HashMap::new();
    for config in configs {
        next_slugs.insert(config.slug.clone(), config.id.clone());
        next_configs.insert(config.id.clone(), config);
    }

    let removed_names = {
        let mut registry = state.registry.lock().await;
        let removed_pool_ids = registry
            .configs
            .keys()
            .filter(|pool_id| !next_configs.contains_key(*pool_id))
            .cloned()
            .collect::<HashSet<_>>();
        let removed_names = registry
            .containers
            .values()
            .filter(|container| removed_pool_ids.contains(&container.pool_id))
            .map(|container| container.name.clone())
            .collect::<Vec<_>>();
        for name in &removed_names {
            registry.containers.remove(name);
            remove_affinity_for_container(&mut registry, name);
        }
        registry.configs = next_configs;
        registry.slug_to_id = next_slugs;
        if registry.next_port == 0 {
            registry.next_port = state.config.port_start;
        }
        registry.last_config_error = None;
        registry.last_config_refresh_ms = Some(now_ms());
        removed_names
    };

    state
        .metrics
        .config_refresh_total
        .fetch_add(1, Ordering::Relaxed);
    for name in removed_names {
        if let Err(error) = remove_container(state, &name).await {
            eprintln!("failed to remove container for deleted pool {name}: {error}");
        }
    }
    Ok(())
}

async fn record_config_error(state: &AppState, error: String) {
    state
        .metrics
        .config_refresh_failures_total
        .fetch_add(1, Ordering::Relaxed);
    let mut registry = state.registry.lock().await;
    registry.last_config_error = Some(error);
}

async fn allocate_container_slot(
    state: &AppState,
    pool_id: &str,
) -> Result<(PoolConfig, WarmContainer), String> {
    retire_stale_starting_containers(state, Some(pool_id)).await;

    let mut registry = state.registry.lock().await;
    let pool = registry
        .configs
        .get(pool_id)
        .cloned()
        .ok_or_else(|| format!("unknown container pool: {pool_id}"))?;
    let active = registry
        .containers
        .values()
        .filter(|container| container.pool_id == pool.id)
        .count();
    if active >= pool.max_warm {
        return Err(format!(
            "container pool {} is at max capacity ({})",
            pool.slug, pool.max_warm
        ));
    }

    let used_ports = registry
        .containers
        .values()
        .map(|container| container.port)
        .collect::<HashSet<_>>();
    let mut port = registry.next_port.max(state.config.port_start);
    let mut scanned = 0u32;
    while used_ports.contains(&port) {
        port = if port >= state.config.port_end {
            state.config.port_start
        } else {
            port + 1
        };
        scanned += 1;
        if scanned > u32::from(state.config.port_end - state.config.port_start) + 1 {
            return Err("container pool port range is exhausted".to_string());
        }
    }
    registry.next_port = if port >= state.config.port_end {
        state.config.port_start
    } else {
        port + 1
    };

    let name = format!(
        "dd-pool-{}-{}-{}",
        pool.slug,
        port,
        now_ms() % 1_000_000_000
    );
    let now = now_ms();
    let container = WarmContainer {
        name: name.clone(),
        pool_id: pool.id.clone(),
        pool_slug: pool.slug.clone(),
        affinity_key: None,
        port,
        status: ContainerStatus::Starting,
        in_flight: 0,
        launched_at_ms: now,
        last_used_at_ms: now,
        last_health_at_ms: None,
        last_healthy_at_ms: None,
        health_failure_count: 0,
        last_health_error: None,
        request_count: 0,
        failure_count: 0,
    };
    registry.containers.insert(name, container.clone());
    Ok((pool, container))
}

async fn start_one_for_pool(state: &AppState, pool_id: &str) -> Result<WarmContainer, String> {
    let (pool, mut container) = allocate_container_slot(state, pool_id).await?;
    let mut args = vec![
        "-n".to_string(),
        state.config.containerd_namespace.clone(),
        "run".to_string(),
        "-d".to_string(),
        "--name".to_string(),
        container.name.clone(),
        "--label".to_string(),
        "dd.container-pool.managed=true".to_string(),
        "--label".to_string(),
        format!("dd.container-pool.pool={}", pool.slug),
        "--label".to_string(),
        format!("dd.container-pool.pool-id={}", pool.id),
        "--label".to_string(),
        format!("dd.container-pool.service={SERVICE_NAME}"),
        "--user".to_string(),
        pool.user.clone(),
        "--cap-drop".to_string(),
        "ALL".to_string(),
        "--security-opt".to_string(),
        "no-new-privileges".to_string(),
        "--pids-limit".to_string(),
        state.config.pids_limit.to_string(),
        "--ulimit".to_string(),
        format!("nofile={limit}:{limit}", limit = state.config.nofile_limit),
    ];
    if pool.read_only {
        args.push("--read-only".to_string());
        args.push("--tmpfs".to_string());
        args.push("/tmp:rw,noexec,nosuid,size=64m".to_string());
    }
    if let Some(memory) = state.config.container_memory.as_deref() {
        args.push("--memory".to_string());
        args.push(memory.to_string());
    }
    if let Some(cpus) = state.config.container_cpus.as_deref() {
        args.push("--cpus".to_string());
        args.push(cpus.to_string());
    }
    if let Some(pull_policy) = state.config.pull_policy.as_deref() {
        args.push("--pull".to_string());
        args.push(pull_policy.to_string());
    }

    if state.config.network == "host" {
        args.push("--network".to_string());
        args.push("host".to_string());
        args.push("--env".to_string());
        args.push(format!("PORT={}", container.port));
    } else {
        args.push("--network".to_string());
        args.push(state.config.network.clone());
        args.push("--publish".to_string());
        args.push(format!(
            "127.0.0.1:{}:{}",
            container.port, pool.container_port
        ));
        args.push("--env".to_string());
        args.push(format!("PORT={}", pool.container_port));
    }

    let mut container_env = pool.env.clone();
    container_env
        .entry("DD_POOL_ID".to_string())
        .or_insert_with(|| pool.id.clone());
    container_env
        .entry("DD_POOL_SLUG".to_string())
        .or_insert_with(|| pool.slug.clone());
    container_env
        .entry("DD_POOL_CONTAINER_NAME".to_string())
        .or_insert_with(|| container.name.clone());
    container_env
        .entry("DD_POOL_MANAGER".to_string())
        .or_insert_with(|| SERVICE_NAME.to_string());
    container_env
        .entry("DD_POOL_REQUEST_PATH".to_string())
        .or_insert_with(|| pool.request_path.clone());
    container_env
        .entry("DD_POOL_HEALTH_PATH".to_string())
        .or_insert_with(|| pool.health_path.clone());
    container_env
        .entry("DD_POOL_CONTAINER_PORT".to_string())
        .or_insert_with(|| pool.container_port.to_string());
    container_env
        .entry("DD_POOL_MAX_BODY_BYTES".to_string())
        .or_insert_with(|| MAX_HTTP_BODY_BYTES.to_string());
    container_env
        .entry("DD_POOL_HANDLER_TIMEOUT_SECONDS".to_string())
        .or_insert_with(|| pool.request_timeout.as_secs().max(1).to_string());
    if let Some(nats_url) = state.config.nats_url.as_deref() {
        container_env
            .entry("NATS_URL".to_string())
            .or_insert_with(|| nats_url.to_string());
        container_env
            .entry("DD_POOL_NATS_EVENT_SUBJECT".to_string())
            .or_insert_with(|| format!("dd.remote.container_pool.{}.events", pool.slug));
        container_env
            .entry("DD_POOL_NATS_HEARTBEAT_SUBJECT".to_string())
            .or_insert_with(|| format!("dd.remote.container_pool.{}.heartbeats", pool.slug));
    }
    for key in &state.config.forward_env_keys {
        if container_env.contains_key(key) {
            continue;
        }
        if let Ok(value) = env::var(key) {
            if !value.is_empty() {
                container_env.insert(key.clone(), value);
            }
        }
    }

    for (key, value) in &container_env {
        args.push("--env".to_string());
        args.push(format!("{key}={value}"));
    }
    args.push(pool.image.clone());
    args.extend(pool.command.clone());

    let container_run_timeout = state.config.command_timeout.min(Duration::from_secs(30));
    match run_command(&state.config.nerdctl_bin, &args, container_run_timeout).await {
        Ok(_) => {
            if let Err(error) = wait_container_ready(state, &pool, &container).await {
                let mut registry = state.registry.lock().await;
                registry.containers.remove(&container.name);
                remove_affinity_for_container(&mut registry, &container.name);
                drop(registry);
                if let Err(remove_error) = remove_container(state, &container.name).await {
                    eprintln!(
                        "failed to remove unready warm container {}: {remove_error}",
                        container.name
                    );
                }
                return Err(error);
            }
            state
                .metrics
                .containers_started_total
                .fetch_add(1, Ordering::Relaxed);
            container.status = ContainerStatus::Idle;
            let mut registry = state.registry.lock().await;
            if let Some(stored) = registry.containers.get_mut(&container.name) {
                stored.status = ContainerStatus::Idle;
                stored.last_health_at_ms = Some(now_ms());
                stored.last_healthy_at_ms = Some(now_ms());
                stored.health_failure_count = 0;
                stored.last_health_error = None;
            }
            Ok(container)
        }
        Err(error) => {
            let mut registry = state.registry.lock().await;
            registry.containers.remove(&container.name);
            remove_affinity_for_container(&mut registry, &container.name);
            Err(error)
        }
    }
}

async fn wait_container_ready(
    state: &AppState,
    pool: &PoolConfig,
    container: &WarmContainer,
) -> Result<(), String> {
    let url = target_url(container, &pool.health_path);
    let started = tokio::time::Instant::now();
    loop {
        match timeout(Duration::from_millis(800), state.http.get(&url).send()).await {
            Ok(Ok(response)) if response.status().is_success() => return Ok(()),
            Ok(Ok(_)) | Ok(Err(_)) | Err(_) => {}
        }
        if !inspect_container_running(state, &container.name).await? {
            return Err(format!(
                "container {} stopped before readiness at {url}",
                container.name
            ));
        }
        if started.elapsed() > state.config.container_start_timeout {
            return Err(format!(
                "container {} readiness timed out at {url}",
                container.name
            ));
        }
        sleep(Duration::from_millis(200)).await;
    }
}

async fn remove_container(state: &AppState, name: &str) -> Result<(), String> {
    let args = vec![
        "-n".to_string(),
        state.config.containerd_namespace.clone(),
        "rm".to_string(),
        "-f".to_string(),
        name.to_string(),
    ];
    run_command(
        &state.config.nerdctl_bin,
        &args,
        state.config.command_timeout,
    )
    .await?;
    state
        .metrics
        .containers_removed_total
        .fetch_add(1, Ordering::Relaxed);
    Ok(())
}

async fn cleanup_managed_containers_on_start(state: &AppState) -> Result<(), String> {
    if !state.config.cleanup_on_start {
        return Ok(());
    }
    let list_args = vec![
        "-n".to_string(),
        state.config.containerd_namespace.clone(),
        "ps".to_string(),
        "-a".to_string(),
        "-q".to_string(),
        "--filter".to_string(),
        "label=dd.container-pool.managed=true".to_string(),
    ];
    let output = run_command(
        &state.config.nerdctl_bin,
        &list_args,
        state.config.command_timeout,
    )
    .await?;
    for id in output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if let Err(error) = remove_container(state, id).await {
            eprintln!("failed to remove stale managed container {id}: {error}");
        }
    }
    Ok(())
}

async fn run_command(
    program: &str,
    args: &[String],
    command_timeout: Duration,
) -> Result<String, String> {
    let output = timeout(command_timeout, Command::new(program).args(args).output())
        .await
        .map_err(|_| format!("{program} timed out after {}s", command_timeout.as_secs()))?
        .map_err(|error| format!("{program} failed to start: {error}"))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).to_string());
    }
    let stderr = String::from_utf8_lossy(&output.stderr)
        .chars()
        .take(2000)
        .collect::<String>();
    Err(format!(
        "{program} exited with {}: {stderr}",
        output.status.code().unwrap_or(-1)
    ))
}

async fn inspect_container_running(state: &AppState, name: &str) -> Result<bool, String> {
    let inspect_timeout = state.config.command_timeout.min(Duration::from_secs(5));
    let args = vec![
        "-n".to_string(),
        state.config.containerd_namespace.clone(),
        "inspect".to_string(),
        name.to_string(),
    ];
    let output = match run_command(&state.config.nerdctl_bin, &args, inspect_timeout).await {
        Ok(output) => output,
        Err(error) => {
            let lower = error.to_ascii_lowercase();
            if lower.contains("not found") || lower.contains("no such") {
                return Ok(false);
            }
            return Err(error);
        }
    };
    let value = serde_json::from_str::<Value>(&output).map_err(|error| error.to_string())?;
    let Some(container) = value
        .as_array()
        .and_then(|items| items.first())
        .or(Some(&value))
    else {
        return Ok(false);
    };
    if let Some(running) = container
        .pointer("/State/Running")
        .and_then(Value::as_bool)
        .or_else(|| container.pointer("/State/running").and_then(Value::as_bool))
    {
        return Ok(running);
    }
    let status = container
        .pointer("/State/Status")
        .or_else(|| container.pointer("/State/status"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    Ok(status.eq_ignore_ascii_case("running"))
}

async fn retire_stale_starting_containers(state: &AppState, pool_id: Option<&str>) {
    let now = now_ms();
    let candidates = {
        let registry = state.registry.lock().await;
        registry
            .containers
            .values()
            .filter(|container| container.in_flight == 0)
            .filter(|container| container.status == ContainerStatus::Starting)
            .filter(|container| pool_id.map(|id| id == container.pool_id).unwrap_or(true))
            .filter(|container| {
                Duration::from_millis(now.saturating_sub(container.launched_at_ms) as u64)
                    >= state.config.unhealthy_grace
            })
            .map(|container| container.name.clone())
            .collect::<Vec<_>>()
    };

    for name in candidates {
        match inspect_container_running(state, &name).await {
            Ok(false) => {
                retire_container(state, &name, "starting container is not running").await;
            }
            Ok(true) => {}
            Err(error) => {
                eprintln!("failed to inspect starting warm container {name}: {error}");
            }
        }
    }
}

async fn probe_container_health(
    state: &AppState,
    pool: &PoolConfig,
    container: &WarmContainer,
) -> Result<(), String> {
    state
        .metrics
        .container_health_checks_total
        .fetch_add(1, Ordering::Relaxed);
    if !inspect_container_running(state, &container.name).await? {
        return Err("container is not running".to_string());
    }
    let url = target_url(container, &pool.health_path);
    match timeout(
        state.config.health_check_timeout,
        state.http.get(&url).send(),
    )
    .await
    {
        Ok(Ok(response)) if response.status().is_success() => Ok(()),
        Ok(Ok(response)) => Err(format!("health check returned {}", response.status())),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err(format!(
            "health check timed out after {}ms",
            duration_millis_u64(state.config.health_check_timeout)
        )),
    }
}

async fn retire_container(state: &AppState, name: &str, reason: &str) {
    let removed = {
        let mut registry = state.registry.lock().await;
        if let Some(container) = registry.containers.get_mut(name) {
            container.status = ContainerStatus::Unhealthy;
            container.last_health_at_ms = Some(now_ms());
            container.last_health_error = Some(reason.chars().take(512).collect());
        }
        let removed = registry.containers.remove(name).is_some();
        if removed {
            remove_affinity_for_container(&mut registry, name);
        }
        removed
    };
    if removed {
        state
            .metrics
            .containers_unhealthy_total
            .fetch_add(1, Ordering::Relaxed);
        if let Err(error) = remove_container(state, name).await {
            eprintln!("failed to remove unhealthy warm container {name}: {error}");
        }
    }
}

async fn prune_unhealthy_containers(state: &AppState) {
    retire_stale_starting_containers(state, None).await;

    let now = now_ms();
    let candidates = {
        let registry = state.registry.lock().await;
        registry
            .containers
            .values()
            .filter(|container| container.in_flight == 0)
            .filter(|container| {
                !matches!(
                    container.status,
                    ContainerStatus::Starting | ContainerStatus::Draining
                )
            })
            .filter(|container| {
                container
                    .last_health_at_ms
                    .map(|last| {
                        Duration::from_millis(now.saturating_sub(last) as u64)
                            >= state.config.health_check_interval
                    })
                    .unwrap_or(true)
            })
            .filter_map(|container| {
                registry
                    .configs
                    .get(&container.pool_id)
                    .cloned()
                    .map(|pool| (pool, container.clone()))
            })
            .collect::<Vec<_>>()
    };

    for (pool, container) in candidates {
        let checked_at = now_ms();
        match probe_container_health(state, &pool, &container).await {
            Ok(()) => {
                let mut registry = state.registry.lock().await;
                if let Some(stored) = registry.containers.get_mut(&container.name) {
                    stored.status = ContainerStatus::Idle;
                    stored.last_health_at_ms = Some(checked_at);
                    stored.last_healthy_at_ms = Some(checked_at);
                    stored.health_failure_count = 0;
                    stored.last_health_error = None;
                }
            }
            Err(error) => {
                state
                    .metrics
                    .container_health_check_failures_total
                    .fetch_add(1, Ordering::Relaxed);
                let should_retire = {
                    let mut registry = state.registry.lock().await;
                    let Some(stored) = registry.containers.get_mut(&container.name) else {
                        continue;
                    };
                    stored.last_health_at_ms = Some(checked_at);
                    stored.health_failure_count = stored.health_failure_count.saturating_add(1);
                    stored.last_health_error = Some(error.chars().take(512).collect());
                    stored.status = ContainerStatus::Unhealthy;
                    let age = Duration::from_millis(
                        checked_at.saturating_sub(stored.launched_at_ms) as u64,
                    );
                    stored.in_flight == 0
                        && age >= state.config.unhealthy_grace
                        && stored.health_failure_count >= state.config.unhealthy_failure_threshold
                };
                if should_retire {
                    retire_container(state, &container.name, "health check failed").await;
                }
            }
        }
    }
}

async fn reconcile_pool(state: &AppState, pool_id: &str) -> Result<(), String> {
    loop {
        let deficit = {
            let registry = state.registry.lock().await;
            let Some(pool) = registry.configs.get(pool_id) else {
                return Ok(());
            };
            let active = registry
                .containers
                .values()
                .filter(|container| container.pool_id == pool.id)
                .count();
            let available_capacity = registry
                .containers
                .values()
                .filter(|container| container.pool_id == pool.id)
                .filter(|container| {
                    !matches!(
                        container.status,
                        ContainerStatus::Starting
                            | ContainerStatus::Draining
                            | ContainerStatus::Unhealthy
                    )
                })
                .map(|container| {
                    pool.max_concurrency_per_container
                        .saturating_sub(container.in_flight)
                })
                .sum::<usize>();
            let capacity_deficit = pool.min_warm.saturating_sub(available_capacity);
            capacity_deficit.min(pool.max_warm.saturating_sub(active))
        };
        if deficit == 0 {
            break;
        }
        start_one_for_pool(state, pool_id).await?;
    }
    Ok(())
}

async fn reconcile_all(state: &AppState) {
    prune_unhealthy_containers(state).await;

    let pool_ids = {
        let registry = state.registry.lock().await;
        registry.configs.keys().cloned().collect::<Vec<_>>()
    };
    for pool_id in pool_ids {
        if let Err(error) = reconcile_pool(state, &pool_id).await {
            eprintln!("container pool reconcile failed for {pool_id}: {error}");
        }
    }

    let stale = {
        let mut registry = state.registry.lock().await;
        let mut per_pool_count = HashMap::<String, usize>::new();
        for container in registry.containers.values() {
            *per_pool_count.entry(container.pool_id.clone()).or_default() += 1;
        }
        let now = now_ms();
        let mut stale = Vec::new();
        let mut names = registry.containers.keys().cloned().collect::<Vec<_>>();
        names.sort();
        for name in names {
            let Some(container) = registry.containers.get(&name) else {
                continue;
            };
            if container.status == ContainerStatus::Busy || container.in_flight > 0 {
                continue;
            }
            let Some(pool) = registry.configs.get(&container.pool_id) else {
                stale.push(name.clone());
                continue;
            };
            let count = per_pool_count.get(&container.pool_id).copied().unwrap_or(0);
            let idle_for = Duration::from_millis((now - container.last_used_at_ms) as u64);
            if count > pool.max_warm || (count > pool.min_warm && idle_for > pool.idle_ttl) {
                stale.push(name.clone());
                if let Some(value) = per_pool_count.get_mut(&container.pool_id) {
                    *value = value.saturating_sub(1);
                }
            }
        }
        for name in &stale {
            if let Some(container) = registry.containers.get_mut(name) {
                container.status = ContainerStatus::Draining;
            }
            registry.containers.remove(name);
            remove_affinity_for_container(&mut registry, name);
        }
        stale
    };
    for name in stale {
        if let Err(error) = remove_container(state, &name).await {
            eprintln!("failed to remove stale warm container {name}: {error}");
        }
    }
}

fn pool_id_from_selector(registry: &PoolRegistry, selector: &str) -> Option<String> {
    if registry.configs.contains_key(selector) {
        Some(selector.to_string())
    } else {
        registry.slug_to_id.get(selector).cloned()
    }
}

fn normalized_affinity_key(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    let mut output = String::new();
    for ch in value.chars() {
        if output.len() >= 256 {
            break;
        }
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':') {
            output.push(ch);
        } else {
            output.push('-');
        }
    }
    let output = output.trim_matches('-').to_string();
    (!output.is_empty()).then_some(output)
}

fn affinity_map_key(pool_id: &str, affinity_key: &str) -> String {
    format!("{pool_id}:{affinity_key}")
}

fn remove_affinity_for_container(registry: &mut PoolRegistry, container_name: &str) {
    registry
        .affinity
        .retain(|_, mapped_name| mapped_name != container_name);
}

fn container_can_accept(pool: &PoolConfig, container: &WarmContainer) -> bool {
    !matches!(
        container.status,
        ContainerStatus::Starting | ContainerStatus::Draining | ContainerStatus::Unhealthy
    ) && container.in_flight < pool.max_concurrency_per_container
}

async fn lease_container(
    state: &AppState,
    selector: &str,
    affinity_key: Option<&str>,
) -> Result<ContainerLease, String> {
    let affinity_key = normalized_affinity_key(affinity_key);
    let pool_id = {
        let registry = state.registry.lock().await;
        pool_id_from_selector(&registry, selector)
            .ok_or_else(|| format!("unknown container pool: {selector}"))?
    };

    for _ in 0..2 {
        let maybe_lease = {
            let mut registry = state.registry.lock().await;
            let pool = registry
                .configs
                .get(&pool_id)
                .cloned()
                .ok_or_else(|| format!("unknown container pool: {selector}"))?;
            let candidate_name = if let Some(affinity_key) = affinity_key.as_deref() {
                let map_key = affinity_map_key(&pool.id, affinity_key);
                let mapped_name = registry.affinity.get(&map_key).cloned();
                let mapped_candidate = match mapped_name
                    .as_deref()
                    .and_then(|name| registry.containers.get(name))
                {
                    Some(container)
                        if container.pool_id == pool.id
                            && container_can_accept(&pool, container) =>
                    {
                        Some(container.name.clone())
                    }
                    Some(container)
                        if container.pool_id == pool.id
                            && !matches!(
                                container.status,
                                ContainerStatus::Draining | ContainerStatus::Unhealthy
                            ) =>
                    {
                        return Err(format!(
                                "affinity container {} for key {} is not ready (status {:?}, inFlight {})",
                                container.name, affinity_key, container.status, container.in_flight
                            ));
                    }
                    _ => None,
                };
                if mapped_candidate.is_none() {
                    if let Some(mapped_name) = mapped_name {
                        let clear_mapping = registry
                            .containers
                            .get(&mapped_name)
                            .map(|container| {
                                container.pool_id != pool.id
                                    || matches!(
                                        container.status,
                                        ContainerStatus::Draining | ContainerStatus::Unhealthy
                                    )
                            })
                            .unwrap_or(true);
                        if clear_mapping {
                            registry.affinity.remove(&map_key);
                        }
                    }
                }
                mapped_candidate.or_else(|| {
                    registry
                        .containers
                        .values()
                        .filter(|container| container.pool_id == pool.id)
                        .filter(|container| container_can_accept(&pool, container))
                        .filter(|container| {
                            container
                                .affinity_key
                                .as_deref()
                                .map(|bound| bound == affinity_key)
                                .unwrap_or(true)
                        })
                        .min_by_key(|container| {
                            (
                                container.affinity_key.as_deref() != Some(affinity_key),
                                container.in_flight,
                                container.last_used_at_ms,
                            )
                        })
                        .map(|container| container.name.clone())
                })
            } else {
                registry
                    .containers
                    .values()
                    .filter(|container| container.pool_id == pool.id)
                    .filter(|container| container_can_accept(&pool, container))
                    .min_by_key(|container| (container.in_flight, container.last_used_at_ms))
                    .map(|container| container.name.clone())
            };
            candidate_name.and_then(|name| {
                let affinity = affinity_key.clone();
                if let Some(affinity_key) = affinity.as_deref() {
                    let map_key = affinity_map_key(&pool.id, affinity_key);
                    registry.affinity.insert(map_key, name.clone());
                }
                let container = registry.containers.get_mut(&name)?;
                if let Some(affinity_key) = affinity {
                    container.affinity_key = Some(affinity_key);
                }
                container.in_flight += 1;
                container.status = ContainerStatus::Busy;
                container.request_count += 1;
                container.last_used_at_ms = now_ms();
                Some(ContainerLease {
                    pool,
                    container: container.clone(),
                })
            })
        };
        if let Some(lease) = maybe_lease {
            return Ok(lease);
        }
        start_one_for_pool(state, &pool_id).await?;
    }

    Err(format!("no warm container available for pool {selector}"))
}

async fn release_container(state: &AppState, container_name: &str, failed: bool) {
    let mut registry = state.registry.lock().await;
    if let Some(container) = registry.containers.get_mut(container_name) {
        container.in_flight = container.in_flight.saturating_sub(1);
        if failed {
            container.failure_count += 1;
        }
        container.status = if container.in_flight == 0 {
            ContainerStatus::Idle
        } else {
            ContainerStatus::Busy
        };
        container.last_used_at_ms = now_ms();
    }
}

fn safe_dispatch_path(path: Option<&str>, fallback: &str) -> Result<String, String> {
    let path = path
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback);
    if !safe_local_path(path) {
        return Err("dispatch path must be a local absolute path".to_string());
    }
    Ok(path.to_string())
}

fn dispatch_body(request: &DispatchRequest) -> Value {
    request
        .payload
        .clone()
        .or_else(|| request.body.clone())
        .unwrap_or_else(|| json!({}))
}

fn request_id_from_request(request: &DispatchRequest) -> String {
    request
        .request_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("container-pool-request")
        .chars()
        .take(128)
        .collect()
}

fn target_url(container: &WarmContainer, path: &str) -> String {
    format!("http://127.0.0.1:{}{path}", container.port)
}

fn apply_forward_headers(
    mut builder: reqwest::RequestBuilder,
    headers: Option<&BTreeMap<String, String>>,
) -> reqwest::RequestBuilder {
    let Some(headers) = headers else {
        return builder;
    };
    for (key, value) in headers.iter().take(32) {
        if key.len() > 64 || value.len() > 8192 {
            continue;
        }
        let lower = key.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "authorization"
                | "cookie"
                | "host"
                | "connection"
                | "content-length"
                | "te"
                | "trailer"
                | "transfer-encoding"
                | "upgrade"
                | "x-agent-auth"
                | "x-container-pool-auth"
                | "x-server-auth"
        ) || lower.starts_with("proxy-")
        {
            continue;
        }
        let Ok(name) = HeaderName::from_bytes(key.as_bytes()) else {
            continue;
        };
        let Ok(value) = HeaderValue::from_str(value) else {
            continue;
        };
        builder = builder.header(name, value);
    }
    builder
}

async fn read_limited_response_body(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(format!(
            "container response exceeded configured byte limit ({max_bytes})"
        ));
    }

    let body = response.bytes().await.map_err(|error| error.to_string())?;
    if body.len() > max_bytes {
        return Err(format!(
            "container response exceeded configured byte limit ({max_bytes})"
        ));
    }
    Ok(body.to_vec())
}

async fn dispatch_to_pool(
    state: &AppState,
    selector: &str,
    request: DispatchRequest,
) -> Result<DispatchResponse, String> {
    let started = now_ms();
    let affinity_key = normalized_affinity_key(request.affinity_key.as_deref());
    let lease = lease_container(state, selector, affinity_key.as_deref()).await?;
    let path = match safe_dispatch_path(request.path.as_deref(), &lease.pool.request_path) {
        Ok(path) => path,
        Err(error) => {
            release_container(state, &lease.container.name, true).await;
            state
                .metrics
                .dispatch_failures_total
                .fetch_add(1, Ordering::Relaxed);
            return Err(error);
        }
    };
    let url = target_url(&lease.container, &path);
    let body = dispatch_body(&request);
    let request_id = request_id_from_request(&request);
    let backfill_state = state.clone();
    let backfill_pool_id = lease.pool.id.clone();
    tokio::spawn(async move {
        if let Err(error) = reconcile_pool(&backfill_state, &backfill_pool_id).await {
            eprintln!("container pool backfill failed for {backfill_pool_id}: {error}");
        }
    });

    let send = apply_forward_headers(state.http.post(&url), request.headers.as_ref()).json(&body);
    let response = timeout(lease.pool.request_timeout, send.send()).await;
    let mut retire_reason = None::<String>;
    let result = match response {
        Ok(Ok(response)) => {
            let status = response.status();
            match read_limited_response_body(response, state.config.worker_response_max_bytes).await
            {
                Ok(bytes) => {
                    let body = serde_json::from_slice::<Value>(&bytes).unwrap_or_else(|_| {
                        json!({
                            "text": String::from_utf8_lossy(&bytes).chars().take(256 * 1024).collect::<String>()
                        })
                    });
                    Ok(DispatchResponse {
                        ok: status.is_success(),
                        request_id,
                        pool_id: lease.pool.id.clone(),
                        pool_slug: lease.pool.slug.clone(),
                        affinity_key: affinity_key.clone(),
                        container_name: lease.container.name.clone(),
                        container_port: lease.container.port,
                        target_url: url,
                        status: status.as_u16(),
                        body,
                        elapsed_ms: now_ms().saturating_sub(started),
                    })
                }
                Err(error) => {
                    let message = error.to_string();
                    retire_reason = Some(message.clone());
                    Err(message)
                }
            }
        }
        Ok(Err(error)) => {
            let message = error.to_string();
            retire_reason = Some(message.clone());
            Err(message)
        }
        Err(_) => {
            let message = format!(
                "container dispatch timed out after {}ms",
                duration_millis_u64(lease.pool.request_timeout)
            );
            retire_reason = Some(message.clone());
            Err(message)
        }
    };

    let failed = result.as_ref().map(|response| !response.ok).unwrap_or(true);
    if let Some(reason) = retire_reason.as_deref() {
        retire_container(state, &lease.container.name, reason).await;
    } else {
        release_container(state, &lease.container.name, failed).await;
    }
    if failed {
        state
            .metrics
            .dispatch_failures_total
            .fetch_add(1, Ordering::Relaxed);
    } else {
        state.metrics.dispatch_total.fetch_add(1, Ordering::Relaxed);
    }
    if retire_reason.is_some() {
        let refill_state = state.clone();
        let refill_pool_id = lease.pool.id.clone();
        tokio::spawn(async move {
            if let Err(error) = reconcile_pool(&refill_state, &refill_pool_id).await {
                eprintln!("container pool refill failed for {refill_pool_id}: {error}");
            }
        });
    }
    result
}

fn pool_selector_from_request(
    request: &DispatchRequest,
    subject: Option<&str>,
    state: &PoolRegistry,
) -> Option<String> {
    request
        .pool_id
        .as_deref()
        .or(request.pool_slug.as_deref())
        .map(ToString::to_string)
        .or_else(|| {
            subject.and_then(|subject| {
                state
                    .configs
                    .values()
                    .find(|config| config.nats_subject.as_deref() == Some(subject))
                    .map(|config| config.id.clone())
            })
        })
}

fn pool_summary(config: &PoolConfig, containers: Vec<WarmContainer>) -> PoolSummary {
    let idle_containers = containers
        .iter()
        .filter(|container| container.status == ContainerStatus::Idle)
        .count();
    let busy_containers = containers
        .iter()
        .filter(|container| container.status == ContainerStatus::Busy)
        .count();
    let unhealthy_containers = containers
        .iter()
        .filter(|container| container.status == ContainerStatus::Unhealthy)
        .count();
    PoolSummary {
        id: config.id.clone(),
        slug: config.slug.clone(),
        display_name: config.display_name.clone(),
        image: config.image.clone(),
        request_path: config.request_path.clone(),
        health_path: config.health_path.clone(),
        container_port: config.container_port,
        min_warm: config.min_warm,
        max_warm: config.max_warm,
        max_concurrency_per_container: config.max_concurrency_per_container,
        request_timeout_ms: duration_millis_u64(config.request_timeout),
        idle_ttl_seconds: config.idle_ttl.as_secs(),
        nats_subject: config.nats_subject.clone(),
        env_keys: config.env.keys().cloned().collect(),
        labels: config.labels.clone(),
        active_containers: containers.len(),
        idle_containers,
        busy_containers,
        unhealthy_containers,
        containers,
    }
}

async fn pool_summaries(state: &AppState) -> Vec<PoolSummary> {
    let registry = state.registry.lock().await;
    let mut pools = registry
        .configs
        .values()
        .map(|config| {
            let mut containers = registry
                .containers
                .values()
                .filter(|container| container.pool_id == config.id)
                .cloned()
                .collect::<Vec<_>>();
            containers.sort_by(|a, b| a.name.cmp(&b.name));
            pool_summary(config, containers)
        })
        .collect::<Vec<_>>();
    pools.sort_by(|a, b| a.slug.cmp(&b.slug));
    pools
}

async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    let registry = state.registry.lock().await;
    Json(HealthResponse {
        ok: true,
        service: SERVICE_NAME,
        postgres_configured: state.config.database_url.is_some(),
        nats_configured: state.nats.is_some(),
        auth_configured: state.config.server_auth_secret.is_some(),
        pool_count: registry.configs.len(),
        warm_container_count: registry.containers.len(),
        last_config_refresh_ms: registry.last_config_refresh_ms,
        last_config_error: registry.last_config_error.clone(),
    })
}

async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    let registry = state.registry.lock().await;
    let warm = registry.containers.len();
    let idle = registry
        .containers
        .values()
        .filter(|container| container.status == ContainerStatus::Idle)
        .count();
    let busy = registry
        .containers
        .values()
        .filter(|container| container.status == ContainerStatus::Busy)
        .count();
    let unhealthy = registry
        .containers
        .values()
        .filter(|container| container.status == ContainerStatus::Unhealthy)
        .count();
    let body = format!(
        "# HELP dd_container_pool_http_requests_total HTTP requests observed by dd-container-pool.\n\
         # TYPE dd_container_pool_http_requests_total counter\n\
         dd_container_pool_http_requests_total {}\n\
         # HELP dd_container_pool_dispatch_total Successful container pool dispatches.\n\
         # TYPE dd_container_pool_dispatch_total counter\n\
         dd_container_pool_dispatch_total {}\n\
         # HELP dd_container_pool_dispatch_failures_total Failed container pool dispatches.\n\
         # TYPE dd_container_pool_dispatch_failures_total counter\n\
         dd_container_pool_dispatch_failures_total {}\n\
         # HELP dd_container_pool_nats_messages_total NATS messages received by the pool service.\n\
         # TYPE dd_container_pool_nats_messages_total counter\n\
         dd_container_pool_nats_messages_total {}\n\
         # HELP dd_container_pool_nats_failures_total NATS dispatch failures.\n\
         # TYPE dd_container_pool_nats_failures_total counter\n\
         dd_container_pool_nats_failures_total {}\n\
         # HELP dd_container_pool_containers_started_total Warm containers started.\n\
         # TYPE dd_container_pool_containers_started_total counter\n\
         dd_container_pool_containers_started_total {}\n\
         # HELP dd_container_pool_containers_removed_total Warm containers removed.\n\
         # TYPE dd_container_pool_containers_removed_total counter\n\
         dd_container_pool_containers_removed_total {}\n\
         # HELP dd_container_pool_containers_unhealthy_total Warm containers retired as unhealthy.\n\
         # TYPE dd_container_pool_containers_unhealthy_total counter\n\
         dd_container_pool_containers_unhealthy_total {}\n\
         # HELP dd_container_pool_config_refresh_total Successful config refreshes.\n\
         # TYPE dd_container_pool_config_refresh_total counter\n\
         dd_container_pool_config_refresh_total {}\n\
         # HELP dd_container_pool_config_refresh_failures_total Failed config refreshes.\n\
         # TYPE dd_container_pool_config_refresh_failures_total counter\n\
         dd_container_pool_config_refresh_failures_total {}\n\
         # HELP dd_container_pool_container_health_checks_total Container health checks attempted.\n\
         # TYPE dd_container_pool_container_health_checks_total counter\n\
         dd_container_pool_container_health_checks_total {}\n\
         # HELP dd_container_pool_container_health_check_failures_total Container health checks failed.\n\
         # TYPE dd_container_pool_container_health_check_failures_total counter\n\
         dd_container_pool_container_health_check_failures_total {}\n\
         # HELP dd_container_pool_warm_containers Current known warm containers.\n\
         # TYPE dd_container_pool_warm_containers gauge\n\
         dd_container_pool_warm_containers {}\n\
         dd_container_pool_idle_containers {}\n\
         dd_container_pool_busy_containers {}\n\
         dd_container_pool_unhealthy_containers {}\n",
        state.metrics.http_requests_total.load(Ordering::Relaxed),
        state.metrics.dispatch_total.load(Ordering::Relaxed),
        state.metrics.dispatch_failures_total.load(Ordering::Relaxed),
        state.metrics.nats_messages_total.load(Ordering::Relaxed),
        state.metrics.nats_failures_total.load(Ordering::Relaxed),
        state.metrics.containers_started_total.load(Ordering::Relaxed),
        state.metrics.containers_removed_total.load(Ordering::Relaxed),
        state
            .metrics
            .containers_unhealthy_total
            .load(Ordering::Relaxed),
        state.metrics.config_refresh_total.load(Ordering::Relaxed),
        state
            .metrics
            .config_refresh_failures_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .container_health_checks_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .container_health_check_failures_total
            .load(Ordering::Relaxed),
        warm,
        idle,
        busy,
        unhealthy
    );
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        body,
    )
}

async fn list_pools(State(state): State<AppState>, headers: HeaderMap) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    Json(PoolsResponse {
        ok: true,
        generated_at_ms: now_ms(),
        pools: pool_summaries(&state).await,
    })
    .into_response()
}

async fn get_pool(
    State(state): State<AppState>,
    Path(pool): Path<String>,
    headers: HeaderMap,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let summaries = pool_summaries(&state).await;
    if let Some(summary) = summaries
        .into_iter()
        .find(|summary| summary.id == pool || summary.slug == pool)
    {
        Json(json!({ "ok": true, "pool": summary })).into_response()
    } else {
        json_error(StatusCode::NOT_FOUND, "unknown container pool")
    }
}

async fn warm_pool(
    State(state): State<AppState>,
    Path(pool): Path<String>,
    headers: HeaderMap,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let pool_id = {
        let registry = state.registry.lock().await;
        match pool_id_from_selector(&registry, &pool) {
            Some(pool_id) => pool_id,
            None => return json_error(StatusCode::NOT_FOUND, "unknown container pool"),
        }
    };
    match reconcile_pool(&state, &pool_id).await {
        Ok(()) => Json(json!({ "ok": true, "pool": pool, "pools": pool_summaries(&state).await }))
            .into_response(),
        Err(error) => json_error(StatusCode::BAD_GATEWAY, &error),
    }
}

async fn dispatch_pool(
    State(state): State<AppState>,
    Path(pool): Path<String>,
    headers: HeaderMap,
    Json(request): Json<DispatchRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    match dispatch_to_pool(&state, &pool, request).await {
        Ok(response) => {
            let status = StatusCode::from_u16(response.status).unwrap_or(StatusCode::OK);
            (status, Json(response)).into_response()
        }
        Err(error) => json_error(StatusCode::BAD_GATEWAY, &error),
    }
}

async fn run_config_refresh_loop(state: AppState) {
    loop {
        if let Err(error) = refresh_pool_configs(&state).await {
            eprintln!("container pool config refresh failed: {error}");
            record_config_error(&state, error).await;
        }
        reconcile_all(&state).await;
        sleep(state.config.config_refresh).await;
    }
}

async fn run_reconcile_loop(state: AppState) {
    loop {
        reconcile_all(&state).await;
        sleep(state.config.reconcile_interval).await;
    }
}

/// Subscribe to the WAL gateway and refresh the pool registry whenever:
///   * the `app_config` row this server reads from changes (scope/key match), or
///   * any row in `container_pool_configs` changes.
///
/// We don't try to be surgical (partial-apply just the changed pool). The
/// existing `refresh_pool_configs` is cheap enough that a full reload is
/// the simplest correct thing — the registry mutex already serializes
/// readers against the swap.
///
/// The poll loop is still on as the fallback path. The CDC subscription
/// just shortens the perceived edit-to-effect latency from O(refresh_secs)
/// to O(WAL gateway poll interval) ≈ a few hundred ms.
async fn run_cdc_refresh_subscription(state: AppState) {
    let Some(nats) = state.nats.clone() else {
        println!("container pool cdc subscription disabled: no NATS_URL configured");
        return;
    };
    let jetstream = async_nats::jetstream::new(nats);
    let app_config_scope = state.config.app_config_scope.clone();
    let app_config_key = state.config.app_config_key.clone();
    let stream_name = env_value("CONTAINER_POOL_CDC_STREAM", "CDC");

    // Subscription 1 — app_config (filtered to the row we read from).
    {
        let task_state = state.clone();
        let scope = app_config_scope.clone();
        let key = app_config_key.clone();
        let durable = format!(
            "dd-container-pool-app-config-{}",
            cdc_sanitize(&format!("{scope}.{key}"))
        );
        let result = dd_wal_consumer::Subscription::builder()
            .stream(stream_name.clone())
            .durable_name(durable.clone())
            .filter_subject("cdc.public.app_config.>")
            .start(&jetstream, move |change: dd_wal_consumer::RowChange| {
                let task_state = task_state.clone();
                let scope = scope.clone();
                let key = key.clone();
                async move {
                    let row_scope = change.column("scope").and_then(Value::as_str);
                    let row_key = change.column("key").and_then(Value::as_str);
                    if row_scope != Some(scope.as_str()) || row_key != Some(key.as_str()) {
                        return;
                    }
                    cdc_trigger_refresh(&task_state, "app_config").await;
                }
            })
            .await;
        log_cdc_subscription_result(&durable, "cdc.public.app_config.>", result);
    }

    // Subscription 2 — container_pool_configs (no row filter, every change
    // touches the registry).
    {
        let task_state = state.clone();
        let durable = "dd-container-pool-table".to_string();
        let result = dd_wal_consumer::Subscription::builder()
            .stream(stream_name.clone())
            .durable_name(durable.clone())
            .filter_subject("cdc.public.container_pool_configs.>")
            .start(&jetstream, move |_change: dd_wal_consumer::RowChange| {
                let task_state = task_state.clone();
                async move {
                    cdc_trigger_refresh(&task_state, "container_pool_configs").await;
                }
            })
            .await;
        log_cdc_subscription_result(&durable, "cdc.public.container_pool_configs.>", result);
    }
}

async fn cdc_trigger_refresh(state: &AppState, source: &str) {
    if let Err(error) = refresh_pool_configs(state).await {
        eprintln!("container pool CDC-driven refresh failed ({source}): {error}");
        record_config_error(state, error).await;
        return;
    }
    // Trigger a reconcile too so containers actually warm/cool in line
    // with the new config without waiting for the regular reconcile tick.
    reconcile_all(state).await;
}

fn cdc_sanitize(input: &str) -> String {
    input
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn log_cdc_subscription_result(
    durable: &str,
    subject: &str,
    result: Result<tokio::task::JoinHandle<()>, dd_wal_consumer::Error>,
) {
    match result {
        Ok(_join) => {
            println!(
                "container pool cdc subscription started: durable={durable} subject={subject}"
            );
        }
        Err(error) => {
            eprintln!(
                "container pool cdc subscription failed to start ({error}); \
                 falling back to poll-only refresh for {subject}"
            );
        }
    }
}

async fn run_nats_loop(state: AppState) {
    let Some(client) = state.nats.clone() else {
        return;
    };
    let mut subscriber = match client.subscribe(state.config.nats_subject.clone()).await {
        Ok(subscriber) => subscriber,
        Err(error) => {
            eprintln!("container pool nats subscribe failed: {error}");
            return;
        }
    };
    while let Some(message) = subscriber.next().await {
        state
            .metrics
            .nats_messages_total
            .fetch_add(1, Ordering::Relaxed);
        if message.payload.len() > state.config.nats_max_payload_bytes {
            state
                .metrics
                .nats_failures_total
                .fetch_add(1, Ordering::Relaxed);
            continue;
        }
        let request = match serde_json::from_slice::<DispatchRequest>(&message.payload) {
            Ok(request) => request,
            Err(error) => {
                eprintln!("container pool invalid nats request: {error}");
                state
                    .metrics
                    .nats_failures_total
                    .fetch_add(1, Ordering::Relaxed);
                continue;
            }
        };
        let selector = {
            let registry = state.registry.lock().await;
            pool_selector_from_request(&request, Some(message.subject.as_ref()), &registry)
        };
        let Some(selector) = selector else {
            eprintln!("container pool nats request missing pool selector");
            state
                .metrics
                .nats_failures_total
                .fetch_add(1, Ordering::Relaxed);
            continue;
        };
        let response = match dispatch_to_pool(&state, &selector, request).await {
            Ok(response) => json!(response),
            Err(error) => {
                state
                    .metrics
                    .nats_failures_total
                    .fetch_add(1, Ordering::Relaxed);
                json!({ "ok": false, "error": error, "generatedAtMs": now_ms() })
            }
        };
        let Ok(payload) = serde_json::to_vec(&response) else {
            continue;
        };
        if let Some(reply) = message.reply {
            if let Err(error) = client.publish(reply, payload.into()).await {
                eprintln!("container pool nats reply failed: {error}");
            }
        } else if let Err(error) = client
            .publish(state.config.nats_result_subject.clone(), payload.into())
            .await
        {
            eprintln!("container pool nats result publish failed: {error}");
        }
    }
}

async fn api_docs_html() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("../generated/api-docs.html"))
}

async fn api_docs_json() -> impl axum::response::IntoResponse {
    (
        [("content-type", "application/json; charset=utf-8")],
        include_str!("../generated/api-docs.json"),
    )
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = Arc::new(service_config_from_env());
    let registry = Arc::new(Mutex::new(PoolRegistry {
        next_port: config.port_start,
        ..PoolRegistry::default()
    }));
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(900))
        .build()?;
    let nats = match config.nats_url.as_deref() {
        Some(url) => match async_nats::connect(url).await {
            Ok(client) => Some(client),
            Err(error) => {
                eprintln!("container pool nats connect failed: {error}");
                None
            }
        },
        None => None,
    };
    let state = AppState {
        config,
        registry,
        http,
        nats,
        metrics: Arc::new(Metrics::default()),
    };

    println!(
        "{SERVICE_NAME} starting: nerdctl={} namespace={} network={} db_configured={} nats_subject={}",
        state.config.nerdctl_bin,
        state.config.containerd_namespace,
        state.config.network,
        state.config.database_url.is_some(),
        state.config.nats_subject
    );

    if let Err(error) = cleanup_managed_containers_on_start(&state).await {
        eprintln!("container pool startup cleanup failed: {error}");
    }
    if let Err(error) = refresh_pool_configs(&state).await {
        eprintln!("container pool initial config refresh failed: {error}");
        record_config_error(&state, error).await;
    }
    let initial_reconcile_state = state.clone();
    tokio::spawn(async move {
        reconcile_all(&initial_reconcile_state).await;
    });

    tokio::spawn(run_config_refresh_loop(state.clone()));
    tokio::spawn(run_reconcile_loop(state.clone()));
    tokio::spawn(run_nats_loop(state.clone()));
    tokio::spawn(run_cdc_refresh_subscription(state.clone()));

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .route("/metrics", get(metrics))
        .route("/pools", get(list_pools))
        .route("/pools/:pool", get(get_pool))
        .route("/pools/:pool/warm", post(warm_pool))
        .route("/pools/:pool/dispatch", post(dispatch_pool))
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .with_state(state.clone());

    let host = env_value("HOST", "0.0.0.0");
    let port = env_u16("PORT", DEFAULT_PORT);
    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("{SERVICE_NAME} listening on {addr}");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            if let Err(error) = tokio::signal::ctrl_c().await {
                eprintln!("shutdown signal error: {error}");
            }
        })
        .await?;
    Ok(())
}
