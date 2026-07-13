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
use dd_nats_subject_defs::{
    cdc_table_filter_subject, container_pool_events_subject, container_pool_heartbeats_subject,
    CONTAINER_POOL_REQUESTS_SUBJECT, CONTAINER_POOL_RESULTS_SUBJECT,
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
const DEFAULT_REDIS_LOCK_PREFIX: &str = "dd:container-pool:affinity";

static LOCK_TOKEN_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
struct AppState {
    config: Arc<ServiceConfig>,
    registry: Arc<Mutex<PoolRegistry>>,
    http: reqwest::Client,
    nats: Option<async_nats::Client>,
    redis_locks: Option<RedisLockManager>,
    metrics: Arc<Metrics>,
}

// Docker-UX container engines we drive with a shared `run -d`/`rm`/`ps`/`inspect`
// flag surface. nerdctl scopes to a containerd namespace via the global `-n`;
// docker and podman do not. Lower-level OCI runtimes (runc, crun, Kata, gVisor)
// are selected under any of these engines via `--runtime` (see `oci_runtime`),
// not as a separate engine. LXD (system containers) and CRI-O (crictl + pod
// sandbox config) use different command models and are intentionally not driven
// here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EngineKind {
    Nerdctl,
    Docker,
    Podman,
}

impl EngineKind {
    fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "docker" => EngineKind::Docker,
            "podman" => EngineKind::Podman,
            _ => EngineKind::Nerdctl,
        }
    }

    fn default_bin(self) -> &'static str {
        match self {
            EngineKind::Docker => "/usr/bin/docker",
            EngineKind::Podman => "/usr/bin/podman",
            EngineKind::Nerdctl => "/usr/local/bin/nerdctl",
        }
    }

    fn label(self) -> &'static str {
        match self {
            EngineKind::Docker => "docker",
            EngineKind::Podman => "podman",
            EngineKind::Nerdctl => "nerdctl",
        }
    }

    // Only nerdctl carries the containerd namespace as a global pre-subcommand flag.
    fn uses_namespace(self) -> bool {
        matches!(self, EngineKind::Nerdctl)
    }
}

#[derive(Clone)]
struct ServiceConfig {
    engine: EngineKind,
    engine_bin: String,
    oci_runtime: Option<String>,
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
    redis_url: Option<String>,
    redis_lock_prefix: String,
    redis_lock_ttl: Duration,
    redis_lock_wait_timeout: Duration,
    redis_lock_retry_delay: Duration,
    redis_lock_request_timeout: Duration,
    worker_response_max_bytes: usize,
    config_refresh: Duration,
    reconcile_interval: Duration,
    command_timeout: Duration,
    nerdctl_run_timeout: Duration,
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
    cap_drop_all: bool,
    no_new_privileges: bool,
    mount_source_allowlist: Vec<String>,
    allow_writable_mounts: bool,
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

/// A volume/bind mount for warm containers. Used to share code or compiled
/// binaries into a generic runtime image (zero-copy) instead of baking a
/// per-language image: the image supplies the runtime/libc, the mount supplies
/// the code, and `command`/`env` are the per-pool flags. Defaults to read-only.
#[derive(Debug, Clone)]
struct Mount {
    source: String,
    target: String,
    read_only: bool,
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
    mounts: Vec<Mount>,
    // Opt out of the automatic cap-drop/no-new-privileges applied to pools that
    // mount external code. Does NOT grant `--privileged` or add capabilities; it
    // only falls back to the service-level security flags.
    unconfined: bool,
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
    fresh_affinity: Option<bool>,
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
    mounts: Vec<String>,
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

#[derive(Clone)]
struct RedisLockManager {
    client: redis::Client,
    key_prefix: String,
    ttl: Duration,
    wait_timeout: Duration,
    retry_delay: Duration,
    request_timeout: Duration,
}

struct RedisLockGuard {
    manager: RedisLockManager,
    key: String,
    token: String,
}

impl RedisLockManager {
    fn new(
        redis_url: &str,
        key_prefix: String,
        ttl: Duration,
        wait_timeout: Duration,
        retry_delay: Duration,
        request_timeout: Duration,
    ) -> Result<Self, String> {
        let client = redis::Client::open(redis_url)
            .map_err(|error| format!("invalid container pool redis url: {error}"))?;
        Ok(Self {
            client,
            key_prefix: key_prefix.trim_matches(':').to_string(),
            ttl,
            wait_timeout,
            retry_delay,
            request_timeout,
        })
    }

    fn lock_key(&self, suffix: &str) -> String {
        format!("{}:{suffix}", self.key_prefix)
    }

    async fn acquire(&self, suffix: &str) -> Result<RedisLockGuard, String> {
        let key = self.lock_key(suffix);
        let token = next_lock_token();
        let started = tokio::time::Instant::now();
        let mut last_error = None::<String>;
        loop {
            match self.try_acquire(&key, &token).await {
                Ok(true) => {
                    return Ok(RedisLockGuard {
                        manager: self.clone(),
                        key,
                        token,
                    });
                }
                Ok(false) => {}
                Err(error) => last_error = Some(error),
            }
            if started.elapsed() >= self.wait_timeout {
                let waited_ms = duration_millis_u64(started.elapsed());
                let detail = last_error
                    .map(|error| format!("; last redis error: {error}"))
                    .unwrap_or_default();
                return Err(format!(
                    "timed out after {waited_ms}ms waiting for container affinity lock {key}{detail}"
                ));
            }
            sleep(self.retry_delay).await;
        }
    }

    async fn try_acquire(&self, key: &str, token: &str) -> Result<bool, String> {
        let mut connection = timeout(
            self.request_timeout,
            self.client.get_multiplexed_async_connection(),
        )
        .await
        .map_err(|_| {
            format!(
                "redis connection timed out after {}ms",
                duration_millis_u64(self.request_timeout)
            )
        })?
        .map_err(|error| error.to_string())?;
        let ttl_ms = duration_millis_u64(self.ttl).max(1);
        let response: Option<String> = timeout(
            self.request_timeout,
            redis::cmd("SET")
                .arg(key)
                .arg(token)
                .arg("NX")
                .arg("PX")
                .arg(ttl_ms)
                .query_async(&mut connection),
        )
        .await
        .map_err(|_| {
            format!(
                "redis SET NX timed out after {}ms",
                duration_millis_u64(self.request_timeout)
            )
        })?
        .map_err(|error| error.to_string())?;
        Ok(response
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case("OK")))
    }
}

impl RedisLockGuard {
    async fn release(self) -> Result<bool, String> {
        let mut connection = timeout(
            self.manager.request_timeout,
            self.manager.client.get_multiplexed_async_connection(),
        )
        .await
        .map_err(|_| {
            format!(
                "redis connection timed out after {}ms",
                duration_millis_u64(self.manager.request_timeout)
            )
        })?
        .map_err(|error| error.to_string())?;
        let _: String = timeout(
            self.manager.request_timeout,
            redis::cmd("WATCH")
                .arg(&self.key)
                .query_async(&mut connection),
        )
        .await
        .map_err(|_| {
            format!(
                "redis WATCH timed out after {}ms",
                duration_millis_u64(self.manager.request_timeout)
            )
        })?
        .map_err(|error| error.to_string())?;
        let current: Option<String> = timeout(
            self.manager.request_timeout,
            redis::cmd("GET")
                .arg(&self.key)
                .query_async(&mut connection),
        )
        .await
        .map_err(|_| {
            format!(
                "redis GET timed out after {}ms",
                duration_millis_u64(self.manager.request_timeout)
            )
        })?
        .map_err(|error| error.to_string())?;
        if current.as_deref() != Some(self.token.as_str()) {
            let _: String = timeout(
                self.manager.request_timeout,
                redis::cmd("UNWATCH").query_async(&mut connection),
            )
            .await
            .map_err(|_| {
                format!(
                    "redis UNWATCH timed out after {}ms",
                    duration_millis_u64(self.manager.request_timeout)
                )
            })?
            .map_err(|error| error.to_string())?;
            return Ok(false);
        }
        let _: String = timeout(
            self.manager.request_timeout,
            redis::cmd("MULTI").query_async(&mut connection),
        )
        .await
        .map_err(|_| {
            format!(
                "redis MULTI timed out after {}ms",
                duration_millis_u64(self.manager.request_timeout)
            )
        })?
        .map_err(|error| error.to_string())?;
        let _: String = timeout(
            self.manager.request_timeout,
            redis::cmd("DEL")
                .arg(&self.key)
                .query_async(&mut connection),
        )
        .await
        .map_err(|_| {
            format!(
                "redis DEL timed out after {}ms",
                duration_millis_u64(self.manager.request_timeout)
            )
        })?
        .map_err(|error| error.to_string())?;
        let deleted: Option<Vec<i64>> = timeout(
            self.manager.request_timeout,
            redis::cmd("EXEC").query_async(&mut connection),
        )
        .await
        .map_err(|_| {
            format!(
                "redis EXEC timed out after {}ms",
                duration_millis_u64(self.manager.request_timeout)
            )
        })?
        .map_err(|error| error.to_string())?;
        Ok(deleted
            .and_then(|values| values.first().copied())
            .unwrap_or_default()
            > 0)
    }
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

fn next_lock_token() -> String {
    let seq = LOCK_TOKEN_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{SERVICE_NAME}:{}:{}:{seq}", std::process::id(), now_ms())
}

fn service_config_from_env() -> ServiceConfig {
    let port_start = env_u16("CONTAINER_POOL_PORT_START", 12_000);
    let port_end = env_u16("CONTAINER_POOL_PORT_END", 12_999).max(port_start);
    let network = env_value("CONTAINER_POOL_NETWORK", "host");
    let engine_raw = env_value("CONTAINER_POOL_ENGINE", "nerdctl");
    let engine = EngineKind::parse(&engine_raw);
    if !engine_raw.trim().is_empty() && !engine_value_recognized(&engine_raw) {
        tracing::error!(
            "{SERVICE_NAME} warning: unrecognized CONTAINER_POOL_ENGINE={engine_raw:?}; defaulting to nerdctl"
        );
    }
    let oci_runtime = match classify_oci_runtime(first_env(&["CONTAINER_POOL_OCI_RUNTIME"]).as_deref())
    {
        Ok(value) => value,
        Err(message) => {
            tracing::error!(
                "{SERVICE_NAME} warning: {message}; containers will use the engine default OCI \
                 runtime (no --runtime) — this may be weaker isolation than intended"
            );
            None
        }
    };
    ServiceConfig {
        engine,
        engine_bin: first_env(&["CONTAINER_POOL_ENGINE_BIN", "CONTAINER_POOL_NERDCTL_BIN"])
            .filter(|bin| !bin.trim().is_empty())
            .unwrap_or_else(|| engine.default_bin().to_string()),
        oci_runtime,
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
            CONTAINER_POOL_REQUESTS_SUBJECT,
        ),
        nats_result_subject: env_value(
            "CONTAINER_POOL_NATS_RESULT_SUBJECT",
            CONTAINER_POOL_RESULTS_SUBJECT,
        ),
        nats_max_payload_bytes: env_usize(
            "CONTAINER_POOL_NATS_MAX_PAYLOAD_BYTES",
            MAX_NATS_PAYLOAD_BYTES,
        )
        .min(16 * 1024 * 1024),
        redis_url: first_env(&["CONTAINER_POOL_REDIS_URL", "REDIS_URL"]),
        redis_lock_prefix: env_value(
            "CONTAINER_POOL_REDIS_LOCK_PREFIX",
            DEFAULT_REDIS_LOCK_PREFIX,
        ),
        redis_lock_ttl: Duration::from_secs(env_u64("CONTAINER_POOL_REDIS_LOCK_TTL_SECONDS", 600)),
        redis_lock_wait_timeout: Duration::from_secs(env_u64(
            "CONTAINER_POOL_REDIS_LOCK_WAIT_TIMEOUT_SECONDS",
            420,
        )),
        redis_lock_retry_delay: Duration::from_millis(env_u64(
            "CONTAINER_POOL_REDIS_LOCK_RETRY_MS",
            250,
        )),
        redis_lock_request_timeout: Duration::from_millis(env_u64(
            "CONTAINER_POOL_REDIS_LOCK_REQUEST_TIMEOUT_MS",
            800,
        )),
        worker_response_max_bytes: env_usize(
            "CONTAINER_POOL_WORKER_RESPONSE_MAX_BYTES",
            MAX_WORKER_RESPONSE_BYTES,
        )
        .min(16 * 1024 * 1024),
        config_refresh: Duration::from_secs(env_u64("CONTAINER_POOL_CONFIG_REFRESH_SECONDS", 30)),
        reconcile_interval: Duration::from_secs(env_u64("CONTAINER_POOL_RECONCILE_SECONDS", 10)),
        command_timeout: Duration::from_secs(env_u64("CONTAINER_POOL_COMMAND_TIMEOUT_SECONDS", 30)),
        nerdctl_run_timeout: Duration::from_secs(env_u64(
            "CONTAINER_POOL_NERDCTL_RUN_TIMEOUT_SECONDS",
            180,
        )),
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
        pids_limit: env_u64("CONTAINER_POOL_PIDS_LIMIT", 4096).clamp(16, 16384),
        nofile_limit: env_u64("CONTAINER_POOL_NOFILE_LIMIT", 65536).clamp(32, 262144),
        cap_drop_all: env_bool("CONTAINER_POOL_CAP_DROP_ALL", false),
        no_new_privileges: env_bool("CONTAINER_POOL_NO_NEW_PRIVILEGES", false),
        mount_source_allowlist: mount_source_allowlist(),
        allow_writable_mounts: env_bool("CONTAINER_POOL_ALLOW_WRITABLE_MOUNTS", false),
    }
}

// Absolute host-path prefixes under which pools may bind-mount code/binaries.
// Empty by default: only named volumes are permitted unless an operator opts in.
fn mount_source_allowlist() -> Vec<String> {
    first_env(&["CONTAINER_POOL_MOUNT_SOURCE_ALLOWLIST"])
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|prefix| prefix.starts_with('/') && safe_local_path(prefix))
        .map(str::to_string)
        .collect()
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
        mounts: mounts_from_json(value, &slug)?,
        unconfined: json_bool_field(value, "unconfined", "unconfined").unwrap_or(false),
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

// Parse the optional `mounts` (alias `volumes`) array from a pool config. Each
// entry is `{source|volume, target|mountPath, readOnly?}`. Shape is validated
// here; host-path/writable *policy* is enforced at container start where the
// service config is available (`enforce_mount_policy`).
fn mounts_from_json(value: &Value, slug: &str) -> Result<Vec<Mount>, String> {
    let Some(items) = value.get("mounts").or_else(|| value.get("volumes")) else {
        return Ok(Vec::new());
    };
    if items.is_null() {
        return Ok(Vec::new());
    }
    let array = items
        .as_array()
        .ok_or_else(|| format!("container pool {slug} mounts must be an array"))?;
    if array.len() > 16 {
        return Err(format!("container pool {slug} has too many mounts (max 16)"));
    }
    let mut mounts = Vec::with_capacity(array.len());
    for item in array {
        let source = json_string_field(item, "source", "source")
            .or_else(|| json_string_field(item, "volume", "volume"))
            .ok_or_else(|| format!("container pool {slug} mount is missing source"))?;
        let target = json_string_field(item, "target", "target")
            .or_else(|| json_string_field(item, "mountPath", "mount_path"))
            .ok_or_else(|| format!("container pool {slug} mount is missing target"))?;
        if !safe_mount_source(&source) {
            return Err(format!(
                "container pool {slug} has invalid mount source: {source}"
            ));
        }
        if !safe_mount_target(&target) {
            return Err(format!(
                "container pool {slug} has invalid mount target: {target}"
            ));
        }
        if is_reserved_mount_target(&target) {
            return Err(format!(
                "container pool {slug} mount target {target} is reserved"
            ));
        }
        if mounts.iter().any(|existing: &Mount| existing.target == target) {
            return Err(format!(
                "container pool {slug} has duplicate mount target {target}"
            ));
        }
        // Shared code/binaries default to read-only; writable needs an explicit
        // opt-in here and a service-level enable at start.
        let read_only = json_bool_field(item, "readOnly", "read_only").unwrap_or(true);
        mounts.push(Mount {
            source,
            target,
            read_only,
        });
    }
    Ok(mounts)
}

// A mount source is either a nerdctl/docker named volume or an absolute host
// path. ':' and ',' are excluded so the `-v src:dst:mode` argv element stays
// unambiguous; control chars / whitespace / backslash are rejected too.
fn safe_mount_source(input: &str) -> bool {
    if input.is_empty() || input.len() > 256 {
        return false;
    }
    if input
        .bytes()
        .any(|byte| byte <= 0x20 || byte == 0x7f || matches!(byte, b':' | b',' | b'\\'))
    {
        return false;
    }
    if input.starts_with('/') {
        // Require a canonical absolute path (no `//`) so allowlist prefix
        // matching is exact.
        safe_local_path(input) && !input.contains("//")
    } else {
        let bytes = input.as_bytes();
        bytes[0].is_ascii_alphanumeric()
            && bytes
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    }
}

fn safe_mount_target(input: &str) -> bool {
    safe_local_path(input) && !input.contains("//") && !input.contains(':') && !input.contains(',')
}

// Refuse to overmount the container root or the kernel pseudo-filesystems:
// these are never legitimate code-mount points and overmounting them can
// subvert isolation/observability inside the container.
fn is_reserved_mount_target(target: &str) -> bool {
    target == "/"
        || ["/proc", "/sys", "/dev"]
            .iter()
            .any(|reserved| path_has_prefix(target, reserved))
}

// Enforced at container start (has the service config). Named volumes are always
// allowed; absolute host paths must sit under an allowlisted prefix; writable
// mounts require the global opt-in. Fails closed with a clear operator message.
fn enforce_mount_policy(
    allowlist: &[String],
    allow_writable: bool,
    slug: &str,
    mount: &Mount,
) -> Result<(), String> {
    if !mount.read_only && !allow_writable {
        return Err(format!(
            "container pool {slug} requests writable mount {}; set \
             CONTAINER_POOL_ALLOW_WRITABLE_MOUNTS=true to permit",
            mount.target
        ));
    }
    if mount.source.starts_with('/') {
        let allowed = allowlist
            .iter()
            .any(|prefix| path_has_prefix(&mount.source, prefix));
        if !allowed {
            return Err(format!(
                "container pool {slug} host-path mount {} is not under any \
                 CONTAINER_POOL_MOUNT_SOURCE_ALLOWLIST prefix",
                mount.source
            ));
        }
    }
    Ok(())
}

// Prefix match on a path boundary so `/data` does not authorize `/database`.
fn path_has_prefix(path: &str, prefix: &str) -> bool {
    let prefix = prefix.trim_end_matches('/');
    path == prefix || path.starts_with(&format!("{prefix}/"))
}

// Global flags that precede every subcommand. Only nerdctl needs the containerd
// namespace; docker/podman take none.
fn engine_global_args(engine: EngineKind, namespace: &str) -> Vec<String> {
    if engine.uses_namespace() {
        vec!["-n".to_string(), namespace.to_string()]
    } else {
        Vec::new()
    }
}

// An OCI runtime handler for `--runtime`: a short name (runc, crun, runsc), a
// containerd handler (io.containerd.kata.v2, io.containerd.runsc.v1), or an
// absolute path to a runtime binary. No whitespace / shell metacharacters.
fn safe_oci_runtime(input: &str) -> bool {
    let bytes = input.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 128
        && (bytes[0].is_ascii_alphanumeric() || bytes[0] == b'/')
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/'))
}

// Resolve CONTAINER_POOL_OCI_RUNTIME. Absent/empty -> None (engine default
// runtime). Valid -> Some. Set-but-invalid -> Err, so the caller warns loudly
// instead of silently dropping the operator's chosen runtime — for sandbox
// runtimes (gVisor/Kata) a silent fallback to runc is an isolation downgrade.
fn classify_oci_runtime(raw: Option<&str>) -> Result<Option<String>, String> {
    match raw {
        None => Ok(None),
        Some(value) if value.trim().is_empty() => Ok(None),
        Some(value) if safe_oci_runtime(value) => Ok(Some(value.to_string())),
        Some(value) => Err(format!("ignoring invalid CONTAINER_POOL_OCI_RUNTIME={value:?}")),
    }
}

fn engine_value_recognized(raw: &str) -> bool {
    matches!(raw.trim().to_ascii_lowercase().as_str(), "nerdctl" | "docker" | "podman")
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

    // `mounts` column is optional; row_value falls back to `[]` if absent (no
    // migration required for the fallback table).
    let mounts = mounts_from_json(&row_value(row, "mounts", json!([])), &slug)?;

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
        mounts,
        unconfined: false,
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
            tracing::error!("container pool postgres connection error: {error}");
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
            tracing::error!(
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
            tracing::error!("failed to remove container for deleted pool {name}: {error}");
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

// Pure builder for the engine `run -d` argv (everything after the engine binary),
// extracted so it can be unit-tested across engines/runtimes/languages without a
// live daemon. Network/env are resolved by the caller and passed in; mount policy
// is enforced here so a disallowed mount fails the start with a clear error.
#[allow(clippy::too_many_arguments)]
fn build_run_args(
    config: &ServiceConfig,
    pool: &PoolConfig,
    container_name: &str,
    host_port: u16,
    container_env: &BTreeMap<String, String>,
) -> Result<Vec<String>, String> {
    let mut args = engine_global_args(config.engine, &config.containerd_namespace);
    args.push("run".to_string());
    args.push("-d".to_string());
    // Lower-level OCI runtime (runc/crun/Kata/gVisor) selection, engine-agnostic.
    if let Some(runtime) = config.oci_runtime.as_deref() {
        args.push("--runtime".to_string());
        args.push(runtime.to_string());
    }
    args.extend([
        "--name".to_string(),
        container_name.to_string(),
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
        "--pids-limit".to_string(),
        config.pids_limit.to_string(),
        "--ulimit".to_string(),
        format!("nofile={limit}:{limit}", limit = config.nofile_limit),
    ]);
    // Pools that mount external code (the generic shared-volume case) are confined
    // by default — `--cap-drop ALL` + no-new-privileges — even when the service
    // defaults leave them off, since they run code the image did not bake in. A
    // pool can opt out with `unconfined: true` (falls back to the service flags;
    // does not grant extra privileges). Mount-less pools keep prior behavior.
    let mount_hardened = !pool.mounts.is_empty() && !pool.unconfined;
    if config.cap_drop_all || mount_hardened {
        args.push("--cap-drop".to_string());
        args.push("ALL".to_string());
    }
    if config.no_new_privileges || mount_hardened {
        args.push("--security-opt".to_string());
        args.push("no-new-privileges".to_string());
    }
    if pool.read_only {
        args.push("--read-only".to_string());
        args.push("--tmpfs".to_string());
        args.push("/tmp:rw,noexec,nosuid,size=64m".to_string());
    }
    if let Some(memory) = config.container_memory.as_deref() {
        args.push("--memory".to_string());
        args.push(memory.to_string());
    }
    if let Some(cpus) = config.container_cpus.as_deref() {
        args.push("--cpus".to_string());
        args.push(cpus.to_string());
    }
    if let Some(pull_policy) = config.pull_policy.as_deref() {
        args.push("--pull".to_string());
        args.push(pull_policy.to_string());
    }

    // Share code/binaries into the warm container (zero-copy) from a named volume
    // or allowlisted host path. Read-only by default; policy is enforced here.
    for mount in &pool.mounts {
        enforce_mount_policy(
            &config.mount_source_allowlist,
            config.allow_writable_mounts,
            &pool.slug,
            mount,
        )?;
        let mode = if mount.read_only { "ro" } else { "rw" };
        args.push("--volume".to_string());
        args.push(format!("{}:{}:{}", mount.source, mount.target, mode));
    }

    if config.network == "host" {
        args.push("--network".to_string());
        args.push("host".to_string());
        args.push("--env".to_string());
        args.push(format!("PORT={host_port}"));
    } else {
        args.push("--network".to_string());
        args.push(config.network.clone());
        args.push("--publish".to_string());
        args.push(format!("127.0.0.1:{}:{}", host_port, pool.container_port));
        args.push("--env".to_string());
        args.push(format!("PORT={}", pool.container_port));
    }

    for (key, value) in container_env {
        args.push("--env".to_string());
        args.push(format!("{key}={value}"));
    }
    args.push(pool.image.clone());
    args.extend(pool.command.clone());
    Ok(args)
}

async fn start_one_for_pool(state: &AppState, pool_id: &str) -> Result<WarmContainer, String> {
    let (pool, mut container) = allocate_container_slot(state, pool_id).await?;

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
            .or_insert_with(|| container_pool_events_subject(&pool.slug));
        container_env
            .entry("DD_POOL_NATS_HEARTBEAT_SUBJECT".to_string())
            .or_insert_with(|| container_pool_heartbeats_subject(&pool.slug));
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

    let args = build_run_args(&state.config, &pool, &container.name, container.port, &container_env)?;

    let container_run_timeout = state.config.nerdctl_run_timeout;
    let scrubbed_args = args
        .iter()
        .map(|arg| {
            // Match either env-name prefixes or value-bearing args whose
            // name reveals sensitivity. The substring checks below catch
            // any env name containing API_KEY/SECRET/DEPLOY_KEY/TOKEN
            // (covers AWS_SESSION_TOKEN, GITHUB_TOKEN, etc.) and the
            // explicit `AWS_` prefix scrubs AWS_ACCESS_KEY_ID, which
            // would otherwise slip past every substring rule.
            if arg.starts_with("GH_DEPLOY_KEY=")
                || arg.starts_with("SERVER_AUTH_SECRET=")
                || arg.starts_with("ANTHROPIC_API_KEY=")
                || arg.starts_with("OPENAI_API_KEY=")
                || arg.starts_with("CLAUDE_API_KEYS_JSON=")
                || arg.starts_with("OPENAI_API_KEYS_JSON=")
                || arg.starts_with("EVENT_INGEST_SECRET=")
                || arg.starts_with("GH_PAT=")
                || arg.starts_with("AWS_")
                || arg.contains("API_KEY")
                || arg.contains("SECRET")
                || arg.contains("DEPLOY_KEY")
                || arg.contains("TOKEN")
            {
                let prefix = arg.splitn(2, '=').next().unwrap_or("").to_string();
                format!("{prefix}=<redacted>")
            } else {
                arg.clone()
            }
        })
        .collect::<Vec<_>>();
    tracing::error!(
        "dd-container-pool {engine} run for {name}: {bin} {scrubbed_args:?}",
        engine = state.config.engine.label(),
        name = container.name,
        bin = state.config.engine_bin,
    );
    // Surface the *names* (not values) of the env keys that end up forwarded
    // into the warm worker. This makes silent-misconfig regressions obvious in
    // pod logs — e.g. when EVENT_INGEST_URL/EVENT_INGEST_SECRET are missing,
    // the dev-server's eventBus.startVercelIngest pipeline never starts and
    // task events never reach the websocket fanout.
    let mut env_keys: Vec<&str> = container_env.keys().map(String::as_str).collect();
    env_keys.sort_unstable();
    let event_ingest_url_present = container_env.contains_key("EVENT_INGEST_URL");
    let event_ingest_secret_present = container_env.contains_key("EVENT_INGEST_SECRET");
    let nats_url_present = container_env.contains_key("NATS_URL");
    let worker_fanout_secret_present = container_env.contains_key("WORKER_FANOUT_WS_SECRET")
        || container_env.contains_key("GLEAM_WORKER_WS_SECRET")
        || container_env.contains_key("GLEAM_BROADCAST_SECRET");
    tracing::error!(
        "dd-container-pool worker env for {name}: keys={env_keys:?} \
         event_ingest_url={event_ingest_url_present} \
         event_ingest_secret={event_ingest_secret_present} \
         nats_url={nats_url_present} \
         worker_fanout_secret={worker_fanout_secret_present}",
        name = container.name,
    );
    match run_command(&state.config.engine_bin, &args, container_run_timeout).await {
        Ok(output) => {
            let trimmed = output.trim();
            if !trimmed.is_empty() {
                tracing::error!(
                    "dd-container-pool {engine} run -d output for {name}: {trimmed}",
                    engine = state.config.engine.label(),
                    name = container.name
                );
            }
            if let Err(error) = wait_container_ready(state, &pool, &container).await {
                let mut registry = state.registry.lock().await;
                registry.containers.remove(&container.name);
                remove_affinity_for_container(&mut registry, &container.name);
                drop(registry);
                if let Err(remove_error) = remove_container(state, &container.name).await {
                    tracing::error!(
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
    let mut args = engine_global_args(state.config.engine, &state.config.containerd_namespace);
    args.extend(["rm".to_string(), "-f".to_string(), name.to_string()]);
    run_command(&state.config.engine_bin, &args, state.config.command_timeout).await?;
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
    let mut list_args = engine_global_args(state.config.engine, &state.config.containerd_namespace);
    list_args.extend([
        "ps".to_string(),
        "-a".to_string(),
        "-q".to_string(),
        "--filter".to_string(),
        "label=dd.container-pool.managed=true".to_string(),
    ]);
    let output = run_command(
        &state.config.engine_bin,
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
            tracing::error!("failed to remove stale managed container {id}: {error}");
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
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr_trimmed = stderr.trim();
        if !stderr_trimmed.is_empty() && args.iter().any(|arg| arg == "run" || arg == "inspect") {
            tracing::error!(
                "{program} stderr (exit 0, args={args:?}): {}",
                stderr_trimmed.chars().take(1500).collect::<String>()
            );
        }
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
    let inspect_timeout = state.config.command_timeout.min(Duration::from_secs(15));
    let mut args = engine_global_args(state.config.engine, &state.config.containerd_namespace);
    args.extend(["inspect".to_string(), name.to_string()]);
    let output = match run_command(&state.config.engine_bin, &args, inspect_timeout).await {
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
                tracing::error!("failed to inspect starting warm container {name}: {error}");
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
            tracing::error!("failed to remove unhealthy warm container {name}: {error}");
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
            tracing::error!("container pool reconcile failed for {pool_id}: {error}");
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
            tracing::error!("failed to remove stale warm container {name}: {error}");
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

async fn acquire_affinity_dispatch_lock(
    state: &AppState,
    selector: &str,
    affinity_key: Option<&str>,
) -> Result<Option<RedisLockGuard>, String> {
    let Some(affinity_key) = affinity_key else {
        return Ok(None);
    };
    let Some(redis_locks) = state.redis_locks.as_ref() else {
        return Ok(None);
    };
    let pool_id = {
        let registry = state.registry.lock().await;
        pool_id_from_selector(&registry, selector)
            .ok_or_else(|| format!("unknown container pool: {selector}"))?
    };
    redis_locks
        .acquire(&affinity_map_key(&pool_id, affinity_key))
        .await
        .map(Some)
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

fn container_matches_affinity_request(
    container: &WarmContainer,
    affinity_key: &str,
    fresh_affinity: bool,
) -> bool {
    match container.affinity_key.as_deref() {
        Some(bound) => bound == affinity_key,
        None => !fresh_affinity || container.request_count == 0,
    }
}

async fn lease_container(
    state: &AppState,
    selector: &str,
    affinity_key: Option<&str>,
    fresh_affinity: bool,
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
                            container_matches_affinity_request(
                                container,
                                affinity_key,
                                fresh_affinity,
                            )
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

fn payload_string<'a>(body: &'a Value, camel_key: &str, snake_key: &str) -> Option<&'a str> {
    body.get(camel_key)
        .or_else(|| body.get(snake_key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn normalized_repo_identity(value: &str) -> String {
    let mut repo = value.trim().trim_end_matches('/').trim_end_matches(".git");
    if let Some(rest) = repo.strip_prefix("git@github.com:") {
        repo = rest;
    } else if let Some(rest) = repo.strip_prefix("ssh://git@github.com/") {
        repo = rest;
    } else if let Some(rest) = repo.strip_prefix("https://github.com/") {
        repo = rest;
    } else if let Some(rest) = repo.strip_prefix("http://github.com/") {
        repo = rest;
    }
    repo.to_ascii_lowercase()
}

fn validate_repo_affinity(pool: &PoolConfig, body: &Value) -> Result<(), String> {
    let Some(configured_repo) = pool.env.get("DD_REPO_URL").map(String::as_str) else {
        return Ok(());
    };
    let Some(request_repo) = payload_string(body, "repo", "repo") else {
        return Ok(());
    };
    if normalized_repo_identity(configured_repo) != normalized_repo_identity(request_repo) {
        return Err(format!(
            "pool {} is configured for repo {configured_repo}, not {request_repo}",
            pool.slug
        ));
    }

    let configured_branch = pool
        .env
        .get("BASE_BRANCH")
        .or_else(|| pool.env.get("DD_REPO_REF"))
        .map(String::as_str)
        .unwrap_or("dev")
        .trim();
    let request_branch = payload_string(body, "baseBranch", "base_branch").unwrap_or("dev");
    if configured_branch != request_branch {
        return Err(format!(
            "pool {} is configured for baseBranch {configured_branch}, not {request_branch}",
            pool.slug
        ));
    }
    Ok(())
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
    let affinity_key = normalized_affinity_key(request.affinity_key.as_deref());
    let fresh_affinity = request.fresh_affinity.unwrap_or(false) && affinity_key.is_some();
    let lock_guard =
        match acquire_affinity_dispatch_lock(state, selector, affinity_key.as_deref()).await {
            Ok(lock_guard) => lock_guard,
            Err(error) => {
                state
                    .metrics
                    .dispatch_failures_total
                    .fetch_add(1, Ordering::Relaxed);
                return Err(error);
            }
        };
    let result =
        dispatch_to_pool_inner(state, selector, request, affinity_key, fresh_affinity).await;
    if let Some(lock_guard) = lock_guard {
        if let Err(error) = lock_guard.release().await {
            tracing::error!("container pool redis affinity lock release failed: {error}");
        }
    }
    result
}

async fn dispatch_to_pool_inner(
    state: &AppState,
    selector: &str,
    request: DispatchRequest,
    affinity_key: Option<String>,
    fresh_affinity: bool,
) -> Result<DispatchResponse, String> {
    let started = now_ms();
    let lease = lease_container(state, selector, affinity_key.as_deref(), fresh_affinity).await?;
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
    if let Err(error) = validate_repo_affinity(&lease.pool, &body) {
        release_container(state, &lease.container.name, true).await;
        state
            .metrics
            .dispatch_failures_total
            .fetch_add(1, Ordering::Relaxed);
        return Err(error);
    }
    let request_id = request_id_from_request(&request);
    let backfill_state = state.clone();
    let backfill_pool_id = lease.pool.id.clone();
    tokio::spawn(async move {
        if let Err(error) = reconcile_pool(&backfill_state, &backfill_pool_id).await {
            tracing::error!("container pool backfill failed for {backfill_pool_id}: {error}");
        }
    });

    let mut send = apply_forward_headers(state.http.post(&url), request.headers.as_ref());
    if let Some(secret) = state.config.server_auth_secret.as_deref() {
        send = send
            .header("x-server-auth", secret)
            .header("x-container-pool-auth", secret)
            .header("x-agent-auth", secret);
    }
    let send = send.json(&body);
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
                tracing::error!("container pool refill failed for {refill_pool_id}: {error}");
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
        mounts: config
            .mounts
            .iter()
            .map(|mount| {
                format!(
                    "{}:{}:{}",
                    mount.source,
                    mount.target,
                    if mount.read_only { "ro" } else { "rw" }
                )
            })
            .collect(),
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
            tracing::error!("container pool config refresh failed: {error}");
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
        tracing::info!("container pool cdc subscription disabled: no NATS_URL configured");
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
        let app_config_filter = cdc_table_filter_subject("cdc", "public", "app_config");
        let result = dd_wal_consumer::Subscription::builder()
            .stream(stream_name.clone())
            .durable_name(durable.clone())
            .filter_subject(app_config_filter.clone())
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
        log_cdc_subscription_result(&durable, &app_config_filter, result);
    }

    // Subscription 2 — container_pool_configs (no row filter, every change
    // touches the registry).
    {
        let task_state = state.clone();
        let durable = "dd-container-pool-table".to_string();
        let table_filter = cdc_table_filter_subject("cdc", "public", "container_pool_configs");
        let result = dd_wal_consumer::Subscription::builder()
            .stream(stream_name.clone())
            .durable_name(durable.clone())
            .filter_subject(table_filter.clone())
            .start(&jetstream, move |_change: dd_wal_consumer::RowChange| {
                let task_state = task_state.clone();
                async move {
                    cdc_trigger_refresh(&task_state, "container_pool_configs").await;
                }
            })
            .await;
        log_cdc_subscription_result(&durable, &table_filter, result);
    }
}

async fn cdc_trigger_refresh(state: &AppState, source: &str) {
    if let Err(error) = refresh_pool_configs(state).await {
        tracing::error!("container pool CDC-driven refresh failed ({source}): {error}");
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
            tracing::info!(
                "container pool cdc subscription started: durable={durable} subject={subject}"
            );
        }
        Err(error) => {
            tracing::error!(
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
    loop {
        let mut subscriber = match client.subscribe(state.config.nats_subject.clone()).await {
            Ok(subscriber) => subscriber,
            Err(error) => {
                tracing::error!("container pool nats subscribe failed: {error}; retrying in 5s");
                sleep(Duration::from_secs(5)).await;
                continue;
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
                    tracing::error!("container pool invalid nats request: {error}");
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
                tracing::error!("container pool nats request missing pool selector");
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
                    tracing::error!("container pool nats reply failed: {error}");
                }
            } else if let Err(error) = client
                .publish(state.config.nats_result_subject.clone(), payload.into())
                .await
            {
                tracing::error!("container pool nats result publish failed: {error}");
            }
        }
        tracing::error!(
            "container pool nats subscription ended (subject={}); re-subscribing in 5s",
            state.config.nats_subject
        );
        sleep(Duration::from_secs(5)).await;
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
    let _otel = dd_telemetry::init("dd-container-pool");

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
                tracing::error!("container pool nats connect failed: {error}");
                None
            }
        },
        None => None,
    };
    let redis_locks = match config.redis_url.as_deref() {
        Some(url) => Some(RedisLockManager::new(
            url,
            config.redis_lock_prefix.clone(),
            config.redis_lock_ttl,
            config.redis_lock_wait_timeout,
            config.redis_lock_retry_delay,
            config.redis_lock_request_timeout,
        )?),
        None => None,
    };
    let state = AppState {
        config,
        registry,
        http,
        nats,
        redis_locks,
        metrics: Arc::new(Metrics::default()),
    };

    tracing::info!(
        "{SERVICE_NAME} starting: engine={} bin={} namespace={} oci_runtime={} network={} db_configured={} nats_subject={} redis_locks={}",
        state.config.engine.label(),
        state.config.engine_bin,
        state.config.containerd_namespace,
        state.config.oci_runtime.as_deref().unwrap_or("(default)"),
        state.config.network,
        state.config.database_url.is_some(),
        state.config.nats_subject,
        state.redis_locks.is_some()
    );

    if let Err(error) = cleanup_managed_containers_on_start(&state).await {
        tracing::error!("container pool startup cleanup failed: {error}");
    }
    if let Err(error) = refresh_pool_configs(&state).await {
        tracing::error!("container pool initial config refresh failed: {error}");
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
        .with_state(state.clone())
        .merge(dd_runtime_config_client::router());

    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    let host = env_value("HOST", "0.0.0.0");
    let port = env_u16("PORT", DEFAULT_PORT);
    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("{SERVICE_NAME} listening on {addr}");
    axum::serve(listener, app.layer(dd_telemetry::http_trace_layer()))
        .with_graceful_shutdown(async {
            if let Err(error) = tokio::signal::ctrl_c().await {
                tracing::error!("shutdown signal error: {error}");
            }
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_container(affinity_key: Option<&str>, request_count: u64) -> WarmContainer {
        WarmContainer {
            name: "dd-pool-test-1".to_string(),
            pool_id: "pool-1".to_string(),
            pool_slug: "nodejs-chat-claude-k8s-cluster-dev".to_string(),
            affinity_key: affinity_key.map(str::to_string),
            port: 31001,
            status: ContainerStatus::Idle,
            in_flight: 0,
            launched_at_ms: 1,
            last_used_at_ms: 1,
            last_health_at_ms: None,
            last_healthy_at_ms: None,
            health_failure_count: 0,
            last_health_error: None,
            request_count,
            failure_count: 0,
        }
    }

    #[test]
    fn redis_affinity_lock_key_uses_normalized_thread_affinity() {
        let affinity_key = normalized_affinity_key(Some(" thread 1 / bad chars "))
            .expect("normalized affinity key");

        assert_eq!(affinity_key, "thread-1---bad-chars");
        assert_eq!(
            affinity_map_key("nodejs-chat-claude-k8s-cluster-dev", &affinity_key),
            "nodejs-chat-claude-k8s-cluster-dev:thread-1---bad-chars"
        );
    }

    #[test]
    fn fresh_affinity_does_not_reuse_unbound_used_container() {
        let new_thread = "47fc0453-5af1-4807-821e-5b24c4839398";
        let used_unbound = test_container(None, 1);
        let clean_unbound = test_container(None, 0);
        let same_thread = test_container(Some(new_thread), 7);
        let other_thread = test_container(Some("11111111-1111-4111-8111-111111111111"), 3);

        assert!(!container_matches_affinity_request(
            &used_unbound,
            new_thread,
            true
        ));
        assert!(container_matches_affinity_request(
            &clean_unbound,
            new_thread,
            true
        ));
        assert!(container_matches_affinity_request(
            &same_thread,
            new_thread,
            true
        ));
        assert!(!container_matches_affinity_request(
            &other_thread,
            new_thread,
            true
        ));
        assert!(container_matches_affinity_request(
            &used_unbound,
            new_thread,
            false
        ));
    }

    #[test]
    fn mount_source_accepts_named_volumes_and_rejects_path_smuggling() {
        assert!(safe_mount_source("dd-code"));
        assert!(safe_mount_source("dd_code.v1-2"));
        assert!(safe_mount_source("/srv/lambda-bin"));
        // Path-classification escapes and `-v` delimiter smuggling.
        assert!(!safe_mount_source("../etc"));
        assert!(!safe_mount_source("./code"));
        assert!(!safe_mount_source("/srv/../etc"));
        assert!(!safe_mount_source("/srv//code"));
        assert!(!safe_mount_source("vol:/etc"));
        assert!(!safe_mount_source("vol,extra"));
        assert!(!safe_mount_source("bad name"));
        assert!(!safe_mount_source(""));
    }

    #[test]
    fn mount_target_must_be_safe_and_unreserved() {
        assert!(safe_mount_target("/opt/code"));
        assert!(!safe_mount_target("/opt/../etc"));
        assert!(!safe_mount_target("relative"));
        assert!(!safe_mount_target("/opt:/code"));
        assert!(is_reserved_mount_target("/"));
        assert!(is_reserved_mount_target("/proc"));
        assert!(is_reserved_mount_target("/proc/sys"));
        assert!(is_reserved_mount_target("/dev/shm"));
        assert!(!is_reserved_mount_target("/opt/code"));
        // Boundary: /devices is not under /dev.
        assert!(!is_reserved_mount_target("/devices"));
    }

    #[test]
    fn path_prefix_matches_on_boundary() {
        assert!(path_has_prefix("/srv/code", "/srv/code"));
        assert!(path_has_prefix("/srv/code/bin", "/srv/code/"));
        assert!(!path_has_prefix("/srv/codex", "/srv/code"));
        assert!(!path_has_prefix("/srv", "/srv/code"));
    }

    #[test]
    fn mounts_from_json_parses_validates_and_defaults_read_only() {
        let value = json!({
            "mounts": [
                { "source": "dd-code", "target": "/opt/code" },
                { "volume": "dd-bin", "mountPath": "/opt/bin", "readOnly": false }
            ]
        });
        let mounts = mounts_from_json(&value, "svc").expect("valid mounts");
        assert_eq!(mounts.len(), 2);
        assert_eq!(mounts[0].target, "/opt/code");
        assert!(mounts[0].read_only, "defaults to read-only");
        assert!(!mounts[1].read_only);

        assert!(mounts_from_json(&json!({}), "svc").unwrap().is_empty());
        assert!(mounts_from_json(&json!({ "mounts": null }), "svc").unwrap().is_empty());
        assert!(mounts_from_json(&json!({ "mounts": "x" }), "svc").is_err());
        assert!(
            mounts_from_json(
                &json!({ "mounts": [
                    { "source": "a", "target": "/opt/code" },
                    { "source": "b", "target": "/opt/code" }
                ] }),
                "svc"
            )
            .is_err(),
            "duplicate target must be rejected"
        );
        assert!(
            mounts_from_json(&json!({ "mounts": [{ "source": "a", "target": "/proc" }] }), "svc")
                .is_err(),
            "reserved target must be rejected"
        );
    }

    #[test]
    fn enforce_mount_policy_gates_host_paths_and_writes() {
        let allowlist = vec!["/srv/code".to_string()];

        let named_ro = Mount { source: "dd-code".into(), target: "/opt/code".into(), read_only: true };
        assert!(enforce_mount_policy(&allowlist, false, "svc", &named_ro).is_ok());

        let host_allowed = Mount { source: "/srv/code/bin".into(), target: "/opt/bin".into(), read_only: true };
        assert!(enforce_mount_policy(&allowlist, false, "svc", &host_allowed).is_ok());

        // Host path outside the allowlist is rejected.
        let host_denied = Mount { source: "/etc".into(), target: "/opt/etc".into(), read_only: true };
        assert!(enforce_mount_policy(&allowlist, false, "svc", &host_denied).is_err());
        // ...and rejected outright when no allowlist is configured.
        assert!(enforce_mount_policy(&[], false, "svc", &host_allowed).is_err());

        // Writable needs the explicit opt-in.
        let writable = Mount { source: "dd-code".into(), target: "/opt/code".into(), read_only: false };
        assert!(enforce_mount_policy(&allowlist, false, "svc", &writable).is_err());
        assert!(enforce_mount_policy(&allowlist, true, "svc", &writable).is_ok());
    }

    fn test_service_config() -> ServiceConfig {
        ServiceConfig {
            engine: EngineKind::Nerdctl,
            engine_bin: "/usr/local/bin/nerdctl".to_string(),
            oci_runtime: None,
            containerd_namespace: "k8s.io".to_string(),
            network: "host".to_string(),
            pull_policy: Some("never".to_string()),
            database_url: None,
            app_config_key: "container-pool.runtime-pools.v1".to_string(),
            app_config_scope: "default".to_string(),
            nats_url: None,
            nats_subject: "dd.remote.container_pool.requests".to_string(),
            nats_result_subject: "dd.remote.container_pool.results".to_string(),
            nats_max_payload_bytes: 1024,
            redis_url: None,
            redis_lock_prefix: "lock".to_string(),
            redis_lock_ttl: Duration::from_secs(1),
            redis_lock_wait_timeout: Duration::from_secs(1),
            redis_lock_retry_delay: Duration::from_millis(10),
            redis_lock_request_timeout: Duration::from_secs(1),
            worker_response_max_bytes: 1024,
            config_refresh: Duration::from_secs(1),
            reconcile_interval: Duration::from_secs(1),
            command_timeout: Duration::from_secs(1),
            nerdctl_run_timeout: Duration::from_secs(1),
            container_start_timeout: Duration::from_secs(1),
            health_check_interval: Duration::from_secs(1),
            health_check_timeout: Duration::from_millis(100),
            unhealthy_grace: Duration::from_secs(1),
            unhealthy_failure_threshold: 2,
            port_start: 12_000,
            port_end: 12_999,
            cleanup_on_start: false,
            server_auth_secret: None,
            container_memory: Some("256m".to_string()),
            container_cpus: Some("0.50".to_string()),
            forward_env_keys: Vec::new(),
            pids_limit: 4096,
            nofile_limit: 65536,
            cap_drop_all: true,
            no_new_privileges: true,
            mount_source_allowlist: Vec::new(),
            allow_writable_mounts: false,
        }
    }

    fn code_pool(slug: &str, image: &str, command: &[&str]) -> PoolConfig {
        let value = json!({
            "slug": slug,
            "image": image,
            "mounts": [{ "source": "dd-code", "target": "/opt/code", "readOnly": true }],
            "command": command,
        });
        pool_config_from_json(&value).expect("valid pool config")
    }

    fn contains_pair(args: &[String], a: &str, b: &str) -> bool {
        args.windows(2).any(|w| w[0] == a && w[1] == b)
    }

    fn tail_is(args: &[String], tail: &[&str]) -> bool {
        args.len() >= tail.len()
            && args[args.len() - tail.len()..]
                .iter()
                .zip(tail)
                .all(|(got, want)| got == want)
    }

    #[test]
    fn shared_volume_code_runs_eight_runtimes_zero_copy() {
        let config = test_service_config();
        // Code (not data) is shared read-only from one volume; the image only
        // supplies the runtime/libc. Covers multi-file trees (erlang ebin, java
        // classpath, node/python/ruby/bash sources) and single compiled binaries
        // (go, rust) — all zero-copy, no per-function image build.
        let runtimes: [(&str, &str, &[&str]); 8] = [
            ("nodejs-fn", "docker.io/library/dd-cp-nodejs:dev", &["node", "/opt/code/server.mjs"]),
            ("python-fn", "docker.io/library/dd-cp-python3:dev", &["python3", "/opt/code/app.py"]),
            ("ruby-fn", "docker.io/library/dd-cp-ruby:dev", &["ruby", "/opt/code/app.rb"]),
            ("bash-fn", "docker.io/library/dd-cp-bash:dev", &["bash", "/opt/code/run.sh"]),
            (
                "erlang-fn",
                "docker.io/library/dd-cp-erlang:dev",
                &["erl", "-noshell", "-pa", "/opt/code/ebin", "-s", "myapp", "start"],
            ),
            ("golang-fn", "docker.io/library/dd-cp-golang:dev", &["/opt/code/bin/server"]),
            ("rust-fn", "docker.io/library/dd-cp-rust:dev", &["/opt/code/bin/svc"]),
            ("java-fn", "docker.io/library/dd-cp-java:dev", &["java", "-cp", "/opt/code/classes", "Main"]),
        ];
        for (slug, image, command) in runtimes {
            let pool = code_pool(slug, image, command);
            let args = build_run_args(&config, &pool, "c1", 12_345, &BTreeMap::new())
                .unwrap_or_else(|error| panic!("{slug}: {error}"));
            assert!(
                contains_pair(&args, "--volume", "dd-code:/opt/code:ro"),
                "{slug} shared-volume mount missing: {args:?}"
            );
            let mut tail = vec![image];
            tail.extend_from_slice(command);
            assert!(tail_is(&args, &tail), "{slug} image+command tail wrong: {args:?}");
            // Hardening still applies uniformly across runtimes.
            assert!(contains_pair(&args, "--cap-drop", "ALL"), "{slug} cap-drop");
            assert!(args.iter().any(|arg| arg == "--read-only"), "{slug} read-only");
        }
    }

    #[test]
    fn engine_kind_controls_namespace_flag() {
        let pool = code_pool("nodejs-fn", "docker.io/library/x:dev", &["node", "/opt/code/s.mjs"]);

        let nerd = test_service_config();
        let args = build_run_args(&nerd, &pool, "c1", 1, &BTreeMap::new()).unwrap();
        assert_eq!(&args[0..4], &["-n", "k8s.io", "run", "-d"], "nerdctl scopes to a namespace");

        for engine in [EngineKind::Docker, EngineKind::Podman] {
            let mut config = test_service_config();
            config.engine = engine;
            let args = build_run_args(&config, &pool, "c1", 1, &BTreeMap::new()).unwrap();
            assert_eq!(&args[0..2], &["run", "-d"], "{engine:?} omits namespace");
            assert!(!args.iter().any(|arg| arg == "-n"), "{engine:?} has no -n");
        }
    }

    #[test]
    fn oci_runtime_passthrough_and_validation() {
        let pool = code_pool("svc", "docker.io/library/x:dev", &["/opt/code/bin/app"]);
        // runc/crun and the containerd Kata/gVisor handlers all flow through
        // --runtime under any engine.
        for runtime in ["runc", "crun", "runsc", "io.containerd.kata.v2", "io.containerd.runsc.v1"] {
            let mut config = test_service_config();
            config.engine = EngineKind::Docker;
            config.oci_runtime = Some(runtime.to_string());
            let args = build_run_args(&config, &pool, "c1", 1, &BTreeMap::new()).unwrap();
            assert!(contains_pair(&args, "--runtime", runtime), "{runtime}: {args:?}");
        }
        let config = test_service_config();
        let args = build_run_args(&config, &pool, "c1", 1, &BTreeMap::new()).unwrap();
        assert!(!args.iter().any(|arg| arg == "--runtime"), "no --runtime when unset");

        assert!(safe_oci_runtime("crun"));
        assert!(safe_oci_runtime("io.containerd.kata.v2"));
        assert!(safe_oci_runtime("/usr/local/bin/crun"));
        assert!(!safe_oci_runtime("runc; rm -rf /"));
        assert!(!safe_oci_runtime("two words"));
        assert!(!safe_oci_runtime(""));
    }

    #[test]
    fn oci_runtime_set_but_invalid_is_an_error_not_a_silent_downgrade() {
        // Absent / empty => engine default (None), no warning path.
        assert_eq!(classify_oci_runtime(None), Ok(None));
        assert_eq!(classify_oci_runtime(Some("")), Ok(None));
        assert_eq!(classify_oci_runtime(Some("   ")), Ok(None));
        // Valid handlers pass through.
        assert_eq!(classify_oci_runtime(Some("runsc")), Ok(Some("runsc".to_string())));
        assert_eq!(
            classify_oci_runtime(Some("io.containerd.kata.v2")),
            Ok(Some("io.containerd.kata.v2".to_string()))
        );
        // Set-but-invalid must surface as an error so the caller warns instead of
        // silently running under the (weaker) default runtime.
        assert!(classify_oci_runtime(Some("runc; rm -rf /")).is_err());
        assert!(classify_oci_runtime(Some("two words")).is_err());
    }

    #[test]
    fn engine_value_recognition() {
        for ok in ["nerdctl", "Docker", " podman "] {
            assert!(engine_value_recognized(ok), "{ok} should be recognized");
        }
        for bad in ["dcoker", "lxd", "crio", "containerd-shim"] {
            assert!(!engine_value_recognized(bad), "{bad} should not be recognized");
        }
        // Parser still falls back to nerdctl for unknown values.
        assert_eq!(EngineKind::parse("dcoker"), EngineKind::Nerdctl);
        assert_eq!(EngineKind::parse("podman"), EngineKind::Podman);
    }

    #[test]
    fn mounted_code_pools_are_confined_even_when_service_defaults_are_off() {
        // Service leaves the strict flags off (the current default).
        let mut config = test_service_config();
        config.cap_drop_all = false;
        config.no_new_privileges = false;

        // A pool that mounts external code must still be confined.
        let pool = code_pool("svc", "docker.io/library/x:dev", &["/opt/code/app"]);
        let args = build_run_args(&config, &pool, "c1", 1, &BTreeMap::new()).unwrap();
        assert!(contains_pair(&args, "--cap-drop", "ALL"), "mounted pool must drop caps: {args:?}");
        assert!(
            contains_pair(&args, "--security-opt", "no-new-privileges"),
            "mounted pool must set no-new-privileges: {args:?}"
        );
    }

    #[test]
    fn unconfined_pool_opts_out_of_mount_hardening() {
        let mut config = test_service_config();
        config.cap_drop_all = false;
        config.no_new_privileges = false;

        let pool = pool_config_from_json(&json!({
            "slug": "svc",
            "image": "docker.io/library/x:dev",
            "mounts": [{ "source": "dd-code", "target": "/opt/code" }],
            "unconfined": true,
            "command": ["/opt/code/app"],
        }))
        .unwrap();
        let args = build_run_args(&config, &pool, "c1", 1, &BTreeMap::new()).unwrap();
        assert!(!args.iter().any(|a| a == "--cap-drop"), "unconfined opts out of cap-drop: {args:?}");
        assert!(
            !args.iter().any(|a| a == "--security-opt"),
            "unconfined opts out of no-new-privileges: {args:?}"
        );
    }

    #[test]
    fn mountless_pools_follow_service_security_defaults() {
        let mountless = pool_config_from_json(&json!({
            "slug": "svc",
            "image": "docker.io/library/x:dev",
            "command": ["/app"],
        }))
        .unwrap();

        // Service flags off => no strict flags (unchanged prior behavior).
        let mut off = test_service_config();
        off.cap_drop_all = false;
        off.no_new_privileges = false;
        let args = build_run_args(&off, &mountless, "c1", 1, &BTreeMap::new()).unwrap();
        assert!(!args.iter().any(|a| a == "--cap-drop"));
        assert!(!args.iter().any(|a| a == "--security-opt"));

        // Service flags on => strict flags, exactly as before.
        let args = build_run_args(&test_service_config(), &mountless, "c1", 1, &BTreeMap::new()).unwrap();
        assert!(contains_pair(&args, "--cap-drop", "ALL"));
        assert!(contains_pair(&args, "--security-opt", "no-new-privileges"));
    }

    #[test]
    fn bridge_network_publishes_host_port() {
        let pool = code_pool("svc", "docker.io/library/x:dev", &["/opt/code/app"]);
        let mut config = test_service_config();
        config.network = "bridge".to_string();
        let args = build_run_args(&config, &pool, "c1", 23_456, &BTreeMap::new()).unwrap();
        assert!(contains_pair(&args, "--network", "bridge"));
        assert!(contains_pair(&args, "--publish", "127.0.0.1:23456:8080"));
    }
}
