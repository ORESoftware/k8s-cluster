use std::{
    collections::{BTreeMap, HashMap},
    sync::{atomic::AtomicU64, Arc},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::redis_lock::RedisLockManager;

pub(crate) const SERVICE_NAME: &str = "dd-container-pool";
pub(crate) const DEFAULT_PORT: u16 = 8102;
pub(crate) const MAX_HTTP_BODY_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const MAX_NATS_PAYLOAD_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const MAX_WORKER_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const DEFAULT_REDIS_LOCK_PREFIX: &str = "dd:container-pool:affinity";

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) config: Arc<ServiceConfig>,
    pub(crate) registry: Arc<Mutex<PoolRegistry>>,
    pub(crate) http: reqwest::Client,
    pub(crate) nats: Option<async_nats::Client>,
    pub(crate) redis_locks: Option<RedisLockManager>,
    pub(crate) metrics: Arc<Metrics>,
}

// Docker-UX container engines we drive with a shared `run -d`/`rm`/`ps`/`inspect`
// flag surface. nerdctl scopes to a containerd namespace via the global `-n`;
// docker and podman do not. Lower-level OCI runtimes (runc, crun, Kata, gVisor)
// are selected under any of these engines via `--runtime` (see `oci_runtime`),
// not as a separate engine. LXD (system containers) and CRI-O (crictl + pod
// sandbox config) use different command models and are intentionally not driven
// here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EngineKind {
    Nerdctl,
    Docker,
    Podman,
}

impl EngineKind {
    pub(crate) fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "docker" => EngineKind::Docker,
            "podman" => EngineKind::Podman,
            _ => EngineKind::Nerdctl,
        }
    }

    pub(crate) fn default_bin(self) -> &'static str {
        match self {
            EngineKind::Docker => "/usr/bin/docker",
            EngineKind::Podman => "/usr/bin/podman",
            EngineKind::Nerdctl => "/usr/local/bin/nerdctl",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            EngineKind::Docker => "docker",
            EngineKind::Podman => "podman",
            EngineKind::Nerdctl => "nerdctl",
        }
    }

    // Only nerdctl carries the containerd namespace as a global pre-subcommand flag.
    pub(crate) fn uses_namespace(self) -> bool {
        matches!(self, EngineKind::Nerdctl)
    }
}

#[derive(Clone)]
pub(crate) struct ServiceConfig {
    pub(crate) engine: EngineKind,
    pub(crate) engine_bin: String,
    pub(crate) oci_runtime: Option<String>,
    pub(crate) containerd_namespace: String,
    pub(crate) network: String,
    pub(crate) pull_policy: Option<String>,
    pub(crate) database_url: Option<String>,
    pub(crate) app_config_key: String,
    pub(crate) app_config_scope: String,
    pub(crate) nats_url: Option<String>,
    pub(crate) nats_subject: String,
    pub(crate) nats_queue_group: String,
    pub(crate) nats_result_subject: String,
    pub(crate) nats_max_payload_bytes: usize,
    pub(crate) redis_url: Option<String>,
    pub(crate) redis_lock_prefix: String,
    pub(crate) redis_lock_ttl: Duration,
    pub(crate) redis_lock_wait_timeout: Duration,
    pub(crate) redis_lock_retry_delay: Duration,
    pub(crate) redis_lock_request_timeout: Duration,
    pub(crate) worker_response_max_bytes: usize,
    pub(crate) config_refresh: Duration,
    pub(crate) reconcile_interval: Duration,
    pub(crate) command_timeout: Duration,
    pub(crate) nerdctl_run_timeout: Duration,
    pub(crate) container_start_timeout: Duration,
    pub(crate) health_check_interval: Duration,
    pub(crate) health_check_timeout: Duration,
    pub(crate) unhealthy_grace: Duration,
    pub(crate) unhealthy_failure_threshold: u64,
    pub(crate) port_start: u16,
    pub(crate) port_end: u16,
    pub(crate) cleanup_on_start: bool,
    pub(crate) server_auth_secret: Option<String>,
    pub(crate) container_memory: Option<String>,
    pub(crate) container_cpus: Option<String>,
    pub(crate) forward_env_keys: Vec<String>,
    pub(crate) pids_limit: u64,
    pub(crate) nofile_limit: u64,
    pub(crate) cap_drop_all: bool,
    pub(crate) no_new_privileges: bool,
    pub(crate) mount_source_allowlist: Vec<String>,
    pub(crate) allow_writable_mounts: bool,
}

#[derive(Default)]
pub(crate) struct Metrics {
    pub(crate) http_requests_total: AtomicU64,
    pub(crate) dispatch_total: AtomicU64,
    pub(crate) dispatch_failures_total: AtomicU64,
    pub(crate) nats_messages_total: AtomicU64,
    pub(crate) nats_failures_total: AtomicU64,
    pub(crate) containers_started_total: AtomicU64,
    pub(crate) containers_removed_total: AtomicU64,
    pub(crate) containers_unhealthy_total: AtomicU64,
    pub(crate) config_refresh_total: AtomicU64,
    pub(crate) config_refresh_failures_total: AtomicU64,
    pub(crate) container_health_checks_total: AtomicU64,
    pub(crate) container_health_check_failures_total: AtomicU64,
}

#[derive(Default)]
pub(crate) struct PoolRegistry {
    pub(crate) configs: HashMap<String, PoolConfig>,
    pub(crate) slug_to_id: HashMap<String, String>,
    pub(crate) containers: HashMap<String, WarmContainer>,
    pub(crate) affinity: HashMap<String, String>,
    pub(crate) next_port: u16,
    pub(crate) last_config_error: Option<String>,
    pub(crate) last_config_refresh_ms: Option<u128>,
}

/// A volume/bind mount for warm containers. Used to share code or compiled
/// binaries into a generic runtime image (zero-copy) instead of baking a
/// per-language image: the image supplies the runtime/libc, the mount supplies
/// the code, and `command`/`env` are the per-pool flags. Defaults to read-only.
#[derive(Debug, Clone)]
pub(crate) struct Mount {
    pub(crate) source: String,
    pub(crate) target: String,
    pub(crate) read_only: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct PoolConfig {
    pub(crate) id: String,
    pub(crate) slug: String,
    pub(crate) display_name: String,
    pub(crate) image: String,
    pub(crate) command: Vec<String>,
    pub(crate) env: BTreeMap<String, String>,
    pub(crate) request_path: String,
    pub(crate) health_path: String,
    pub(crate) container_port: u16,
    pub(crate) min_warm: usize,
    pub(crate) max_warm: usize,
    pub(crate) max_concurrency_per_container: usize,
    pub(crate) request_timeout: Duration,
    pub(crate) idle_ttl: Duration,
    pub(crate) nats_subject: Option<String>,
    pub(crate) read_only: bool,
    pub(crate) user: String,
    pub(crate) labels: Value,
    pub(crate) mounts: Vec<Mount>,
    // Opt out of the automatic cap-drop/no-new-privileges applied to pools that
    // mount external code. Does NOT grant `--privileged` or add capabilities; it
    // only falls back to the service-level security flags.
    pub(crate) unconfined: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ContainerStatus {
    Starting,
    Idle,
    Busy,
    Draining,
    Unhealthy,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WarmContainer {
    pub(crate) name: String,
    pub(crate) pool_id: String,
    pub(crate) pool_slug: String,
    pub(crate) affinity_key: Option<String>,
    pub(crate) port: u16,
    pub(crate) status: ContainerStatus,
    pub(crate) in_flight: usize,
    pub(crate) launched_at_ms: u128,
    pub(crate) last_used_at_ms: u128,
    pub(crate) last_health_at_ms: Option<u128>,
    pub(crate) last_healthy_at_ms: Option<u128>,
    pub(crate) health_failure_count: u64,
    pub(crate) last_health_error: Option<String>,
    pub(crate) request_count: u64,
    pub(crate) failure_count: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DispatchRequest {
    pub(crate) request_id: Option<String>,
    pub(crate) pool_id: Option<String>,
    pub(crate) pool_slug: Option<String>,
    pub(crate) affinity_key: Option<String>,
    pub(crate) fresh_affinity: Option<bool>,
    pub(crate) path: Option<String>,
    pub(crate) headers: Option<BTreeMap<String, String>>,
    pub(crate) payload: Option<Value>,
    pub(crate) body: Option<Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DispatchResponse {
    pub(crate) ok: bool,
    pub(crate) request_id: String,
    pub(crate) pool_id: String,
    pub(crate) pool_slug: String,
    pub(crate) affinity_key: Option<String>,
    pub(crate) container_name: String,
    pub(crate) container_port: u16,
    pub(crate) target_url: String,
    pub(crate) status: u16,
    pub(crate) body: Value,
    pub(crate) elapsed_ms: u128,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HealthResponse {
    pub(crate) ok: bool,
    pub(crate) service: &'static str,
    pub(crate) postgres_configured: bool,
    pub(crate) nats_configured: bool,
    pub(crate) auth_configured: bool,
    pub(crate) pool_count: usize,
    pub(crate) warm_container_count: usize,
    pub(crate) last_config_refresh_ms: Option<u128>,
    pub(crate) last_config_error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PoolsResponse {
    pub(crate) ok: bool,
    pub(crate) generated_at_ms: u128,
    pub(crate) pools: Vec<PoolSummary>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PoolSummary {
    pub(crate) id: String,
    pub(crate) slug: String,
    pub(crate) display_name: String,
    pub(crate) image: String,
    pub(crate) request_path: String,
    pub(crate) health_path: String,
    pub(crate) container_port: u16,
    pub(crate) min_warm: usize,
    pub(crate) max_warm: usize,
    pub(crate) max_concurrency_per_container: usize,
    pub(crate) request_timeout_ms: u64,
    pub(crate) idle_ttl_seconds: u64,
    pub(crate) nats_subject: Option<String>,
    pub(crate) env_keys: Vec<String>,
    pub(crate) mounts: Vec<String>,
    pub(crate) labels: Value,
    pub(crate) active_containers: usize,
    pub(crate) idle_containers: usize,
    pub(crate) busy_containers: usize,
    pub(crate) unhealthy_containers: usize,
    pub(crate) containers: Vec<WarmContainer>,
}

#[derive(Debug, Clone)]
pub(crate) struct ContainerLease {
    pub(crate) pool: PoolConfig,
    pub(crate) container: WarmContainer,
}
