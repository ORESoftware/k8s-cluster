use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    net::IpAddr,
    path::{Component, Path as FsPath},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    body::Bytes,
    extract::{Path as AxumPath, State},
    http::{header::CONTENT_TYPE, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use reqwest::{redirect::Policy as RedirectPolicy, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::{
    net::TcpListener,
    sync::{Mutex, RwLock},
    time::Duration,
};
use tracing::info;

const SERVICE_NAME: &str = "gha-executor-router";
const SCHEMA_VERSION: &str = "gha.executor-router/v1";
const DEFAULT_MAX_ROUTES: usize = 4_096;
const DEFAULT_MAX_REQUEST_BYTES: usize = 65_536;
const DEFAULT_MAX_RESPONSE_BYTES: usize = 65_536;
const DEFAULT_REQUEST_TIMEOUT_SECONDS: u64 = 60;

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    client: reqwest::Client,
    routes: Arc<RwLock<RouteStore>>,
    submission_lock: Arc<Mutex<()>>,
    metrics: Arc<Metrics>,
}

#[derive(Clone, Debug)]
struct Config {
    host: String,
    port: u16,
    execution_enabled: bool,
    inbound_auth: Option<String>,
    executors: Vec<Executor>,
    max_routes: usize,
    max_request_bytes: usize,
    max_response_bytes: usize,
    request_timeout_seconds: u64,
}

#[derive(Clone, Debug)]
struct Executor {
    id: String,
    provider: String,
    base_url: String,
    auth: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExecutorInput {
    id: String,
    provider: String,
    base_url: String,
    auth_path: String,
}

impl Config {
    fn from_env() -> Result<Self, String> {
        let execution_enabled = env_bool("GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED", false)?;
        let inbound_auth = env_optional("GHA_EXECUTOR_ROUTER_AUTH_PATH")
            .map(|path| read_secret_file("GHA_EXECUTOR_ROUTER_AUTH_PATH", &path))
            .transpose()?;
        let executor_inputs = env_optional("GHA_EXECUTOR_ROUTER_EXECUTORS_JSON")
            .map(|value| {
                serde_json::from_str::<Vec<ExecutorInput>>(&value).map_err(|error| {
                    format!("GHA_EXECUTOR_ROUTER_EXECUTORS_JSON is invalid: {error}")
                })
            })
            .transpose()?
            .unwrap_or_default();
        let executors = validate_and_load_executors(executor_inputs)?;

        Ok(Self {
            host: env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string()),
            port: env_u16("PORT", 8126)?,
            execution_enabled,
            inbound_auth,
            executors,
            max_routes: env_nonzero_usize(
                "GHA_EXECUTOR_ROUTER_MAX_ROUTES",
                DEFAULT_MAX_ROUTES,
            )?,
            max_request_bytes: env_nonzero_usize(
                "GHA_EXECUTOR_ROUTER_MAX_REQUEST_BYTES",
                DEFAULT_MAX_REQUEST_BYTES,
            )?,
            max_response_bytes: env_nonzero_usize(
                "GHA_EXECUTOR_ROUTER_MAX_RESPONSE_BYTES",
                DEFAULT_MAX_RESPONSE_BYTES,
            )?,
            request_timeout_seconds: env_nonzero_u64(
                "GHA_EXECUTOR_ROUTER_REQUEST_TIMEOUT_SECONDS",
                DEFAULT_REQUEST_TIMEOUT_SECONDS,
            )?,
        })
    }

    fn execution_ready(&self) -> bool {
        !self.execution_enabled || (self.inbound_auth.is_some() && !self.executors.is_empty())
    }

    fn public_executors(&self) -> Vec<Value> {
        self.executors
            .iter()
            .enumerate()
            .map(|(priority, executor)| {
                json!({
                    "id": executor.id,
                    "provider": executor.provider,
                    "priority": priority
                })
            })
            .collect()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BuildServerRequest {
    schema_version: String,
    job_kind: String,
    repo_url: String,
    git_ref: String,
    profile: String,
    request_id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpstreamBuildResponse {
    id: String,
    status: String,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PublicBuildResponse {
    id: String,
    status: String,
    error: Option<String>,
}

#[derive(Clone, Debug)]
struct RouteRecord {
    route_id: String,
    request_id: String,
    request_digest: String,
    executor_id: String,
    provider: String,
    upstream_id: Option<String>,
    status: String,
    created_at_ms: u128,
}

impl RouteRecord {
    fn public_response(&self) -> PublicBuildResponse {
        PublicBuildResponse {
            id: self.route_id.clone(),
            status: self.status.clone(),
            error: public_error_for_status(&self.status),
        }
    }
}

#[derive(Default)]
struct RouteStore {
    by_request: BTreeMap<String, RouteRecord>,
    request_by_route: BTreeMap<String, String>,
}

#[derive(Default)]
struct Metrics {
    submission_attempts: AtomicU64,
    failovers: AtomicU64,
    duplicate_requests: AtomicU64,
    contract_rejections: AtomicU64,
    ambiguous_acceptances: AtomicU64,
    pinned_poll_failures: AtomicU64,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "gha_executor_router=info,tower_http=info".into()),
        )
        .init();

    let config = Config::from_env().unwrap_or_else(|error| {
        eprintln!("{SERVICE_NAME}: configuration error: {error}");
        std::process::exit(2);
    });
    let address = format!("{}:{}", config.host, config.port);
    let request_timeout_seconds = config.request_timeout_seconds;
    let state = AppState {
        config: Arc::new(config),
        client: reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(request_timeout_seconds))
            .redirect(RedirectPolicy::none())
            .user_agent("gha-executor-router/0.1")
            .build()
            .expect("reqwest client"),
        routes: Arc::new(RwLock::new(RouteStore::default())),
        submission_lock: Arc::new(Mutex::new(())),
        metrics: Arc::new(Metrics::default()),
    };

    let listener = TcpListener::bind(&address)
        .await
        .unwrap_or_else(|error| panic!("failed to bind {address}: {error}"));
    info!(%address, "listening");
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .expect("server");
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(descriptor))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/v1/capabilities", get(capabilities))
        .route("/metrics", get(metrics))
        .route("/builds", post(submit_build))
        .route("/builds/:route_id", get(get_build))
        .with_state(state)
}

async fn descriptor(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "service": SERVICE_NAME,
        "schemaVersion": SCHEMA_VERSION,
        "executionEnabled": state.config.execution_enabled,
        "executionReady": state.config.execution_ready(),
        "executors": state.config.public_executors(),
        "failoverPolicy": "before-acceptance-only",
        "pollingPolicy": "pinned-to-accepting-executor"
    }))
}

async fn healthz(State(state): State<AppState>) -> Json<Value> {
    let retained = state.routes.read().await.by_request.len();
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "executionEnabled": state.config.execution_enabled,
        "executionReady": state.config.execution_ready(),
        "executors": state.config.public_executors(),
        "routesRetained": retained,
        "maxRoutes": state.config.max_routes
    }))
}

async fn readyz(State(state): State<AppState>) -> Response {
    let ready = state.config.execution_ready();
    (
        if ready {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(json!({
            "ok": ready,
            "service": SERVICE_NAME,
            "executionEnabled": state.config.execution_enabled,
            "executionReady": ready,
            "executorsConfigured": state.config.executors.len()
        })),
    )
        .into_response()
}

async fn capabilities(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "schemaVersion": SCHEMA_VERSION,
        "api": {
            "submit": "POST /builds",
            "status": "GET /builds/{id}",
            "authenticationHeader": "x-build-server-auth"
        },
        "executors": state.config.public_executors(),
        "guarantees": {
            "requestIdForwardedUnchanged": true,
            "fallbackOnlyBeforeAcceptance": true,
            "pollingPinnedAfterAcceptance": true,
            "callerCannotSelectExecutor": true,
            "callerCannotSelectCommandOrImage": true,
            "routeStorage": "bounded-in-memory"
        }
    }))
}

async fn metrics(State(state): State<AppState>) -> Response {
    let routes = state.routes.read().await.by_request.len();
    let mut body = String::new();
    body.push_str("# HELP gha_executor_router_routes Retained deterministic request routes.\n");
    body.push_str("# TYPE gha_executor_router_routes gauge\n");
    body.push_str(&format!("gha_executor_router_routes {routes}\n"));
    body.push_str("# HELP gha_executor_router_execution_enabled Whether independent execution is enabled.\n");
    body.push_str("# TYPE gha_executor_router_execution_enabled gauge\n");
    body.push_str(&format!(
        "gha_executor_router_execution_enabled {}\n",
        u8::from(state.config.execution_enabled)
    ));
    for executor in &state.config.executors {
        body.push_str(&format!(
            "gha_executor_router_executor_configured{{executor=\"{}\",provider=\"{}\"}} 1\n",
            executor.id, executor.provider
        ));
    }
    append_counter(
        &mut body,
        "gha_executor_router_submission_attempts_total",
        state.metrics.submission_attempts.load(Ordering::Relaxed),
    );
    append_counter(
        &mut body,
        "gha_executor_router_failovers_total",
        state.metrics.failovers.load(Ordering::Relaxed),
    );
    append_counter(
        &mut body,
        "gha_executor_router_duplicate_requests_total",
        state.metrics.duplicate_requests.load(Ordering::Relaxed),
    );
    append_counter(
        &mut body,
        "gha_executor_router_contract_rejections_total",
        state.metrics.contract_rejections.load(Ordering::Relaxed),
    );
    append_counter(
        &mut body,
        "gha_executor_router_ambiguous_acceptances_total",
        state.metrics.ambiguous_acceptances.load(Ordering::Relaxed),
    );
    append_counter(
        &mut body,
        "gha_executor_router_pinned_poll_failures_total",
        state.metrics.pinned_poll_failures.load(Ordering::Relaxed),
    );
    (
        StatusCode::OK,
        [(CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")],
        body,
    )
        .into_response()
}

async fn submit_build(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    if !state.config.execution_enabled {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "independent executor routing is disabled",
        );
    }
    if !state.config.execution_ready() {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "independent executor routing is not ready",
        );
    }
    if body.len() > state.config.max_request_bytes {
        return json_error(StatusCode::PAYLOAD_TOO_LARGE, "build request exceeds limit");
    }
    let request: BuildServerRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return json_error(StatusCode::BAD_REQUEST, "invalid build request JSON"),
    };
    if let Err(error) = validate_build_request(&request) {
        return json_error(StatusCode::UNPROCESSABLE_ENTITY, &error);
    }
    let request_digest = request_digest(&request);

    // Submission is intentionally serialized. The upstream operation is short,
    // and this gives one process a strict check/submit/record critical section
    // for deterministic request IDs without exposing repository commands.
    let _submission_guard = state.submission_lock.lock().await;
    if let Some(existing) = state
        .routes
        .read()
        .await
        .by_request
        .get(&request.request_id)
        .cloned()
    {
        state
            .metrics
            .duplicate_requests
            .fetch_add(1, Ordering::Relaxed);
        if existing.request_digest != request_digest {
            return json_error(
                StatusCode::CONFLICT,
                "requestId is already bound to different immutable build inputs",
            );
        }
        return duplicate_response(existing);
    }
    if state.routes.read().await.by_request.len() >= state.config.max_routes {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "route retention is full; operator reconciliation is required",
        );
    }

    for (index, executor) in state.config.executors.iter().enumerate() {
        state
            .metrics
            .submission_attempts
            .fetch_add(1, Ordering::Relaxed);
        if index > 0 {
            state.metrics.failovers.fetch_add(1, Ordering::Relaxed);
        }
        let response = match state
            .client
            .post(format!("{}/builds", executor.base_url))
            .header("x-build-server-auth", &executor.auth)
            .json(&request)
            .send()
            .await
        {
            Ok(response) => response,
            Err(_) => continue,
        };
        let status = response.status();
        if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
            continue;
        }
        if status.is_client_error() {
            state
                .metrics
                .contract_rejections
                .fetch_add(1, Ordering::Relaxed);
            return json_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                &format!(
                    "executor {} rejected the fixed build contract before acceptance",
                    executor.id
                ),
            );
        }
        if status != StatusCode::ACCEPTED {
            state
                .metrics
                .contract_rejections
                .fetch_add(1, Ordering::Relaxed);
            return json_error(
                StatusCode::BAD_GATEWAY,
                &format!(
                    "executor {} returned an unsupported pre-acceptance status",
                    executor.id
                ),
            );
        }

        let bytes = match response.bytes().await {
            Ok(bytes) if bytes.len() <= state.config.max_response_bytes => bytes,
            _ => {
                return remember_ambiguous_acceptance(
                    &state,
                    &request,
                    &request_digest,
                    executor,
                    "executor accepted the request but its response could not be verified",
                )
                .await
            }
        };
        let upstream: UpstreamBuildResponse = match serde_json::from_slice(&bytes) {
            Ok(upstream) if validate_upstream_response(&upstream).is_ok() => upstream,
            _ => {
                return remember_ambiguous_acceptance(
                    &state,
                    &request,
                    &request_digest,
                    executor,
                    "executor accepted the request but returned invalid job evidence",
                )
                .await
            }
        };
        let record = RouteRecord {
            route_id: route_id(executor, &request_digest),
            request_id: request.request_id.clone(),
            request_digest: request_digest.clone(),
            executor_id: executor.id.clone(),
            provider: executor.provider.clone(),
            upstream_id: Some(upstream.id),
            status: upstream.status,
            created_at_ms: now_ms(),
        };
        if let Err(error) = insert_route(&state, record.clone()).await {
            return json_error(StatusCode::SERVICE_UNAVAILABLE, &error);
        }
        return (StatusCode::ACCEPTED, Json(record.public_response())).into_response();
    }

    json_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "no configured executor accepted the build before the failover boundary",
    )
}

async fn get_build(
    State(state): State<AppState>,
    AxumPath(route_id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let record = {
        let routes = state.routes.read().await;
        let Some(request_id) = routes.request_by_route.get(&route_id) else {
            return json_error(StatusCode::NOT_FOUND, "build route not found");
        };
        routes.by_request.get(request_id).cloned()
    };
    let Some(record) = record else {
        return json_error(StatusCode::NOT_FOUND, "build route not found");
    };
    let Some(upstream_id) = record.upstream_id.as_deref() else {
        return json_error(
            StatusCode::BAD_GATEWAY,
            "executor acceptance is ambiguous; polling cannot be redirected or retried",
        );
    };
    let Some(executor) = state
        .config
        .executors
        .iter()
        .find(|executor| executor.id == record.executor_id)
    else {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "the accepting executor is no longer configured",
        );
    };

    let response = match state
        .client
        .get(format!("{}/builds/{upstream_id}", executor.base_url))
        .header("x-build-server-auth", &executor.auth)
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => {
            state
                .metrics
                .pinned_poll_failures
                .fetch_add(1, Ordering::Relaxed);
            return json_error(
                StatusCode::BAD_GATEWAY,
                &format!(
                    "polling failed for the build pinned to executor {}; it was not resubmitted",
                    executor.id
                ),
            );
        }
    };
    if response.status() != StatusCode::OK {
        state
            .metrics
            .pinned_poll_failures
            .fetch_add(1, Ordering::Relaxed);
        return json_error(
            StatusCode::BAD_GATEWAY,
            &format!(
                "executor {} could not report the pinned build; it was not resubmitted",
                executor.id
            ),
        );
    }
    let bytes = match response.bytes().await {
        Ok(bytes) if bytes.len() <= state.config.max_response_bytes => bytes,
        _ => {
            state
                .metrics
                .pinned_poll_failures
                .fetch_add(1, Ordering::Relaxed);
            return json_error(
                StatusCode::BAD_GATEWAY,
                "pinned build status evidence exceeded the response limit",
            );
        }
    };
    let upstream: UpstreamBuildResponse = match serde_json::from_slice(&bytes) {
        Ok(upstream) => upstream,
        Err(_) => {
            state
                .metrics
                .pinned_poll_failures
                .fetch_add(1, Ordering::Relaxed);
            return json_error(
                StatusCode::BAD_GATEWAY,
                "pinned executor returned invalid build status evidence",
            );
        }
    };
    if upstream.id != upstream_id || validate_upstream_response(&upstream).is_err() {
        state
            .metrics
            .pinned_poll_failures
            .fetch_add(1, Ordering::Relaxed);
        return json_error(
            StatusCode::BAD_GATEWAY,
            "pinned executor returned mismatched build status evidence",
        );
    }

    let response = PublicBuildResponse {
        id: record.route_id.clone(),
        status: upstream.status.clone(),
        error: public_error_for_status(&upstream.status),
    };
    if let Some(stored) = state
        .routes
        .write()
        .await
        .by_request
        .get_mut(&record.request_id)
    {
        stored.status = upstream.status;
    }
    (StatusCode::OK, Json(response)).into_response()
}

async fn remember_ambiguous_acceptance(
    state: &AppState,
    request: &BuildServerRequest,
    request_digest: &str,
    executor: &Executor,
    message: &str,
) -> Response {
    state
        .metrics
        .ambiguous_acceptances
        .fetch_add(1, Ordering::Relaxed);
    let record = RouteRecord {
        route_id: route_id(executor, request_digest),
        request_id: request.request_id.clone(),
        request_digest: request_digest.to_string(),
        executor_id: executor.id.clone(),
        provider: executor.provider.clone(),
        upstream_id: None,
        status: "unknown".to_string(),
        created_at_ms: now_ms(),
    };
    if let Err(error) = insert_route(state, record).await {
        return json_error(StatusCode::SERVICE_UNAVAILABLE, &error);
    }
    json_error(StatusCode::BAD_GATEWAY, message)
}

async fn insert_route(state: &AppState, record: RouteRecord) -> Result<(), String> {
    let mut routes = state.routes.write().await;
    if routes.by_request.len() >= state.config.max_routes {
        return Err("route retention is full; operator reconciliation is required".to_string());
    }
    if routes.by_request.contains_key(&record.request_id)
        || routes.request_by_route.contains_key(&record.route_id)
    {
        return Err("deterministic route identity collision was rejected".to_string());
    }
    routes
        .request_by_route
        .insert(record.route_id.clone(), record.request_id.clone());
    routes.by_request.insert(record.request_id.clone(), record);
    Ok(())
}

fn duplicate_response(record: RouteRecord) -> Response {
    if record.upstream_id.is_none() {
        json_error(
            StatusCode::BAD_GATEWAY,
            "the original request reached an ambiguous acceptance boundary and was not resubmitted",
        )
    } else {
        (StatusCode::ACCEPTED, Json(record.public_response())).into_response()
    }
}

fn validate_and_load_executors(inputs: Vec<ExecutorInput>) -> Result<Vec<Executor>, String> {
    if inputs.len() > 8 {
        return Err("no more than eight executors may be configured".to_string());
    }
    let mut ids = BTreeSet::new();
    let mut providers = BTreeSet::new();
    let mut urls = BTreeSet::new();
    let mut auth_paths = BTreeSet::new();
    let mut executors = Vec::with_capacity(inputs.len());
    for input in inputs {
        if !valid_slug(&input.id, 32) {
            return Err(format!("executor id {:?} is invalid", input.id));
        }
        if !matches!(input.provider.as_str(), "aws" | "hetzner") {
            return Err(format!(
                "executor provider {:?} must be aws or hetzner",
                input.provider
            ));
        }
        if !ids.insert(input.id.clone()) {
            return Err(format!("duplicate executor id {:?}", input.id));
        }
        if !providers.insert(input.provider.clone()) {
            return Err(format!("duplicate executor provider {:?}", input.provider));
        }
        let base_url = normalize_executor_url(&input.base_url)?;
        if !urls.insert(base_url.clone()) {
            return Err(format!("duplicate executor base URL for {:?}", input.id));
        }
        validate_secret_path(&input.auth_path)?;
        if !auth_paths.insert(input.auth_path.clone()) {
            return Err(format!("duplicate executor auth path for {:?}", input.id));
        }
        let auth = read_secret_file("executor authPath", &input.auth_path)?;
        executors.push(Executor {
            id: input.id,
            provider: input.provider,
            base_url,
            auth,
        });
    }
    Ok(executors)
}

fn normalize_executor_url(value: &str) -> Result<String, String> {
    let value = value.trim().trim_end_matches('/');
    if value.is_empty() || value.len() > 512 {
        return Err("executor baseUrl must be nonempty and bounded".to_string());
    }
    let parsed = Url::parse(value).map_err(|error| format!("executor baseUrl is invalid: {error}"))?;
    if parsed.cannot_be_a_base()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.path() != "/"
    {
        return Err(
            "executor baseUrl must not contain credentials, path, query, or fragment".to_string(),
        );
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| "executor baseUrl is missing a host".to_string())?;
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    let cluster_service = host.ends_with(".svc") || host.ends_with(".svc.cluster.local");
    if parsed.scheme() != "https" && !(parsed.scheme() == "http" && (loopback || cluster_service)) {
        return Err(
            "executor baseUrl must use HTTPS; HTTP is allowed only for loopback tests or in-cluster Service DNS"
                .to_string(),
        );
    }
    Ok(value.to_string())
}

fn validate_secret_path(value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > 512 {
        return Err("secret file path must be nonempty and bounded".to_string());
    }
    let path = FsPath::new(value);
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err("secret file path must be absolute and contain no traversal".to_string());
    }
    Ok(())
}

fn read_secret_file(field: &str, path: &str) -> Result<String, String> {
    validate_secret_path(path)?;
    let value = fs::read_to_string(path)
        .map_err(|_| format!("{field} could not be read from its configured file"))?;
    let value = value.trim();
    if value.is_empty() || value.len() > 4_096 || value.contains(['\n', '\r']) {
        return Err(format!("{field} file must contain one bounded nonempty value"));
    }
    Ok(value.to_string())
}

fn validate_build_request(request: &BuildServerRequest) -> Result<(), String> {
    if request.schema_version != "build-server.v1" {
        return Err("schemaVersion must be build-server.v1".to_string());
    }
    if request.job_kind != "run-profile" {
        return Err("jobKind must be run-profile".to_string());
    }
    validate_repository_url(&request.repo_url)?;
    if request.git_ref.len() != 40
        || request.git_ref != request.git_ref.to_ascii_lowercase()
        || !request.git_ref.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("gitRef must be an immutable lowercase 40-hex commit".to_string());
    }
    if !valid_slug(&request.profile, 64) {
        return Err("profile must be a bounded fixed-profile slug".to_string());
    }
    if request.request_id.is_empty()
        || request.request_id.len() > 256
        || !request.request_id.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'.' | b'_' | b'-')
        })
    {
        return Err("requestId contains unsupported characters or exceeds its limit".to_string());
    }
    Ok(())
}

fn validate_repository_url(value: &str) -> Result<(), String> {
    if value.len() > 512 {
        return Err("repoUrl exceeds its limit".to_string());
    }
    let parsed = Url::parse(value).map_err(|_| "repoUrl is invalid".to_string())?;
    if parsed.scheme() != "https"
        || parsed.host_str() != Some("github.com")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err("repoUrl must be a credential-free HTTPS GitHub origin".to_string());
    }
    let parts = parsed
        .path()
        .trim_matches('/')
        .split('/')
        .collect::<Vec<_>>();
    if parts.len() != 2
        || parts[0].is_empty()
        || !parts[1].ends_with(".git")
        || parts[1].trim_end_matches(".git").is_empty()
        || !parts.iter().all(|part| {
            part.bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
    {
        return Err("repoUrl must identify exactly one GitHub owner/repository.git".to_string());
    }
    Ok(())
}

fn validate_upstream_response(response: &UpstreamBuildResponse) -> Result<(), String> {
    if response.id.is_empty()
        || response.id.len() > 128
        || !response
            .id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err("upstream build id is invalid".to_string());
    }
    if !matches!(
        response.status.as_str(),
        "queued" | "running" | "succeeded" | "failed"
    ) {
        return Err("upstream build status is invalid".to_string());
    }
    let _ = &response.error;
    Ok(())
}

fn valid_slug(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn request_digest(request: &BuildServerRequest) -> String {
    let bytes = serde_json::to_vec(request).expect("serializable build request");
    hex::encode(Sha256::digest(bytes))
}

fn route_id(executor: &Executor, digest: &str) -> String {
    format!("{}~{}", executor.id, &digest[..24])
}

fn public_error_for_status(status: &str) -> Option<String> {
    match status {
        "failed" => Some("executor reported a failed fixed-profile build".to_string()),
        "unknown" => Some("executor acceptance requires operator reconciliation".to_string()),
        _ => None,
    }
}

fn require_auth(headers: &HeaderMap, state: &AppState) -> Result<(), Response> {
    let Some(expected) = state.config.inbound_auth.as_deref() else {
        return Err(json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "router authentication is not configured",
        ));
    };
    let presented = headers
        .get("x-build-server-auth")
        .and_then(|value| value.to_str().ok());
    if presented.is_some_and(|value| digest_eq(value, expected)) {
        Ok(())
    } else {
        Err(json_error(StatusCode::UNAUTHORIZED, "unauthorized"))
    }
}

fn digest_eq(left: &str, right: &str) -> bool {
    let left = Sha256::digest(left.as_bytes());
    let right = Sha256::digest(right.as_bytes());
    left.as_slice().ct_eq(right.as_slice()).into()
}

fn json_error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

fn append_counter(body: &mut String, name: &str, value: u64) {
    body.push_str(&format!("# TYPE {name} counter\n{name} {value}\n"));
}

fn env_optional(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_bool(name: &str, default: bool) -> Result<bool, String> {
    match env_optional(name).as_deref() {
        None => Ok(default),
        Some("true") => Ok(true),
        Some("false") => Ok(false),
        Some(_) => Err(format!("{name} must be true or false")),
    }
}

fn env_u16(name: &str, default: u16) -> Result<u16, String> {
    env_optional(name)
        .map(|value| {
            value
                .parse::<u16>()
                .map_err(|_| format!("{name} must be a valid port"))
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn env_nonzero_usize(name: &str, default: usize) -> Result<usize, String> {
    let value = env_optional(name)
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| format!("{name} must be a positive integer"))
        })
        .transpose()?
        .unwrap_or(default);
    if value == 0 {
        Err(format!("{name} must be greater than zero"))
    } else {
        Ok(value)
    }
}

fn env_nonzero_u64(name: &str, default: u64) -> Result<u64, String> {
    let value = env_optional(name)
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| format!("{name} must be a positive integer"))
        })
        .transpose()?
        .unwrap_or(default);
    if value == 0 {
        Err(format!("{name} must be greater than zero"))
    } else {
        Ok(value)
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> BuildServerRequest {
        BuildServerRequest {
            schema_version: "build-server.v1".to_string(),
            job_kind: "run-profile".to_string(),
            repo_url: "https://github.com/owner/repo.git".to_string(),
            git_ref: "0123456789abcdef0123456789abcdef01234567".to_string(),
            profile: "rust-verify".to_string(),
            request_id: "gha-clone:plan:rust".to_string(),
        }
    }

    #[test]
    fn executor_urls_are_https_cluster_service_or_loopback_only() {
        assert_eq!(
            normalize_executor_url("https://executor.example.com/").unwrap(),
            "https://executor.example.com"
        );
        assert!(normalize_executor_url("http://127.0.0.1:8100").is_ok());
        assert!(normalize_executor_url("http://build.default.svc.cluster.local:8100").is_ok());
        assert!(normalize_executor_url("http://executor.example.com").is_err());
        assert!(normalize_executor_url("https://user:secret@example.com").is_err());
        assert!(normalize_executor_url("https://example.com/path").is_err());
        assert!(normalize_executor_url("https://example.com?token=x").is_err());
    }

    #[test]
    fn secret_paths_are_absolute_and_traversal_free() {
        assert!(validate_secret_path("/var/run/secrets/gha/aws-auth").is_ok());
        assert!(validate_secret_path("relative/auth").is_err());
        assert!(validate_secret_path("/var/run/../secret").is_err());
    }

    #[test]
    fn fixed_build_contract_is_immutable_and_bounded() {
        assert!(validate_build_request(&request()).is_ok());
        let mut mutable = request();
        mutable.git_ref = "main".to_string();
        assert!(validate_build_request(&mutable).is_err());
        let mut arbitrary = request();
        arbitrary.profile = "rust-verify; curl example.com".to_string();
        assert!(validate_build_request(&arbitrary).is_err());
        let mut credentialed = request();
        credentialed.repo_url = "https://token@github.com/owner/repo.git".to_string();
        assert!(validate_build_request(&credentialed).is_err());
    }

    #[test]
    fn request_identity_is_deterministic_and_content_bound() {
        let first = request();
        let mut second = request();
        assert_eq!(request_digest(&first), request_digest(&second));
        second.profile = "node-verify".to_string();
        assert_ne!(request_digest(&first), request_digest(&second));
    }

    #[test]
    fn constant_time_auth_compares_digests() {
        assert!(digest_eq("same", "same"));
        assert!(!digest_eq("same", "different"));
    }
}
