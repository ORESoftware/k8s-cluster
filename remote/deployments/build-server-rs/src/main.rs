use std::{
    collections::{BTreeMap, HashMap, HashSet},
    env,
    net::SocketAddr,
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    body::Body,
    extract::{Path as AxumPath, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use hmac::{Hmac, Mac};
use reqwest::header::{HeaderMap as ReqwestHeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use time::OffsetDateTime;
use tokio::{
    fs::{self, OpenOptions},
    io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader},
    process::Command,
    sync::{RwLock, Semaphore},
    time::timeout,
};
use tokio_util::io::ReaderStream;

mod db;
mod entity;
mod events;
mod fiducia;
mod gh_secrets;
mod lambda_exec;
mod profiles;
mod webhooks;

const SERVICE_NAME: &str = "dd-build-server";
const DEFAULT_PORT: u16 = 8100;

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    http: reqwest::Client,
    jobs: Arc<RwLock<HashMap<String, BuildJobRecord>>>,
    semaphore: Arc<Semaphore>,
    counters: Arc<Counters>,
    /// Optional Postgres persistence (own database `dd_build_server` on RDS).
    db: Option<sea_orm::DatabaseConnection>,
    /// Optional NATS client for lifecycle events and request intake.
    nats: Option<async_nats::Client>,
    /// Stable per-process holder identity for fiducia locks/leases.
    holder: String,
    /// Local dedupe of NATS/webhook requestIds (fiducia + JetStream Nats-Msg-Id
    /// are the distributed guards; this catches quick same-process redelivery).
    recent_request_ids: Arc<RwLock<HashSet<String>>>,
}

#[derive(Clone)]
struct Config {
    work_root: PathBuf,
    git_bin: String,
    /// Precomputed Basic authorization header for trusted private GitHub clones.
    /// Never serialized or written to command logs.
    git_http_auth_header: Option<String>,
    nerdctl_bin: String,
    kubectl_bin: String,
    tar_bin: String,
    containerd_namespace: String,
    allowed_repo_prefixes: Vec<String>,
    allowed_image_prefixes: Vec<String>,
    allowed_namespaces: HashSet<String>,
    allowed_profiles: HashSet<String>,
    allowed_profile_repo_prefixes: Vec<String>,
    profile_cpus: String,
    profile_memory: String,
    profile_pids_limit: String,
    deploy_enabled: bool,
    push_enabled: bool,
    ecr_login_enabled: bool,
    aws_region: String,
    job_timeout: Duration,
    /// Overall wall-clock deadline for one job (all commands together);
    /// job_timeout still bounds each individual command.
    job_deadline: Duration,
    max_log_bytes: u64,
    max_jobs: usize,
    /// Reject new submissions once this many jobs are queued (backpressure —
    /// authenticated callers must not be able to grow memory unboundedly).
    max_queued: usize,
    /// Keep cloned repos on disk after the job finishes (default: remove;
    /// build logs are always kept).
    keep_workdirs: bool,
    server_auth_secret: Option<String>,

    // --- Postgres (own database dd_build_server; see src/db.rs) ---
    database_url: Option<String>,

    // --- fiducia.cloud coordination (see src/fiducia.rs) ---
    fiducia_url: String,
    fiducia_api_key: Option<String>,
    coordination_enabled: bool,
    coordination_required: bool,
    lock_ttl: Duration,
    lock_wait_budget: Duration,
    lock_retry_interval: Duration,
    idempotency_lease: Duration,
    idempotency_retention: Duration,

    // --- NATS MQ (see src/events.rs) ---
    nats_url: String,
    nats_enabled: bool,
    nats_intake_enabled: bool,
    nats_event_subject: String,
    nats_result_subject: String,
    nats_image_subject: String,
    nats_request_subject: String,
    nats_critical_subject: String,

    // --- Webhooks (see src/webhooks.rs) ---
    github_webhook_secret: Option<String>,
    registry_webhook_secret: Option<String>,
    webhook_rules: Vec<webhooks::WebhookRule>,

    // --- GitHub Actions secret sync (see src/gh_secrets.rs) ---
    gh_sync_enabled: bool,
    gh_sync_token: Option<String>,
    gh_sync_rules: Vec<gh_secrets::SyncRule>,
    gh_sync_interval: Duration,

    // --- gleam-lambda-runner executor (see src/lambda_exec.rs) ---
    lambda_executor_enabled: bool,
    lambda_url: String,
    lambda_function_id: Option<String>,
    lambda_auth_secret: Option<String>,
}

#[derive(Default)]
struct Counters {
    submitted: AtomicU64,
    running: AtomicU64,
    succeeded: AtomicU64,
    failed: AtomicU64,
    rejected: AtomicU64,
    command_failures: AtomicU64,
    ecr_logins: AtomicU64,
    ecr_login_failures: AtomicU64,
    locks_acquired: AtomicU64,
    lock_failures: AtomicU64,
    webhooks_received: AtomicU64,
    webhooks_rejected: AtomicU64,
    nats_published: AtomicU64,
    nats_publish_failures: AtomicU64,
    gh_secrets_synced: AtomicU64,
    gh_secret_sync_failures: AtomicU64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BuildRequest {
    schema_version: Option<String>,
    job_kind: Option<String>,
    repo_url: String,
    git_ref: Option<String>,
    #[serde(default)]
    image: String,
    /// Fixed operator-reviewed command pipeline for jobKind=run-profile.
    profile: Option<String>,
    context_dir: Option<String>,
    dockerfile: Option<String>,
    build_args: Option<BTreeMap<String, String>>,
    push: Option<bool>,
    deploy: Option<DeployRequest>,
    /// "local" (default: git + nerdctl + kubectl on this node) or "lambda"
    /// (forward to the gleam-lambda-runner build function).
    executor: Option<String>,
    /// Caller-supplied idempotency id for at-least-once transports
    /// (NATS/webhooks); duplicate ids are accepted-and-ignored.
    request_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeployRequest {
    kind: String,
    path: String,
    namespace: Option<String>,
    rollout: Option<String>,
    rollout_timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BuildJobRecord {
    id: String,
    status: BuildStatus,
    request: BuildRequest,
    /// Where the job came from: http | webhook | nats.
    source: String,
    /// Which executor runs it: local | lambda.
    executor: String,
    created_at_ms: u128,
    started_at_ms: Option<u128>,
    finished_at_ms: Option<u128>,
    log_path: String,
    error: Option<String>,
    /// fiducia.cloud lock key + fencing token, when coordination is enabled.
    lock_key: Option<String>,
    fencing_token: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
enum BuildStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HealthResponse {
    ok: bool,
    service: &'static str,
    auth_configured: bool,
    deploy_enabled: bool,
    push_enabled: bool,
    ecr_login_enabled: bool,
    allowed_repo_prefixes: Vec<String>,
    allowed_image_prefixes: Vec<String>,
    allowed_namespaces: Vec<String>,
    allowed_profiles: Vec<String>,
    allowed_profile_repo_prefixes: Vec<String>,
    queued: usize,
    running: u64,
}

#[derive(Debug, Clone)]
struct EcrImage {
    registry: String,
    region: String,
}

#[derive(Debug)]
struct AwsCredentials {
    access_key_id: String,
    secret_access_key: String,
    session_token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EcrAuthResponse {
    authorization_data: Vec<EcrAuthorizationData>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EcrAuthorizationData {
    authorization_token: String,
    proxy_endpoint: String,
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
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
                value.as_str(),
                "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
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

fn env_usize(key: &str, fallback: usize) -> usize {
    first_env(&[key])
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn parse_namespaces(value: &str) -> HashSet<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn parse_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

/// Inline JSON env var, or a file path env var (mounted ConfigMap), or None.
fn env_or_file(inline_key: &str, path_key: &str) -> Option<String> {
    if let Some(inline) = first_env(&[inline_key]) {
        return Some(inline);
    }
    let path = first_env(&[path_key])?;
    match std::fs::read_to_string(&path) {
        Ok(contents) => Some(contents),
        Err(error) => {
            tracing::error!("failed to read {path_key}={path}: {error}");
            None
        }
    }
}

fn config_from_env() -> Config {
    let webhook_rules = env_or_file(
        "BUILD_SERVER_WEBHOOK_RULES",
        "BUILD_SERVER_WEBHOOK_RULES_PATH",
    )
    .map(|raw| match webhooks::parse_rules(&raw) {
        Ok(rules) => rules,
        Err(error) => {
            tracing::error!("ignoring webhook rules: {error}");
            Vec::new()
        }
    })
    .unwrap_or_default();

    let gh_sync_rules = env_or_file(
        "BUILD_SERVER_GH_SYNC_RULES",
        "BUILD_SERVER_GH_SYNC_RULES_PATH",
    )
    .map(|raw| match gh_secrets::parse_rules(&raw) {
        Ok(rules) => rules,
        Err(error) => {
            tracing::error!("ignoring gh secret sync rules: {error}");
            Vec::new()
        }
    })
    .unwrap_or_default();

    let coordination_enabled = env_bool("BUILD_SERVER_COORDINATION_ENABLED", false);
    let github_token = first_env(&["BUILD_SERVER_GIT_TOKEN", "GH_PAT"]);
    let git_http_auth_header = github_token.as_deref().map(|token| {
        format!(
            "AUTHORIZATION: basic {}",
            BASE64.encode(format!("x-access-token:{token}"))
        )
    });
    let fiducia_url = env_value(
        "FIDUCIA_LOCK_URL",
        "http://fiducia-load-balance.fiducia.svc.cluster.local:8088",
    );
    let coordination_enabled = if coordination_enabled {
        match fiducia::validate_lock_url(&fiducia_url) {
            Ok(()) => true,
            Err(error) => {
                tracing::error!("disabling fiducia coordination: {error}");
                false
            }
        }
    } else {
        false
    };

    Config {
        work_root: PathBuf::from(env_value(
            "BUILD_SERVER_WORK_ROOT",
            "/var/lib/dd-build-server/jobs",
        )),
        git_bin: env_value("BUILD_SERVER_GIT_BIN", "git"),
        git_http_auth_header,
        nerdctl_bin: env_value("BUILD_SERVER_NERDCTL_BIN", "/usr/local/bin/nerdctl"),
        kubectl_bin: env_value("BUILD_SERVER_KUBECTL_BIN", "/usr/bin/kubectl"),
        tar_bin: env_value("BUILD_SERVER_TAR_BIN", "/bin/tar"),
        containerd_namespace: env_value("BUILD_SERVER_CONTAINERD_NAMESPACE", "k8s.io"),
        allowed_repo_prefixes: parse_csv(&env_value("BUILD_SERVER_ALLOWED_REPO_PREFIXES", "")),
        allowed_image_prefixes: parse_csv(&env_value("BUILD_SERVER_ALLOWED_IMAGE_PREFIXES", "")),
        allowed_namespaces: parse_namespaces(&env_value(
            "BUILD_SERVER_ALLOWED_NAMESPACES",
            "default",
        )),
        allowed_profiles: parse_namespaces(&env_value(
            "BUILD_SERVER_ALLOWED_PROFILES",
            &profiles::names().collect::<Vec<_>>().join(","),
        )),
        allowed_profile_repo_prefixes: parse_csv(&env_value(
            "BUILD_SERVER_ALLOWED_PROFILE_REPO_PREFIXES",
            "https://github.com/ORESoftware/,https://github.com/sonus-auris/,git@github.com:ORESoftware/,git@github.com:sonus-auris/",
        )),
        profile_cpus: env_value("BUILD_SERVER_PROFILE_CPUS", "4"),
        profile_memory: env_value("BUILD_SERVER_PROFILE_MEMORY", "8g"),
        profile_pids_limit: env_value("BUILD_SERVER_PROFILE_PIDS_LIMIT", "2048"),
        deploy_enabled: env_bool("BUILD_SERVER_DEPLOY_ENABLED", true),
        push_enabled: env_bool("BUILD_SERVER_PUSH_ENABLED", false),
        ecr_login_enabled: env_bool("BUILD_SERVER_ECR_LOGIN_ENABLED", true),
        aws_region: first_env(&["AWS_REGION", "AWS_DEFAULT_REGION"])
            .unwrap_or_else(|| "us-east-1".to_string()),
        job_timeout: Duration::from_secs(env_u64("BUILD_SERVER_JOB_TIMEOUT_SECONDS", 1_800)),
        job_deadline: Duration::from_secs(env_u64("BUILD_SERVER_JOB_DEADLINE_SECONDS", 3_600)),
        max_log_bytes: env_u64("BUILD_SERVER_MAX_LOG_BYTES", 4 * 1024 * 1024),
        max_jobs: env_usize("BUILD_SERVER_MAX_JOBS", 200),
        max_queued: env_usize("BUILD_SERVER_MAX_QUEUED", 32),
        keep_workdirs: env_bool("BUILD_SERVER_KEEP_WORKDIRS", false),
        server_auth_secret: first_env(&["BUILD_SERVER_AUTH_SECRET", "SERVER_AUTH_SECRET"]),

        database_url: first_env(&["BUILD_SERVER_DATABASE_URL", "DATABASE_URL"]),

        fiducia_url,
        fiducia_api_key: first_env(&["FIDUCIA_API_KEY"]),
        coordination_enabled,
        coordination_required: env_bool("BUILD_SERVER_COORDINATION_REQUIRED", false),
        lock_ttl: Duration::from_millis(env_u64("BUILD_SERVER_LOCK_TTL_MS", 3_900_000)),
        lock_wait_budget: Duration::from_millis(env_u64("BUILD_SERVER_LOCK_WAIT_MS", 120_000)),
        lock_retry_interval: Duration::from_millis(env_u64("BUILD_SERVER_LOCK_RETRY_MS", 3_000)),
        idempotency_lease: Duration::from_millis(env_u64(
            "BUILD_SERVER_IDEMPOTENCY_LEASE_MS",
            300_000,
        )),
        idempotency_retention: Duration::from_millis(env_u64(
            "BUILD_SERVER_IDEMPOTENCY_RETENTION_MS",
            7 * 24 * 3_600_000,
        )),

        nats_url: env_value(
            "NATS_URL",
            "nats://dd-nats.messaging.svc.cluster.local:4222",
        ),
        nats_enabled: env_bool("BUILD_SERVER_NATS_ENABLED", true),
        nats_intake_enabled: env_bool("BUILD_SERVER_NATS_INTAKE_ENABLED", false),
        nats_event_subject: env_value(
            "BUILD_SERVER_NATS_EVENT_SUBJECT",
            dd_nats_subject_defs::BUILD_SERVER_EVENTS_SUBJECT,
        ),
        nats_result_subject: env_value(
            "BUILD_SERVER_NATS_RESULT_SUBJECT",
            dd_nats_subject_defs::BUILD_SERVER_RESULTS_SUBJECT,
        ),
        nats_image_subject: env_value(
            "BUILD_SERVER_NATS_IMAGE_SUBJECT",
            dd_nats_subject_defs::BUILD_SERVER_IMAGES_SUBJECT,
        ),
        nats_request_subject: env_value(
            "BUILD_SERVER_NATS_REQUEST_SUBJECT",
            dd_nats_subject_defs::BUILD_SERVER_REQUESTS_SUBJECT,
        ),
        nats_critical_subject: env_value(
            "NATS_CRITICAL_EVENT_SUBJECT",
            dd_nats_subject_defs::RUNTIME_CRITICAL_EVENTS_SUBJECT,
        ),

        github_webhook_secret: first_env(&["BUILD_SERVER_GITHUB_WEBHOOK_SECRET"]),
        registry_webhook_secret: first_env(&["BUILD_SERVER_REGISTRY_WEBHOOK_SECRET"]),
        webhook_rules,

        gh_sync_enabled: env_bool("BUILD_SERVER_GH_SYNC_ENABLED", false),
        gh_sync_token: first_env(&["GH_SECRETS_SYNC_TOKEN", "GH_PAT", "GITHUB_TOKEN"]),
        gh_sync_rules,
        gh_sync_interval: Duration::from_secs(
            first_env(&["BUILD_SERVER_GH_SYNC_INTERVAL_SECONDS"])
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0),
        ),

        lambda_executor_enabled: env_bool("BUILD_SERVER_LAMBDA_ENABLED", false),
        lambda_url: env_value(
            "BUILD_SERVER_LAMBDA_URL",
            "http://dd-gleam-lambda-runner.default.svc.cluster.local:8083",
        ),
        lambda_function_id: first_env(&["BUILD_SERVER_LAMBDA_FUNCTION_ID"]),
        lambda_auth_secret: first_env(&["BUILD_SERVER_LAMBDA_AUTH_SECRET"]),
    }
}

fn request_is_authorized(headers: &HeaderMap, secret: &str) -> bool {
    headers
        .get("x-server-auth")
        .or_else(|| headers.get("x-build-server-auth"))
        .or_else(|| headers.get("x-agent-auth"))
        .and_then(|value| value.to_str().ok())
        // Constant-time comparison of digests: no timing side channel and no
        // length leak from the shared secret.
        .is_some_and(|value| {
            let presented = Sha256::digest(value.as_bytes());
            let expected = Sha256::digest(secret.as_bytes());
            presented.as_slice().ct_eq(expected.as_slice()).into()
        })
}

fn require_auth(headers: &HeaderMap, state: &AppState) -> Result<(), Response> {
    let Some(secret) = state.config.server_auth_secret.as_deref() else {
        state.counters.rejected.fetch_add(1, Ordering::Relaxed);
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "SERVER_AUTH_SECRET is not configured" })),
        )
            .into_response());
    };
    if !request_is_authorized(headers, secret) {
        state.counters.rejected.fetch_add(1, Ordering::Relaxed);
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "error": "unauthorized",
                "errMessage": "missing required build server auth header",
            })),
        )
            .into_response());
    }
    Ok(())
}

fn clean_optional(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn ensure_allowed_prefix(
    name: &str,
    value: &str,
    prefixes: &[String],
    env_name: &str,
) -> Result<(), String> {
    if prefixes.is_empty() || prefixes.iter().any(|prefix| value.starts_with(prefix)) {
        Ok(())
    } else {
        Err(format!("{name} is not allowed by {env_name}"))
    }
}

fn validate_no_whitespace(name: &str, value: &str, max_len: usize) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{name} must not be empty"));
    }
    if value.len() > max_len {
        return Err(format!("{name} must be {max_len} characters or fewer"));
    }
    if value.chars().any(char::is_whitespace) {
        return Err(format!("{name} must not contain whitespace"));
    }
    if value.chars().any(char::is_control) {
        return Err(format!("{name} must not contain control characters"));
    }
    Ok(())
}

fn validate_repo_url(repo_url: &str) -> Result<(), String> {
    let repo_url = repo_url.trim();
    if repo_url.is_empty() {
        return Err("repoUrl is required".to_string());
    }
    if repo_url.len() > 2048 {
        return Err("repoUrl must be 2048 characters or fewer".to_string());
    }
    if repo_url.chars().any(char::is_control) {
        return Err("repoUrl must not contain control characters".to_string());
    }
    if repo_url.starts_with("https://")
        || repo_url.starts_with("ssh://")
        || repo_url.starts_with("git@")
    {
        Ok(())
    } else {
        Err("repoUrl must use https://, ssh://, or git@".to_string())
    }
}

fn has_explicit_image_version(image: &str) -> bool {
    let last_path = image.rsplit('/').next().unwrap_or(image);
    image.contains('@') || last_path.contains(':')
}

fn image_registry(image: &str) -> Option<&str> {
    let first = image.split('/').next().unwrap_or_default();
    if first.contains('.') || first.contains(':') || first == "localhost" {
        Some(first)
    } else {
        None
    }
}

fn ecr_image(image: &str) -> Option<EcrImage> {
    let registry = image_registry(image)?;
    let parts = registry.split('.').collect::<Vec<_>>();
    if parts.len() >= 6
        && parts[1] == "dkr"
        && parts[2] == "ecr"
        && parts[4] == "amazonaws"
        && (parts[5] == "com" || parts[5] == "com.cn")
    {
        return Some(EcrImage {
            registry: registry.to_string(),
            region: parts[3].to_string(),
        });
    }
    None
}

fn validate_image(config: &Config, image: &str, push: bool) -> Result<Option<EcrImage>, String> {
    validate_no_whitespace("image", image, 512)?;
    if !has_explicit_image_version(image) {
        return Err("image must include an explicit tag or digest".to_string());
    }
    ensure_allowed_prefix(
        "image",
        image,
        &config.allowed_image_prefixes,
        "BUILD_SERVER_ALLOWED_IMAGE_PREFIXES",
    )?;
    let ecr = ecr_image(image);
    if push && config.ecr_login_enabled && ecr.is_none() {
        return Err(
            "push currently requires an Amazon ECR image when ECR login is enabled".to_string(),
        );
    }
    Ok(ecr)
}

fn validate_relative_path(name: &str, value: &str) -> Result<PathBuf, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{name} must not be empty"));
    }
    if trimmed.len() > 240 {
        return Err(format!("{name} must be 240 characters or fewer"));
    }
    let path = Path::new(trimmed);
    if path.is_absolute() {
        return Err(format!("{name} must be relative to the repository root"));
    }

    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => clean.push(value),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!("{name} must stay inside the repository root"));
            }
        }
    }

    if clean.as_os_str().is_empty() {
        clean.push(".");
    }
    Ok(clean)
}

fn validate_build_args(build_args: &Option<BTreeMap<String, String>>) -> Result<(), String> {
    let Some(build_args) = build_args else {
        return Ok(());
    };
    if build_args.len() > 32 {
        return Err("buildArgs can contain at most 32 entries".to_string());
    }
    for (key, value) in build_args {
        if key.is_empty() || key.len() > 80 {
            return Err("build arg keys must be 1-80 characters".to_string());
        }
        let upper_key = key.to_ascii_uppercase();
        if ["SECRET", "PASSWORD", "TOKEN", "CREDENTIAL", "PRIVATE_KEY"]
            .iter()
            .any(|part| upper_key.contains(part))
        {
            return Err(format!(
                "build arg key {key:?} looks secret-like; use registry/repo credentials, not Docker build args"
            ));
        }
        if !key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
        {
            return Err(format!(
                "build arg key {key:?} contains unsupported characters"
            ));
        }
        if value.len() > 1024 || value.chars().any(char::is_control) {
            return Err(format!(
                "build arg {key:?} must be printable and 1024 characters or fewer"
            ));
        }
    }
    Ok(())
}

fn validate_namespace(config: &Config, namespace: &str) -> Result<(), String> {
    validate_no_whitespace("deploy.namespace", namespace, 63)?;
    if !config.allowed_namespaces.contains(namespace) {
        return Err(format!(
            "namespace {namespace:?} is not allowed by BUILD_SERVER_ALLOWED_NAMESPACES"
        ));
    }
    Ok(())
}

fn validate_rollout_resource(value: &str) -> Result<String, String> {
    let value = value.trim();
    validate_no_whitespace("deploy.rollout", value, 160)?;
    if value.contains("..") {
        return Err("deploy.rollout must not contain '..'".to_string());
    }
    if value.contains('/') {
        Ok(value.to_string())
    } else {
        Ok(format!("deployment/{value}"))
    }
}

fn validate_deploy(config: &Config, deploy: &Option<DeployRequest>) -> Result<(), String> {
    let Some(deploy) = deploy else {
        return Ok(());
    };
    match deploy.kind.as_str() {
        "kustomize" | "manifest" | "none" => {}
        _ => return Err("deploy.kind must be one of: kustomize, manifest, none".to_string()),
    }
    if deploy.kind == "none" {
        return Ok(());
    }
    if !config.deploy_enabled {
        return Err("deploy is disabled by BUILD_SERVER_DEPLOY_ENABLED=false".to_string());
    }
    validate_relative_path("deploy.path", &deploy.path)?;
    let namespace = deploy.namespace.as_deref().unwrap_or("default");
    validate_namespace(config, namespace)?;
    if let Some(rollout) = deploy.rollout.as_deref() {
        validate_rollout_resource(rollout)?;
    }
    Ok(())
}

fn validate_build_request(config: &Config, request: &BuildRequest) -> Result<(), String> {
    if let Some(schema_version) = clean_optional(request.schema_version.as_deref()) {
        if schema_version != "build-server.v1" {
            return Err("schemaVersion must be build-server.v1".to_string());
        }
    }
    let job_kind =
        clean_optional(request.job_kind.as_deref()).unwrap_or_else(|| "build-image".to_string());
    if !matches!(
        job_kind.as_str(),
        "build-image" | "build-and-deploy" | "run-profile"
    ) {
        return Err("jobKind must be build-image, build-and-deploy, or run-profile".to_string());
    }
    validate_repo_url(&request.repo_url)?;
    ensure_allowed_prefix(
        "repoUrl",
        &request.repo_url,
        &config.allowed_repo_prefixes,
        "BUILD_SERVER_ALLOWED_REPO_PREFIXES",
    )?;
    if job_kind == "run-profile" {
        ensure_allowed_prefix(
            "profile repoUrl",
            &request.repo_url,
            &config.allowed_profile_repo_prefixes,
            "BUILD_SERVER_ALLOWED_PROFILE_REPO_PREFIXES",
        )?;
        let profile = clean_optional(request.profile.as_deref())
            .ok_or_else(|| "profile is required for jobKind=run-profile".to_string())?;
        if profiles::find(&profile).is_none() || !config.allowed_profiles.contains(&profile) {
            return Err(format!(
                "profile {profile:?} is not allowed by BUILD_SERVER_ALLOWED_PROFILES"
            ));
        }
        if !request.image.trim().is_empty() {
            return Err("image must be omitted for jobKind=run-profile".to_string());
        }
        if request.push.unwrap_or(false)
            || request.deploy.is_some()
            || request.build_args.is_some()
            || request.dockerfile.is_some()
        {
            return Err(
                "run-profile does not accept image, push, deploy, buildArgs, or dockerfile"
                    .to_string(),
            );
        }
    } else {
        if request.profile.is_some() {
            return Err("profile is only valid for jobKind=run-profile".to_string());
        }
        validate_image(config, &request.image, request.push.unwrap_or(false))?;
    }
    if let Some(git_ref) = clean_optional(request.git_ref.as_deref()) {
        validate_no_whitespace("gitRef", &git_ref, 180)?;
    }
    validate_relative_path("contextDir", request.context_dir.as_deref().unwrap_or("."))?;
    if job_kind != "run-profile" {
        validate_relative_path(
            "dockerfile",
            request.dockerfile.as_deref().unwrap_or("Dockerfile"),
        )?;
        validate_build_args(&request.build_args)?;
    }
    if request.push.unwrap_or(false) && !config.push_enabled {
        return Err("push is disabled by BUILD_SERVER_PUSH_ENABLED=false".to_string());
    }
    match request.executor.as_deref() {
        None | Some("local") => {}
        Some("lambda") => {
            if job_kind == "run-profile" {
                return Err("run-profile currently requires executor=local".to_string());
            }
            if !config.lambda_executor_enabled {
                return Err(
                    "executor \"lambda\" is disabled by BUILD_SERVER_LAMBDA_ENABLED=false"
                        .to_string(),
                );
            }
        }
        Some(other) => return Err(format!("executor {other:?} must be local or lambda")),
    }
    if let Some(request_id) = clean_optional(request.request_id.as_deref()) {
        validate_no_whitespace("requestId", &request_id, 128)?;
    }
    if job_kind == "run-profile" {
        Ok(())
    } else {
        validate_deploy(config, &request.deploy)
    }
}

fn request_job_kind(request: &BuildRequest) -> String {
    clean_optional(request.job_kind.as_deref()).unwrap_or_else(|| "build-image".to_string())
}

fn shellish(value: &str) -> String {
    if value.chars().all(|ch| {
        ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | ':' | '=' | '@')
    }) {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

fn printable_command(program: &str, args: &[String]) -> String {
    std::iter::once(program.to_string())
        .chain(args.iter().cloned())
        .map(|value| shellish(&value))
        .collect::<Vec<_>>()
        .join(" ")
}

fn redacted_build_args(args: &[String]) -> Vec<String> {
    let mut redacted = Vec::with_capacity(args.len());
    let mut redact_next = false;
    for arg in args {
        if redact_next {
            let key = arg.split_once('=').map(|(key, _)| key).unwrap_or(arg);
            redacted.push(format!("{key}=<redacted>"));
            redact_next = false;
            continue;
        }
        redacted.push(arg.clone());
        if arg == "--build-arg" {
            redact_next = true;
        }
    }
    redacted
}

async fn append_log(path: &Path, message: &str, max_bytes: u64) {
    let current_len = fs::metadata(path).await.map(|meta| meta.len()).unwrap_or(0);
    if current_len >= max_bytes {
        return;
    }
    let remaining = (max_bytes - current_len) as usize;
    let bytes = message.as_bytes();
    let limit = remaining.min(bytes.len());
    if limit == 0 {
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent).await;
    }
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
    {
        let _ = file.write_all(&bytes[..limit]).await;
    }
}

async fn pipe_reader<R>(reader: R, log_path: PathBuf, prefix: &'static str, max_bytes: u64)
where
    R: AsyncRead + Unpin,
{
    let mut reader = BufReader::new(reader);
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line).await {
            Ok(0) => break,
            Ok(_) => {
                let text = String::from_utf8_lossy(&line);
                append_log(&log_path, &format!("{prefix}{text}"), max_bytes).await;
            }
            Err(error) => {
                append_log(
                    &log_path,
                    &format!("{prefix}failed to read command output: {error}\n"),
                    max_bytes,
                )
                .await;
                break;
            }
        }
    }
}

async fn run_logged_command(
    config: &Config,
    log_path: &Path,
    cwd: &Path,
    program: &str,
    args: Vec<String>,
) -> Result<(), String> {
    run_logged_command_inner(config, log_path, cwd, program, args, None, None).await
}

async fn run_logged_command_with_input(
    config: &Config,
    log_path: &Path,
    cwd: &Path,
    program: &str,
    args: Vec<String>,
    display_args: Vec<String>,
    stdin: Vec<u8>,
) -> Result<(), String> {
    run_logged_command_inner(
        config,
        log_path,
        cwd,
        program,
        args,
        Some(display_args),
        Some(stdin),
    )
    .await
}

async fn run_logged_command_inner(
    config: &Config,
    log_path: &Path,
    cwd: &Path,
    program: &str,
    args: Vec<String>,
    display_args: Option<Vec<String>>,
    stdin: Option<Vec<u8>>,
) -> Result<(), String> {
    let display_args = display_args.unwrap_or_else(|| args.clone());
    append_log(
        log_path,
        &format!("\n$ {}\n", printable_command(program, &display_args)),
        config.max_log_bytes,
    )
    .await;

    let mut command = Command::new(program);
    command
        .args(&args)
        .current_dir(cwd)
        .env_clear()
        .env("HOME", cwd)
        .env(
            "PATH",
            "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        )
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "/bin/false")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if program == config.git_bin {
        if let Some(auth_header) = config.git_http_auth_header.as_deref() {
            command
                .env("GIT_CONFIG_COUNT", "1")
                .env("GIT_CONFIG_KEY_0", "http.https://github.com/.extraheader")
                .env("GIT_CONFIG_VALUE_0", auth_header);
        }
    }
    if stdin.is_some() {
        command.stdin(Stdio::piped());
    }
    for key in [
        "KUBERNETES_SERVICE_HOST",
        "KUBERNETES_SERVICE_PORT",
        "KUBERNETES_SERVICE_PORT_HTTPS",
    ] {
        if let Ok(value) = env::var(key) {
            command.env(key, value);
        }
    }

    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to spawn {program}: {error}"))?;
    if let Some(input) = stdin {
        if let Some(mut child_stdin) = child.stdin.take() {
            child_stdin
                .write_all(&input)
                .await
                .map_err(|error| format!("failed to write stdin for {program}: {error}"))?;
        }
    }

    let stdout_task = child.stdout.take().map(|stdout| {
        tokio::spawn(pipe_reader(
            stdout,
            log_path.to_path_buf(),
            "",
            config.max_log_bytes,
        ))
    });
    let stderr_task = child.stderr.take().map(|stderr| {
        tokio::spawn(pipe_reader(
            stderr,
            log_path.to_path_buf(),
            "",
            config.max_log_bytes,
        ))
    });

    let status = match timeout(config.job_timeout, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(error)) => return Err(format!("{program} failed to wait: {error}")),
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(format!(
                "{program} timed out after {:?}",
                config.job_timeout
            ));
        }
    };

    if let Some(task) = stdout_task {
        let _ = task.await;
    }
    if let Some(task) = stderr_task {
        let _ = task.await;
    }

    append_log(
        log_path,
        &format!("exit status: {status}\n"),
        config.max_log_bytes,
    )
    .await;

    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} exited with {status}"))
    }
}

type HmacSha256 = Hmac<Sha256>;

fn hmac_sha256(key: &[u8], value: &str) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts keys of any size");
    mac.update(value.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

fn sha256_hex(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn aws_timestamp() -> (String, String) {
    let now = OffsetDateTime::now_utc();
    let date = format!(
        "{:04}{:02}{:02}",
        now.year(),
        u8::from(now.month()),
        now.day()
    );
    let timestamp = format!(
        "{}T{:02}{:02}{:02}Z",
        date,
        now.hour(),
        now.minute(),
        now.second()
    );
    (date, timestamp)
}

fn aws_credentials_from_env() -> Result<AwsCredentials, String> {
    let access_key_id = first_env(&["AWS_ACCESS_KEY_ID"])
        .ok_or_else(|| "AWS_ACCESS_KEY_ID is required for ECR push".to_string())?;
    let secret_access_key = first_env(&["AWS_SECRET_ACCESS_KEY"])
        .ok_or_else(|| "AWS_SECRET_ACCESS_KEY is required for ECR push".to_string())?;
    let session_token = first_env(&["AWS_SESSION_TOKEN"]);
    Ok(AwsCredentials {
        access_key_id,
        secret_access_key,
        session_token,
    })
}

fn ecr_headers(
    config: &Config,
    credentials: &AwsCredentials,
    region: &str,
    host: &str,
    body: &str,
) -> Result<ReqwestHeaderMap, String> {
    let target = "AmazonEC2ContainerRegistry_V20150921.GetAuthorizationToken";
    let content_type = "application/x-amz-json-1.1";
    let (date, timestamp) = aws_timestamp();
    let session_token = credentials.session_token.as_deref().unwrap_or("");
    let (canonical_headers, signed_headers) = if session_token.is_empty() {
        (
            format!("content-type:{content_type}\nhost:{host}\nx-amz-date:{timestamp}\nx-amz-target:{target}\n"),
            "content-type;host;x-amz-date;x-amz-target",
        )
    } else {
        (
            format!(
                "content-type:{content_type}\nhost:{host}\nx-amz-date:{timestamp}\nx-amz-security-token:{session_token}\nx-amz-target:{target}\n"
            ),
            "content-type;host;x-amz-date;x-amz-security-token;x-amz-target",
        )
    };
    let canonical_request = format!(
        "POST\n/\n\n{canonical_headers}\n{signed_headers}\n{}",
        sha256_hex(body)
    );
    let credential_scope = format!("{date}/{region}/ecr/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{timestamp}\n{credential_scope}\n{}",
        sha256_hex(&canonical_request)
    );
    let date_key = hmac_sha256(
        format!("AWS4{}", credentials.secret_access_key).as_bytes(),
        &date,
    );
    let region_key = hmac_sha256(&date_key, region);
    let service_key = hmac_sha256(&region_key, "ecr");
    let signing_key = hmac_sha256(&service_key, "aws4_request");
    let signature = hex::encode(hmac_sha256(&signing_key, &string_to_sign));
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{credential_scope}, SignedHeaders={signed_headers}, Signature={signature}",
        credentials.access_key_id
    );

    let mut headers = ReqwestHeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static(content_type));
    headers.insert(
        "x-amz-date",
        HeaderValue::from_str(&timestamp).map_err(|error| error.to_string())?,
    );
    headers.insert("x-amz-target", HeaderValue::from_static(target));
    headers.insert(
        "authorization",
        HeaderValue::from_str(&authorization).map_err(|error| error.to_string())?,
    );
    if !session_token.is_empty() {
        headers.insert(
            "x-amz-security-token",
            HeaderValue::from_str(session_token).map_err(|error| error.to_string())?,
        );
    }
    headers.insert(
        "user-agent",
        HeaderValue::from_str(&format!("{SERVICE_NAME}/0.1 ({})", config.aws_region))
            .map_err(|error| error.to_string())?,
    );
    Ok(headers)
}

async fn ecr_authorization_password(state: &AppState, ecr: &EcrImage) -> Result<String, String> {
    let credentials = aws_credentials_from_env()?;
    let body = "{}";
    let host = format!("api.ecr.{}.amazonaws.com", ecr.region);
    let headers = ecr_headers(&state.config, &credentials, &ecr.region, &host, body)?;
    let response = state
        .http
        .post(format!("https://{host}/"))
        .headers(headers)
        .body(body.to_string())
        .send()
        .await
        .map_err(|error| format!("failed to request ECR authorization token: {error}"))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|error| format!("failed to read ECR authorization response: {error}"))?;
    if !status.is_success() {
        return Err(format!(
            "ECR authorization failed with HTTP {}: {}",
            status.as_u16(),
            text.chars().take(400).collect::<String>()
        ));
    }
    let parsed: EcrAuthResponse = serde_json::from_str(&text)
        .map_err(|error| format!("failed to parse ECR authorization response: {error}"))?;
    let data = parsed
        .authorization_data
        .iter()
        .find(|data| data.proxy_endpoint.trim_start_matches("https://") == ecr.registry)
        .or_else(|| parsed.authorization_data.first())
        .ok_or_else(|| {
            "ECR authorization response did not include authorizationData".to_string()
        })?;
    let decoded = BASE64
        .decode(&data.authorization_token)
        .map_err(|error| format!("failed to decode ECR authorization token: {error}"))?;
    let decoded = String::from_utf8(decoded)
        .map_err(|error| format!("ECR authorization token was not UTF-8: {error}"))?;
    let Some((username, password)) = decoded.split_once(':') else {
        return Err("ECR authorization token did not contain username/password".to_string());
    };
    if username != "AWS" {
        return Err("ECR authorization token had an unexpected username".to_string());
    }
    Ok(password.to_string())
}

async fn login_to_ecr(
    state: &AppState,
    log_path: &Path,
    cwd: &Path,
    ecr: &EcrImage,
) -> Result<(), String> {
    append_log(
        log_path,
        &format!("requesting ECR login token for {}\n", ecr.registry),
        state.config.max_log_bytes,
    )
    .await;
    let password = ecr_authorization_password(state, ecr).await?;
    let args = vec![
        "-n".to_string(),
        state.config.containerd_namespace.clone(),
        "login".to_string(),
        "--username".to_string(),
        "AWS".to_string(),
        "--password-stdin".to_string(),
        ecr.registry.clone(),
    ];
    let display_args = args.clone();
    run_logged_command_with_input(
        &state.config,
        log_path,
        cwd,
        &state.config.nerdctl_bin,
        args,
        display_args,
        format!("{password}\n").into_bytes(),
    )
    .await
}

fn job_id(counter: u64) -> String {
    format!("build-{}-{counter}", now_ms())
}

async fn update_job<F>(state: &AppState, id: &str, mutate: F)
where
    F: FnOnce(&mut BuildJobRecord),
{
    let updated = {
        let mut jobs = state.jobs.write().await;
        match jobs.get_mut(id) {
            Some(job) => {
                mutate(job);
                Some(job.clone())
            }
            None => None,
        }
    };
    if let Some(job) = updated {
        if let Some(db) = state.db.as_ref() {
            db::persist_job(db, &job).await;
        }
        events::publish_lifecycle(state, &job).await;
    }
}

async fn prune_jobs(state: &AppState) {
    let max_jobs = state.config.max_jobs;
    let mut jobs = state.jobs.write().await;
    if jobs.len() <= max_jobs {
        return;
    }

    let mut candidates = jobs
        .values()
        .filter(|job| !matches!(job.status, BuildStatus::Queued | BuildStatus::Running))
        .map(|job| (job.created_at_ms, job.id.clone()))
        .collect::<Vec<_>>();
    candidates.sort_by_key(|(created_at_ms, _)| *created_at_ms);
    for (_, id) in candidates
        .into_iter()
        .take(jobs.len().saturating_sub(max_jobs))
    {
        jobs.remove(&id);
    }
}

fn resolve_repo_path(repo_dir: &Path, name: &str, value: &str) -> Result<PathBuf, String> {
    let clean = validate_relative_path(name, value)?;
    Ok(repo_dir.join(clean))
}

async fn clone_repository(
    config: &Config,
    request: &BuildRequest,
    job_dir: &Path,
    repo_dir: &Path,
    log_path: &Path,
) -> Result<(), String> {
    let mut clone_args = vec![
        "-c".to_string(),
        "protocol.ext.allow=never".to_string(),
        "-c".to_string(),
        "protocol.file.allow=never".to_string(),
        "-c".to_string(),
        "protocol.local.allow=never".to_string(),
        "clone".to_string(),
        "--depth".to_string(),
        "1".to_string(),
        "--no-tags".to_string(),
    ];
    if let Some(git_ref) = clean_optional(request.git_ref.as_deref()) {
        clone_args.push("--branch".to_string());
        clone_args.push(git_ref);
    }
    clone_args.push("--".to_string());
    clone_args.push(request.repo_url.clone());
    clone_args.push(repo_dir.to_string_lossy().to_string());
    run_logged_command(config, log_path, job_dir, &config.git_bin, clone_args).await
}

async fn execute_profile(state: &AppState, job: &BuildJobRecord) -> Result<(), String> {
    let config = state.config.as_ref();
    let request = &job.request;
    let profile_name = clean_optional(request.profile.as_deref())
        .ok_or_else(|| "validated profile request lost its profile".to_string())?;
    let profile = profiles::find(&profile_name)
        .ok_or_else(|| format!("profile {profile_name:?} is not installed"))?;
    let job_dir = config.work_root.join(&job.id);
    let repo_dir = job_dir.join("repo");
    let log_path = PathBuf::from(&job.log_path);

    fs::create_dir_all(&job_dir)
        .await
        .map_err(|error| format!("failed to create job dir: {error}"))?;
    append_log(
        &log_path,
        &format!(
            "{SERVICE_NAME} starting profile job={} repo={} profile={}\n",
            job.id, request.repo_url, profile.name
        ),
        config.max_log_bytes,
    )
    .await;
    clone_repository(config, request, &job_dir, &repo_dir, &log_path).await?;

    let context_path = resolve_repo_path(
        &repo_dir,
        "contextDir",
        request.context_dir.as_deref().unwrap_or("."),
    )?;
    for step in profile.steps {
        let step_cwd = validate_relative_path("profile step subdirectory", step.subdirectory)?;
        let container_cwd = if step_cwd == Path::new(".") {
            "/workspace".to_string()
        } else {
            format!("/workspace/{}", step_cwd.to_string_lossy())
        };
        append_log(
            &log_path,
            &format!(
                "\nprofile={} step={} runner={}\n",
                profile.name, step.name, step.image
            ),
            config.max_log_bytes,
        )
        .await;
        let mut runner_args = vec![
            "-n".to_string(),
            config.containerd_namespace.clone(),
            "run".to_string(),
            "--rm".to_string(),
            "--pull=missing".to_string(),
            format!("--cpus={}", config.profile_cpus),
            format!("--memory={}", config.profile_memory),
            format!("--pids-limit={}", config.profile_pids_limit),
            "--security-opt=no-new-privileges".to_string(),
            "--cap-drop=ALL".to_string(),
        ];
        runner_args.extend(
            step.capabilities
                .iter()
                .map(|capability| format!("--cap-add={capability}")),
        );
        runner_args.extend([
            "--env=CI=true".to_string(),
            "--mount".to_string(),
            format!(
                "type=bind,src={},dst=/workspace",
                context_path.to_string_lossy()
            ),
            "--workdir".to_string(),
            container_cwd,
            step.image.to_string(),
            "/bin/bash".to_string(),
            "-lc".to_string(),
            step.script.to_string(),
        ]);
        run_logged_command(
            config,
            &log_path,
            &context_path,
            &config.nerdctl_bin,
            runner_args,
        )
        .await?;
    }

    if !profile.artifact_paths.is_empty() {
        let archive_path = job_dir.join("artifacts.tar.gz");
        let mut args = vec![
            "-czf".to_string(),
            archive_path.to_string_lossy().to_string(),
            "--".to_string(),
        ];
        args.extend(profile.artifact_paths.iter().map(|path| path.to_string()));
        run_logged_command(config, &log_path, &context_path, &config.tar_bin, args).await?;
        append_log(
            &log_path,
            &format!("artifacts: /builds/{}/artifacts\n", job.id),
            config.max_log_bytes,
        )
        .await;
    }

    append_log(
        &log_path,
        &format!("{SERVICE_NAME} completed profile job={}\n", job.id),
        config.max_log_bytes,
    )
    .await;
    Ok(())
}

async fn execute_build(state: &AppState, job: &BuildJobRecord) -> Result<(), String> {
    let config = state.config.as_ref();
    let request = &job.request;
    let job_dir = config.work_root.join(&job.id);
    let repo_dir = job_dir.join("repo");
    let log_path = PathBuf::from(&job.log_path);

    fs::create_dir_all(&job_dir)
        .await
        .map_err(|error| format!("failed to create job dir: {error}"))?;
    append_log(
        &log_path,
        &format!(
            "{SERVICE_NAME} starting job={} repo={} image={}\n",
            job.id, request.repo_url, request.image
        ),
        config.max_log_bytes,
    )
    .await;

    // Locked-down clone: no non-network transports, no tags, and an explicit
    // `--` so nothing user-supplied can ever be parsed as a git option.
    clone_repository(config, request, &job_dir, &repo_dir, &log_path).await?;

    let context_path = resolve_repo_path(
        &repo_dir,
        "contextDir",
        request.context_dir.as_deref().unwrap_or("."),
    )?;
    let dockerfile_path = resolve_repo_path(
        &repo_dir,
        "dockerfile",
        request.dockerfile.as_deref().unwrap_or("Dockerfile"),
    )?;

    let mut build_args = vec![
        "-n".to_string(),
        config.containerd_namespace.clone(),
        "build".to_string(),
        "-f".to_string(),
        dockerfile_path.to_string_lossy().to_string(),
        "-t".to_string(),
        request.image.clone(),
    ];
    if let Some(args) = &request.build_args {
        for (key, value) in args {
            build_args.push("--build-arg".to_string());
            build_args.push(format!("{key}={value}"));
        }
    }
    build_args.push(context_path.to_string_lossy().to_string());
    let display_build_args = redacted_build_args(&build_args);
    run_logged_command_inner(
        config,
        &log_path,
        &repo_dir,
        &config.nerdctl_bin,
        build_args,
        Some(display_build_args),
        None,
    )
    .await?;

    if request.push.unwrap_or(false) {
        let ecr = validate_image(config, &request.image, true)?;
        if config.ecr_login_enabled {
            let ecr = ecr.ok_or_else(|| "push requires an ECR image".to_string())?;
            match login_to_ecr(state, &log_path, &repo_dir, &ecr).await {
                Ok(()) => {
                    state.counters.ecr_logins.fetch_add(1, Ordering::Relaxed);
                }
                Err(error) => {
                    state
                        .counters
                        .ecr_login_failures
                        .fetch_add(1, Ordering::Relaxed);
                    return Err(error);
                }
            }
        }
        run_logged_command(
            config,
            &log_path,
            &repo_dir,
            &config.nerdctl_bin,
            vec![
                "-n".to_string(),
                config.containerd_namespace.clone(),
                "push".to_string(),
                request.image.clone(),
            ],
        )
        .await?;
    }

    if let Some(deploy) = &request.deploy {
        if deploy.kind != "none" {
            let namespace = deploy.namespace.as_deref().unwrap_or("default");
            let deploy_path = resolve_repo_path(&repo_dir, "deploy.path", &deploy.path)?;
            let mut apply_args = vec!["-n".to_string(), namespace.to_string(), "apply".to_string()];
            match deploy.kind.as_str() {
                "kustomize" => {
                    apply_args.push("-k".to_string());
                    apply_args.push(deploy_path.to_string_lossy().to_string());
                }
                "manifest" => {
                    apply_args.push("-f".to_string());
                    apply_args.push(deploy_path.to_string_lossy().to_string());
                }
                _ => unreachable!("deploy kind is validated before queueing"),
            }
            run_logged_command(
                config,
                &log_path,
                &repo_dir,
                &config.kubectl_bin,
                apply_args,
            )
            .await?;

            if let Some(rollout) = deploy.rollout.as_deref() {
                let resource = validate_rollout_resource(rollout)?;
                let timeout_seconds = deploy.rollout_timeout_seconds.unwrap_or(300);
                run_logged_command(
                    config,
                    &log_path,
                    &repo_dir,
                    &config.kubectl_bin,
                    vec![
                        "-n".to_string(),
                        namespace.to_string(),
                        "rollout".to_string(),
                        "status".to_string(),
                        resource,
                        format!("--timeout={timeout_seconds}s"),
                    ],
                )
                .await?;
            }
        }
    }

    append_log(
        &log_path,
        &format!("{SERVICE_NAME} completed job={}\n", job.id),
        config.max_log_bytes,
    )
    .await;
    Ok(())
}

async fn run_job(state: AppState, id: String) {
    let permit = match state.semaphore.clone().acquire_owned().await {
        Ok(permit) => permit,
        Err(error) => {
            update_job(&state, &id, |job| {
                job.status = BuildStatus::Failed;
                job.finished_at_ms = Some(now_ms());
                job.error = Some(format!("build queue is closed: {error}"));
            })
            .await;
            return;
        }
    };

    // Distributed mutual exclusion (fiducia.cloud): one lock per image ref, so
    // concurrent builds of the same image serialize across replicas. The local
    // semaphore above only bounds this process.
    let lock_key = {
        let jobs = state.jobs.read().await;
        jobs.get(&id).map(|job| {
            if request_job_kind(&job.request) == "run-profile" {
                format!(
                    "build-server/profile/{}/{}",
                    job.request.profile.as_deref().unwrap_or("unknown"),
                    sha256_hex(&job.request.repo_url)
                )
            } else {
                format!("build-server/image/{}", job.request.image)
            }
        })
    };
    let mut grant: Option<fiducia::LockGrant> = None;
    if let Some(lock_key) = lock_key.as_deref() {
        match fiducia::acquire_lock(&state.http, &state.config, lock_key, &state.holder).await {
            fiducia::LockOutcome::Disabled => {}
            fiducia::LockOutcome::Acquired(acquired) => {
                state
                    .counters
                    .locks_acquired
                    .fetch_add(1, Ordering::Relaxed);
                grant = Some(acquired);
            }
            fiducia::LockOutcome::Busy { key } => {
                state.counters.lock_failures.fetch_add(1, Ordering::Relaxed);
                state.counters.failed.fetch_add(1, Ordering::Relaxed);
                update_job(&state, &id, |job| {
                    job.status = BuildStatus::Failed;
                    job.finished_at_ms = Some(now_ms());
                    job.error = Some(format!(
                        "another build holds the fiducia lock for {key}; retry later"
                    ));
                })
                .await;
                drop(permit);
                return;
            }
            fiducia::LockOutcome::Unavailable { error } => {
                state.counters.lock_failures.fetch_add(1, Ordering::Relaxed);
                if state.config.coordination_required {
                    state.counters.failed.fetch_add(1, Ordering::Relaxed);
                    update_job(&state, &id, |job| {
                        job.status = BuildStatus::Failed;
                        job.finished_at_ms = Some(now_ms());
                        job.error = Some(format!(
                            "fiducia coordination is required but unavailable: {error}"
                        ));
                    })
                    .await;
                    drop(permit);
                    return;
                }
                tracing::warn!(
                    "fiducia coordination unavailable, continuing with local semaphore only: {error}"
                );
            }
        }
    }

    state.counters.running.fetch_add(1, Ordering::Relaxed);
    let grant_key = grant.as_ref().map(|grant| grant.key.clone());
    let grant_token = grant.as_ref().map(|grant| grant.fencing_token);
    update_job(&state, &id, |job| {
        job.status = BuildStatus::Running;
        job.started_at_ms = Some(now_ms());
        job.lock_key = grant_key.clone();
        job.fencing_token = grant_token;
    })
    .await;

    let job = {
        let jobs = state.jobs.read().await;
        jobs.get(&id).cloned()
    };

    let result = match job {
        Some(job) => {
            // Hard wall-clock deadline for the whole job, on top of the
            // per-command timeout inside the executors.
            let deadline = state.config.job_deadline;
            let execution = async {
                if request_job_kind(&job.request) == "run-profile" {
                    execute_profile(&state, &job).await
                } else if job.executor == "lambda" {
                    lambda_exec::execute(&state, &job, Path::new(&job.log_path)).await
                } else {
                    execute_build(&state, &job).await
                }
            };
            match timeout(deadline, execution).await {
                Ok(result) => result,
                Err(_) => Err(format!("job exceeded overall deadline of {deadline:?}")),
            }
        }
        None => Err("job disappeared before execution".to_string()),
    };

    if let Some(grant) = grant.as_ref() {
        fiducia::release_lock(&state.http, &state.config, grant).await;
    }

    // Workdir GC: the cloned repo is scratch space; keep only the build log
    // unless the operator opts into keeping workdirs for debugging.
    if !state.config.keep_workdirs {
        let repo_dir = state.config.work_root.join(&id).join("repo");
        if let Err(error) = fs::remove_dir_all(&repo_dir).await {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("failed to remove workdir {}: {error}", repo_dir.display());
            }
        }
    }

    state.counters.running.fetch_sub(1, Ordering::Relaxed);
    drop(permit);

    match result {
        Ok(()) => {
            state.counters.succeeded.fetch_add(1, Ordering::Relaxed);
            update_job(&state, &id, |job| {
                job.status = BuildStatus::Succeeded;
                job.finished_at_ms = Some(now_ms());
                job.error = None;
            })
            .await;
        }
        Err(error) => {
            state.counters.failed.fetch_add(1, Ordering::Relaxed);
            state
                .counters
                .command_failures
                .fetch_add(1, Ordering::Relaxed);
            update_job(&state, &id, |job| {
                job.status = BuildStatus::Failed;
                job.finished_at_ms = Some(now_ms());
                job.error = Some(error);
            })
            .await;
        }
    }
}

async fn descriptor(State(state): State<AppState>) -> impl IntoResponse {
    let config = &state.config;
    Json(json!({
        "service": SERVICE_NAME,
        "description": "Authenticated Rust build server for repo image builds and controlled Kubernetes deploys, with fiducia.cloud build locks, Postgres persistence, NATS events, webhooks, and GitHub secret sync.",
        "endpoints": {
            "submit": "POST /builds",
            "list": "GET /builds",
            "status": "GET /builds/<jobId>",
            "logs": "GET /builds/<jobId>/logs",
            "artifacts": "GET /builds/<jobId>/artifacts",
            "githubWebhook": "POST /webhooks/github",
            "registryWebhook": "POST /webhooks/registry",
            "syncSecrets": "POST /secrets/sync",
            "syncSecretsStatus": "GET /secrets/sync/status",
            "healthz": "GET /healthz",
            "metrics": "GET /metrics"
        },
        "jobSchema": {
            "schemaVersion": "build-server.v1",
            "jobKind": ["build-image", "build-and-deploy", "run-profile"],
            "required": ["repoUrl"],
            "conditional": {
                "build-image/build-and-deploy": ["image"],
                "run-profile": ["profile"]
            },
            "optional": ["gitRef", "contextDir", "dockerfile", "buildArgs", "push", "deploy", "executor", "requestId"]
        },
        "profiles": profiles::SPECS,
        "delegatedCapabilities": [
            { "platform": "macos", "profiles": ["flutter-ios-release", "flutter-macos-release"], "runner": "GitHub-hosted macOS or a dedicated macOS worker" },
            { "platform": "windows", "profiles": ["flutter-windows-release"], "runner": "GitHub-hosted Windows or a dedicated Windows worker" }
        ],
        "executors": ["local", "lambda"],
        "pushRegistries": ["amazon-ecr"],
        "deployKinds": ["kustomize", "manifest", "none"],
        "coordination": {
            "provider": "fiducia.cloud",
            "enabled": config.coordination_enabled,
            "required": config.coordination_required
        },
        "persistence": { "postgres": config.database_url.is_some(), "database": "dd_build_server" },
        "messaging": {
            "nats": config.nats_enabled,
            "intake": config.nats_intake_enabled,
            "eventSubject": config.nats_event_subject,
            "requestSubject": config.nats_request_subject
        },
        "webhooks": {
            "github": config.github_webhook_secret.is_some(),
            "registry": config.registry_webhook_secret.is_some(),
            "rules": config.webhook_rules.len()
        },
        "secretSync": { "enabled": config.gh_sync_enabled, "rules": config.gh_sync_rules.len() }
    }))
}

async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    let jobs = state.jobs.read().await;
    let queued = jobs
        .values()
        .filter(|job| matches!(job.status, BuildStatus::Queued))
        .count();
    let mut allowed_namespaces = state
        .config
        .allowed_namespaces
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    allowed_namespaces.sort();
    let mut allowed_repo_prefixes = state.config.allowed_repo_prefixes.clone();
    allowed_repo_prefixes.sort();
    let mut allowed_image_prefixes = state.config.allowed_image_prefixes.clone();
    allowed_image_prefixes.sort();
    let mut allowed_profiles = state
        .config
        .allowed_profiles
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    allowed_profiles.sort();
    let mut allowed_profile_repo_prefixes = state.config.allowed_profile_repo_prefixes.clone();
    allowed_profile_repo_prefixes.sort();

    Json(HealthResponse {
        ok: true,
        service: SERVICE_NAME,
        auth_configured: state.config.server_auth_secret.is_some(),
        deploy_enabled: state.config.deploy_enabled,
        push_enabled: state.config.push_enabled,
        ecr_login_enabled: state.config.ecr_login_enabled,
        allowed_repo_prefixes,
        allowed_image_prefixes,
        allowed_namespaces,
        allowed_profiles,
        allowed_profile_repo_prefixes,
        queued,
        running: state.counters.running.load(Ordering::Relaxed),
    })
}

fn build_dependencies_ready(config: &Config) -> bool {
    config.server_auth_secret.is_some()
        && config.work_root.exists()
        && executable_available(&config.git_bin)
        && executable_available(&config.nerdctl_bin)
        && executable_available(&config.tar_bin)
        && (!config.deploy_enabled || executable_available(&config.kubectl_bin))
}

fn executable_available(value: &str) -> bool {
    let path = Path::new(value);
    if path.is_absolute() || path.components().count() > 1 {
        return path.is_file();
    }
    env::var_os("PATH").is_some_and(|paths| {
        env::split_paths(&paths)
            .map(|directory| directory.join(value))
            .any(|candidate| candidate.is_file())
    })
}

async fn readyz(State(state): State<AppState>) -> Response {
    let ready = build_dependencies_ready(&state.config);
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(json!({
            "ok": ready,
            "service": SERVICE_NAME,
            "dependenciesReady": ready,
        })),
    )
        .into_response()
}

async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    let jobs = state.jobs.read().await;
    let queued = jobs
        .values()
        .filter(|job| matches!(job.status, BuildStatus::Queued))
        .count();
    let mut body = format!(
        "# HELP dd_build_server_jobs_submitted_total Build jobs accepted by the build server.\n\
         # TYPE dd_build_server_jobs_submitted_total counter\n\
         dd_build_server_jobs_submitted_total {}\n\
         # HELP dd_build_server_jobs_running Current running build jobs.\n\
         # TYPE dd_build_server_jobs_running gauge\n\
         dd_build_server_jobs_running {}\n\
         # HELP dd_build_server_jobs_queued Current queued build jobs.\n\
         # TYPE dd_build_server_jobs_queued gauge\n\
         dd_build_server_jobs_queued {}\n\
         # HELP dd_build_server_jobs_succeeded_total Build jobs that completed successfully.\n\
         # TYPE dd_build_server_jobs_succeeded_total counter\n\
         dd_build_server_jobs_succeeded_total {}\n\
         # HELP dd_build_server_jobs_failed_total Build jobs that failed.\n\
         # TYPE dd_build_server_jobs_failed_total counter\n\
         dd_build_server_jobs_failed_total {}\n\
         # HELP dd_build_server_requests_rejected_total Requests rejected before queueing.\n\
         # TYPE dd_build_server_requests_rejected_total counter\n\
         dd_build_server_requests_rejected_total {}\n\
         # HELP dd_build_server_command_failures_total Build pipeline command failures.\n\
         # TYPE dd_build_server_command_failures_total counter\n\
         dd_build_server_command_failures_total {}\n\
         # HELP dd_build_server_ecr_logins_total Successful ECR registry logins.\n\
         # TYPE dd_build_server_ecr_logins_total counter\n\
         dd_build_server_ecr_logins_total {}\n\
         # HELP dd_build_server_ecr_login_failures_total Failed ECR registry logins.\n\
         # TYPE dd_build_server_ecr_login_failures_total counter\n\
         dd_build_server_ecr_login_failures_total {}\n",
        state.counters.submitted.load(Ordering::Relaxed),
        state.counters.running.load(Ordering::Relaxed),
        queued,
        state.counters.succeeded.load(Ordering::Relaxed),
        state.counters.failed.load(Ordering::Relaxed),
        state.counters.rejected.load(Ordering::Relaxed),
        state.counters.command_failures.load(Ordering::Relaxed),
        state.counters.ecr_logins.load(Ordering::Relaxed),
        state.counters.ecr_login_failures.load(Ordering::Relaxed),
    );
    body.push_str(&format!(
        "# HELP dd_build_server_locks_acquired_total fiducia.cloud build locks acquired.\n\
         # TYPE dd_build_server_locks_acquired_total counter\n\
         dd_build_server_locks_acquired_total {}\n\
         # HELP dd_build_server_lock_failures_total fiducia lock contention or unavailability.\n\
         # TYPE dd_build_server_lock_failures_total counter\n\
         dd_build_server_lock_failures_total {}\n\
         # HELP dd_build_server_webhooks_received_total Inbound webhooks accepted (after auth).\n\
         # TYPE dd_build_server_webhooks_received_total counter\n\
         dd_build_server_webhooks_received_total {}\n\
         # HELP dd_build_server_webhooks_rejected_total Inbound webhooks rejected (bad signature/secret).\n\
         # TYPE dd_build_server_webhooks_rejected_total counter\n\
         dd_build_server_webhooks_rejected_total {}\n\
         # HELP dd_build_server_nats_published_total NATS events published.\n\
         # TYPE dd_build_server_nats_published_total counter\n\
         dd_build_server_nats_published_total {}\n\
         # HELP dd_build_server_nats_publish_failures_total NATS publish failures.\n\
         # TYPE dd_build_server_nats_publish_failures_total counter\n\
         dd_build_server_nats_publish_failures_total {}\n\
         # HELP dd_build_server_gh_secrets_synced_total GitHub Actions secrets synced.\n\
         # TYPE dd_build_server_gh_secrets_synced_total counter\n\
         dd_build_server_gh_secrets_synced_total {}\n\
         # HELP dd_build_server_gh_secret_sync_failures_total GitHub Actions secret sync failures.\n\
         # TYPE dd_build_server_gh_secret_sync_failures_total counter\n\
         dd_build_server_gh_secret_sync_failures_total {}\n",
        state.counters.locks_acquired.load(Ordering::Relaxed),
        state.counters.lock_failures.load(Ordering::Relaxed),
        state.counters.webhooks_received.load(Ordering::Relaxed),
        state.counters.webhooks_rejected.load(Ordering::Relaxed),
        state.counters.nats_published.load(Ordering::Relaxed),
        state.counters.nats_publish_failures.load(Ordering::Relaxed),
        state.counters.gh_secrets_synced.load(Ordering::Relaxed),
        state.counters.gh_secret_sync_failures.load(Ordering::Relaxed),
    ));
    body.push_str(&format!(
        "# HELP dd_build_server_dependencies_ready Whether auth, work storage, and required build tools are available.\n\
         # TYPE dd_build_server_dependencies_ready gauge\n\
         dd_build_server_dependencies_ready {}\n",
        u8::from(build_dependencies_ready(&state.config))
    ));
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
}

/// Failure classes for the NATS intake path (drives ack vs term vs nak).
pub enum NatsSubmitError {
    /// Permanently invalid — the message can never succeed (bad JSON/schema).
    Invalid(String),
    /// Transient — queue full or a dependency is down; redeliver later.
    Transient(String),
}

/// Validate + enqueue a build, shared by the HTTP, webhook, and NATS paths.
/// Applies queue backpressure and best-effort in-process requestId dedupe.
async fn enqueue_build(
    state: &AppState,
    request: BuildRequest,
    source: &str,
) -> Result<BuildJobRecord, (StatusCode, String)> {
    if let Err(error) = validate_build_request(&state.config, &request) {
        state.counters.rejected.fetch_add(1, Ordering::Relaxed);
        return Err((StatusCode::BAD_REQUEST, error));
    }

    // In-process dedupe for at-least-once transports. fiducia idempotency
    // leases and the JetStream Nats-Msg-Id are the cross-replica guards; this
    // just collapses a burst of same-process redelivery.
    if let Some(request_id) = clean_optional(request.request_id.as_deref()) {
        let mut seen = state.recent_request_ids.write().await;
        if !seen.insert(request_id.clone()) {
            return Err((
                StatusCode::CONFLICT,
                format!("requestId {request_id} was already accepted"),
            ));
        }
        if seen.len() > 4096 {
            seen.clear();
        }
    }

    // Backpressure: bound the queue so authenticated callers cannot grow
    // memory (and the on-disk job tree) without limit.
    {
        let jobs = state.jobs.read().await;
        let queued = jobs
            .values()
            .filter(|job| matches!(job.status, BuildStatus::Queued))
            .count();
        if queued >= state.config.max_queued {
            state.counters.rejected.fetch_add(1, Ordering::Relaxed);
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                format!(
                    "build queue is full ({queued} queued; limit {})",
                    state.config.max_queued
                ),
            ));
        }
    }

    let executor = request
        .executor
        .clone()
        .unwrap_or_else(|| "local".to_string());
    let counter = state.counters.submitted.fetch_add(1, Ordering::Relaxed) + 1;
    let id = job_id(counter);
    let job_dir = state.config.work_root.join(&id);
    let log_path = job_dir.join("build.log");
    let record = BuildJobRecord {
        id: id.clone(),
        status: BuildStatus::Queued,
        request,
        source: source.to_string(),
        executor,
        created_at_ms: now_ms(),
        started_at_ms: None,
        finished_at_ms: None,
        log_path: log_path.to_string_lossy().to_string(),
        error: None,
        lock_key: None,
        fencing_token: None,
    };

    {
        let mut jobs = state.jobs.write().await;
        jobs.insert(id.clone(), record.clone());
    }
    if let Some(db) = state.db.as_ref() {
        db::persist_job(db, &record).await;
    }
    prune_jobs(state).await;

    let task_state = state.clone();
    let task_id = id.clone();
    tokio::spawn(async move {
        run_job(task_state, task_id).await;
    });

    Ok(record)
}

/// NATS intake: parse a build-server.v1 document and enqueue it.
async fn submit_from_nats(state: &AppState, payload: &[u8]) -> Result<(), NatsSubmitError> {
    let request: BuildRequest = serde_json::from_slice(payload).map_err(|error| {
        NatsSubmitError::Invalid(format!("invalid build request JSON: {error}"))
    })?;
    match enqueue_build(state, request, "nats").await {
        Ok(_) => Ok(()),
        Err((StatusCode::CONFLICT, message)) => {
            tracing::info!("nats build request deduped: {message}");
            Ok(())
        }
        Err((StatusCode::SERVICE_UNAVAILABLE, message)) => Err(NatsSubmitError::Transient(message)),
        Err((_, message)) => Err(NatsSubmitError::Invalid(message)),
    }
}

async fn submit_build(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<BuildRequest>,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    match enqueue_build(&state, request, "http").await {
        Ok(record) => (StatusCode::ACCEPTED, Json(record)).into_response(),
        Err((status, message)) => (status, Json(json!({ "error": message }))).into_response(),
    }
}

async fn sync_secrets(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    if !state.config.gh_sync_enabled {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "gh secret sync is disabled by BUILD_SERVER_GH_SYNC_ENABLED=false" })),
        )
            .into_response();
    }
    let outcomes = gh_secrets::sync_all(&state).await;
    (StatusCode::OK, Json(json!({ "outcomes": outcomes }))).into_response()
}

async fn sync_secrets_status(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let runs = match state.db.as_ref() {
        Some(db) => db::recent_secret_sync_runs(db, 100).await,
        None => Vec::new(),
    };
    (StatusCode::OK, Json(json!({ "runs": runs }))).into_response()
}

async fn list_builds(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let mut jobs = state
        .jobs
        .read()
        .await
        .values()
        .cloned()
        .collect::<Vec<_>>();
    jobs.sort_by_key(|job| std::cmp::Reverse(job.created_at_ms));
    // With persistence on, also surface recent jobs from prior processes
    // (the in-memory map only holds this process's jobs).
    if let Some(db) = state.db.as_ref() {
        let known: HashSet<String> = jobs.iter().map(|job| job.id.clone()).collect();
        let persisted = db::recent_jobs(db, 200).await;
        let mut merged = persisted
            .into_iter()
            .filter(|row| !known.contains(&row.id))
            .map(|row| {
                json!({
                    "id": row.id,
                    "status": row.status,
                    "jobKind": row.job_kind,
                    "source": row.source,
                    "executor": row.executor,
                    "repoUrl": row.repo_url,
                    "gitRef": row.git_ref,
                    "image": row.image,
                    "error": row.error,
                    "persisted": true,
                })
            })
            .collect::<Vec<_>>();
        let mut live = jobs
            .iter()
            .map(|job| serde_json::to_value(job).unwrap_or(serde_json::Value::Null))
            .collect::<Vec<_>>();
        live.append(&mut merged);
        return Json(live).into_response();
    }
    Json(jobs).into_response()
}

async fn get_build(
    State(state): State<AppState>,
    AxumPath(job_id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let jobs = state.jobs.read().await;
    match jobs.get(&job_id) {
        Some(job) => Json(job).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "build job not found" })),
        )
            .into_response(),
    }
}

async fn get_build_logs(
    State(state): State<AppState>,
    AxumPath(job_id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let log_path = {
        let jobs = state.jobs.read().await;
        match jobs.get(&job_id) {
            Some(job) => PathBuf::from(&job.log_path),
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({ "error": "build job not found" })),
                )
                    .into_response();
            }
        }
    };

    match fs::read_to_string(&log_path).await {
        Ok(body) => ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], body).into_response(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "build log not found" })),
        )
            .into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("failed to read build log: {error}") })),
        )
            .into_response(),
    }
}

async fn get_build_artifacts(
    State(state): State<AppState>,
    AxumPath(job_id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    {
        let jobs = state.jobs.read().await;
        if !jobs.contains_key(&job_id) {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "build job not found" })),
            )
                .into_response();
        }
    }

    let artifact_path = state
        .config
        .work_root
        .join(&job_id)
        .join("artifacts.tar.gz");
    match fs::File::open(&artifact_path).await {
        Ok(file) => {
            let stream = ReaderStream::new(file);
            let disposition = format!("attachment; filename=\"{job_id}-artifacts.tar.gz\"");
            (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, "application/gzip".to_string()),
                    (header::CONTENT_DISPOSITION, disposition),
                ],
                Body::from_stream(stream),
            )
                .into_response()
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "build artifacts not found" })),
        )
            .into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("failed to open build artifacts: {error}") })),
        )
            .into_response(),
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
async fn main() {
    let _otel = dd_telemetry::init("dd-build-server");

    let config = Arc::new(config_from_env());
    let host = env_value("HOST", "0.0.0.0");
    let port = env_u64("PORT", DEFAULT_PORT as u64) as u16;
    let max_concurrent = env_usize("BUILD_SERVER_MAX_CONCURRENT_BUILDS", 1);

    if let Err(error) = fs::create_dir_all(&config.work_root).await {
        panic!("failed to create build server work root: {error}");
    }

    // Optional Postgres persistence (own database dd_build_server on RDS). A
    // connection failure is fatal only when a URL was configured — it signals
    // misconfiguration; with no URL the server runs in-memory as before.
    let db = match config.database_url.as_deref() {
        Some(url) => match db::connect(url).await {
            Ok(connection) => {
                db::fail_interrupted_jobs(&connection).await;
                Some(connection)
            }
            Err(error) => panic!("BUILD_SERVER_DATABASE_URL was set but connect failed: {error}"),
        },
        None => {
            tracing::info!(
                "no BUILD_SERVER_DATABASE_URL configured; running with in-memory jobs only"
            );
            None
        }
    };

    // Optional NATS (on by default; failure is non-fatal — the server still
    // serves HTTP, it just won't publish/consume events).
    let nats = if config.nats_enabled {
        match events::connect(&config.nats_url).await {
            Ok(client) => Some(client),
            Err(error) => {
                tracing::warn!("NATS disabled: {error}");
                None
            }
        }
    } else {
        None
    };

    let holder = format!("dd-build-server/{}", uuid::Uuid::new_v4());

    let state = AppState {
        config: config.clone(),
        http: reqwest::Client::new(),
        jobs: Arc::new(RwLock::new(HashMap::new())),
        semaphore: Arc::new(Semaphore::new(max_concurrent)),
        counters: Arc::new(Counters::default()),
        db,
        nats,
        holder,
        recent_request_ids: Arc::new(RwLock::new(HashSet::new())),
    };

    // Durable JetStream build-request intake (opt-in).
    if config.nats_intake_enabled && state.nats.is_some() {
        tokio::spawn(events::run_request_intake(state.clone()));
    }
    // Periodic GitHub Actions secret sync (opt-in; 0 interval = manual only).
    if config.gh_sync_enabled && !config.gh_sync_interval.is_zero() {
        tokio::spawn(gh_secrets::run_periodic_sync(state.clone()));
    }

    let app = Router::new()
        .route("/", get(descriptor))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .route("/metrics", get(metrics))
        .route("/builds", get(list_builds).post(submit_build))
        .route("/builds/:job_id", get(get_build))
        .route("/builds/:job_id/logs", get(get_build_logs))
        .route("/builds/:job_id/artifacts", get(get_build_artifacts))
        .route("/webhooks/github", post(webhooks::github_webhook))
        .route("/webhooks/registry", post(webhooks::registry_webhook))
        .route("/secrets/sync", post(sync_secrets))
        .route("/secrets/sync/status", get(sync_secrets_status))
        .with_state(state)
        .merge(dd_runtime_config_client::router());

    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    let address: SocketAddr = format!("{host}:{port}")
        .parse()
        .expect("failed to parse bind address");
    tracing::info!("{SERVICE_NAME} listening on http://{address}");

    let listener = tokio::net::TcpListener::bind(address)
        .await
        .expect("failed to bind tcp listener");
    axum::serve(listener, app.layer(dd_telemetry::http_trace_layer()))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("axum server crashed");
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_and_path_validation_blocks_command_and_path_injection() {
        assert!(validate_repo_url("https://github.com/ORESoftware/example.git").is_ok());
        assert!(validate_repo_url("file:///etc/passwd").is_err());
        assert!(validate_repo_url("https://github.com/example.git\n--upload-pack=evil").is_err());
        assert!(validate_relative_path("contextDir", "services/api").is_ok());
        assert!(validate_relative_path("contextDir", "../../etc").is_err());
        assert!(validate_relative_path("contextDir", "/etc").is_err());
    }

    #[test]
    fn build_args_reject_secret_like_keys() {
        let safe = Some(BTreeMap::from([(
            "BUILD_PROFILE".to_string(),
            "release".to_string(),
        )]));
        let unsafe_args = Some(BTreeMap::from([(
            "GITHUB_TOKEN".to_string(),
            "do-not-pass-secrets-as-build-args".to_string(),
        )]));
        assert!(validate_build_args(&safe).is_ok());
        assert!(validate_build_args(&unsafe_args).is_err());
    }

    #[test]
    fn executable_lookup_accepts_path_commands_and_rejects_missing_tools() {
        assert!(executable_available("sh"));
        assert!(!executable_available(
            "dd-build-server-tool-that-does-not-exist"
        ));
    }

    #[test]
    fn fixed_profiles_exist_and_do_not_accept_commands_from_callers() {
        let names = profiles::names().collect::<HashSet<_>>();
        for expected in [
            "flutter-android-debug",
            "flutter-web-release",
            "flutter-linux-release",
            "flutter-web-e2e",
            "playwright",
            "puppeteer",
        ] {
            assert!(names.contains(expected));
        }
        assert!(profiles::find("sh -c evil").is_none());
    }
}
