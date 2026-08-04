use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Component, Path as FsPath},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use reqwest::{redirect::Policy, Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::{
    net::TcpListener,
    sync::{watch, Mutex},
    time::{timeout, Duration},
};
use tracing::{info, warn};

const SERVICE_NAME: &str = "gha-executor-router";
const MAX_EXECUTORS: usize = 2;
const MAX_UPSTREAM_RESPONSE_BYTES: usize = 32 * 1024;

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    client: Client,
    routes: Arc<Mutex<BTreeMap<String, RouteEntry>>>,
}

#[derive(Clone, Debug)]
struct Config {
    host: String,
    port: u16,
    enabled: bool,
    inbound_auth_secret: Option<String>,
    executors: Vec<Executor>,
    request_timeout: Duration,
    submission_wait_timeout: Duration,
    poll_hint_seconds: u64,
    max_routes: usize,
    max_request_bytes: usize,
}

#[derive(Clone, Debug)]
struct Executor {
    id: String,
    provider: Provider,
    base_url: Url,
    auth_secret: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Provider {
    Aws,
    Hetzner,
}

impl Provider {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "aws" => Ok(Self::Aws),
            "hetzner" => Ok(Self::Hetzner),
            _ => Err("executor provider must be exactly aws or hetzner".to_string()),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Aws => "aws",
            Self::Hetzner => "hetzner",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExecutorSpec {
    id: String,
    provider: String,
    base_url: String,
    auth_secret_path: String,
}

#[derive(Clone)]
struct RouteEntry {
    request_hash: String,
    created_at_ms: u128,
    state: RouteState,
}

#[derive(Clone)]
enum RouteState {
    Pending(watch::Sender<bool>),
    Accepted(RouteRecord),
    Ambiguous(AmbiguousRoute),
}

#[derive(Clone, Debug)]
struct RouteRecord {
    request_id: String,
    route_id: String,
    executor_id: String,
    provider: Provider,
    upstream_id: String,
    status: String,
}

#[derive(Clone, Debug)]
struct AmbiguousRoute {
    request_id: String,
    route_id: String,
    executor_id: String,
    provider: Provider,
    reason: &'static str,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BuildRequest {
    schema_version: String,
    job_kind: String,
    repo_url: String,
    git_ref: String,
    profile: String,
    request_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BuildJobResponse {
    id: String,
    status: String,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RoutedBuildResponse<'a> {
    id: &'a str,
    status: &'a str,
    request_id: &'a str,
    executor_id: &'a str,
    provider: Provider,
    pinned: bool,
    reused: bool,
    upstream_error_present: bool,
}

#[derive(Debug)]
enum Claim {
    Submit(watch::Sender<bool>),
    Wait(watch::Receiver<bool>),
    Existing(RouteRecord),
    Ambiguous(AmbiguousRoute),
    Conflict,
    Capacity,
}

#[derive(Debug)]
enum SubmissionOutcome {
    Accepted(RouteRecord),
    Ambiguous(AmbiguousRoute),
    Rejected {
        status: StatusCode,
        code: &'static str,
    },
}

impl Config {
    fn from_env() -> Result<Self, String> {
        let enabled = env_bool("GHA_EXECUTOR_ROUTER_ENABLED", false)?;
        let inbound_auth_secret = env_optional("GHA_EXECUTOR_ROUTER_AUTH_SECRET_PATH")
            .map(|path| read_secret("GHA_EXECUTOR_ROUTER_AUTH_SECRET_PATH", &path))
            .transpose()?;

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
        let executors = load_executors(specs)?;

        if enabled && inbound_auth_secret.is_none() {
            return Err(
                "GHA_EXECUTOR_ROUTER_AUTH_SECRET_PATH is required when execution is enabled"
                    .to_string(),
            );
        }
        if enabled && executors.is_empty() {
            return Err(
                "at least one complete executor is required when execution is enabled".to_string(),
            );
        }

        Ok(Self {
            host: env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string()),
            port: env_u16("PORT", 8126)?,
            enabled,
            inbound_auth_secret,
            executors,
            request_timeout: Duration::from_secs(env_bounded_u64(
                "GHA_EXECUTOR_ROUTER_REQUEST_TIMEOUT_SECONDS",
                30,
                1,
                60,
            )?),
            submission_wait_timeout: Duration::from_secs(env_bounded_u64(
                "GHA_EXECUTOR_ROUTER_SUBMISSION_WAIT_SECONDS",
                35,
                1,
                90,
            )?),
            poll_hint_seconds: env_bounded_u64("GHA_EXECUTOR_ROUTER_POLL_HINT_SECONDS", 2, 1, 30)?,
            max_routes: env_bounded_usize("GHA_EXECUTOR_ROUTER_MAX_ROUTES", 1024, 1, 4096)?,
            max_request_bytes: env_bounded_usize(
                "GHA_EXECUTOR_ROUTER_MAX_REQUEST_BYTES",
                16 * 1024,
                512,
                64 * 1024,
            )?,
        })
    }

    fn ready(&self) -> bool {
        !self.enabled || (self.inbound_auth_secret.is_some() && !self.executors.is_empty())
    }
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
    let address = format!("{}:{}", config.host, config.port);
    let state = AppState {
        config: Arc::new(config),
        client: Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(60))
            .redirect(Policy::none())
            .user_agent("gha-executor-router/0.1")
            .build()
            .expect("reqwest client"),
        routes: Arc::new(Mutex::new(BTreeMap::new())),
    };
    let app = router(state);
    let listener = TcpListener::bind(&address)
        .await
        .unwrap_or_else(|error| panic!("failed to bind {address}: {error}"));
    info!(%address, "listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
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
        .route("/builds", post(create_build))
        .route("/builds/:id", get(get_build))
        .with_state(state)
}

async fn descriptor() -> Json<Value> {
    Json(json!({
        "service": SERVICE_NAME,
        "purpose": "Fail-closed pre-acceptance routing to fixed-profile AWS and Hetzner build servers",
        "endpoints": {
            "submit": "POST /builds",
            "status": "GET /builds/<routed-id>",
            "health": "GET /healthz",
            "ready": "GET /readyz",
            "capabilities": "GET /v1/capabilities",
            "metrics": "GET /metrics"
        }
    }))
}

async fn healthz(State(state): State<AppState>) -> Json<Value> {
    let routes = state.routes.lock().await;
    let (pending, accepted, ambiguous) = route_counts(&routes);
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "enabled": state.config.enabled,
        "authConfigured": state.config.inbound_auth_secret.is_some(),
        "executors": public_executors(&state.config.executors),
        "routes": {
            "pending": pending,
            "accepted": accepted,
            "ambiguous": ambiguous,
            "retained": routes.len()
        }
    }))
}

async fn readyz(State(state): State<AppState>) -> Response {
    let ready = state.config.ready();
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
            "enabled": state.config.enabled,
            "providers": public_executors(&state.config.executors)
        })),
    )
        .into_response()
}

async fn capabilities(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "service": SERVICE_NAME,
        "schemaVersion": "gha-executor-router.v1",
        "acceptedBuildSchema": "build-server.v1",
        "acceptedJobKind": "run-profile",
        "providers": public_executors(&state.config.executors),
        "failover": {
            "beforeAcceptanceOnly": true,
            "connectErrors": true,
            "http429": true,
            "http5xx": true,
            "contract4xx": false,
            "afterAcceptance": false
        },
        "pollHintSeconds": state.config.poll_hint_seconds,
        "maxRequestBytes": state.config.max_request_bytes,
        "maxRetainedRoutes": state.config.max_routes
    }))
}

async fn metrics(State(state): State<AppState>) -> Response {
    let routes = state.routes.lock().await;
    let (pending, accepted, ambiguous) = route_counts(&routes);
    let body = format!(
        "# HELP gha_executor_router_enabled Whether independent routing is enabled.\n\
# TYPE gha_executor_router_enabled gauge\n\
gha_executor_router_enabled {}\n\
# HELP gha_executor_router_executors Configured executor count.\n\
# TYPE gha_executor_router_executors gauge\n\
gha_executor_router_executors {}\n\
# HELP gha_executor_router_routes Retained routes by state.\n\
# TYPE gha_executor_router_routes gauge\n\
gha_executor_router_routes{{state=\"pending\"}} {pending}\n\
gha_executor_router_routes{{state=\"accepted\"}} {accepted}\n\
gha_executor_router_routes{{state=\"ambiguous\"}} {ambiguous}\n",
        usize::from(state.config.enabled),
        state.config.executors.len()
    );
    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], body).into_response()
}

async fn create_build(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    if !state.config.enabled {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "routing_disabled",
            "independent executor routing is disabled",
        );
    }
    if body.len() > state.config.max_request_bytes {
        return json_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "request_too_large",
            "build request exceeds the configured byte limit",
        );
    }
    let request: BuildRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                "invalid_request_json",
                "request must be the bounded run-profile schema",
            )
        }
    };
    if let Err(detail) = validate_build_request(&request) {
        return json_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_build_request",
            detail,
        );
    }
    let canonical = serde_json::to_vec(&request).expect("serializable build request");
    let request_hash = hex::encode(Sha256::digest(&canonical));

    loop {
        match claim_route(&state, &request.request_id, &request_hash).await {
            Claim::Existing(route) => return accepted_response(&route, true),
            Claim::Ambiguous(route) => return ambiguous_response(&route, true),
            Claim::Conflict => {
                return json_error(
                    StatusCode::CONFLICT,
                    "request_id_conflict",
                    "requestId was already used with a different build request",
                )
            }
            Claim::Capacity => {
                return json_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "route_capacity_exhausted",
                    "no retained route slot is safely reusable",
                )
            }
            Claim::Wait(mut receiver) => {
                if !*receiver.borrow()
                    && timeout(state.config.submission_wait_timeout, receiver.changed())
                        .await
                        .is_err()
                {
                    return json_error(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "submission_claim_timeout",
                        "the authoritative submission claim did not resolve in time",
                    );
                }
            }
            Claim::Submit(done) => {
                let outcome = submit_to_executors(&state, &request, &request_hash).await;
                match outcome {
                    SubmissionOutcome::Accepted(route) => {
                        finish_route(
                            &state,
                            &request.request_id,
                            RouteState::Accepted(route.clone()),
                        )
                        .await;
                        let _ = done.send(true);
                        return accepted_response(&route, false);
                    }
                    SubmissionOutcome::Ambiguous(route) => {
                        finish_route(
                            &state,
                            &request.request_id,
                            RouteState::Ambiguous(route.clone()),
                        )
                        .await;
                        let _ = done.send(true);
                        return ambiguous_response(&route, false);
                    }
                    SubmissionOutcome::Rejected { status, code } => {
                        release_pending_route(&state, &request.request_id, &request_hash).await;
                        let _ = done.send(true);
                        return json_error(
                            status,
                            code,
                            "no executor accepted the deterministic request",
                        );
                    }
                }
            }
        }
    }
}

async fn get_build(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(route_id): Path<String>,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let route = {
        let routes = state.routes.lock().await;
        routes.values().find_map(|entry| match &entry.state {
            RouteState::Accepted(route) if route.route_id == route_id => Some(Ok(route.clone())),
            RouteState::Ambiguous(route) if route.route_id == route_id => Some(Err(route.clone())),
            _ => None,
        })
    };
    let route = match route {
        Some(Ok(route)) => route,
        Some(Err(route)) => return ambiguous_response(&route, true),
        None => {
            return json_error(
                StatusCode::NOT_FOUND,
                "route_not_found",
                "the routed build ID is not retained",
            )
        }
    };
    let Some(executor) = state
        .config
        .executors
        .iter()
        .find(|executor| executor.id == route.executor_id)
    else {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "pinned_executor_missing",
            "the accepted build remains pinned but its executor is not configured",
        );
    };
    let status_url = endpoint_url(&executor.base_url, &format!("builds/{}", route.upstream_id));
    let response = match timeout(
        state.config.request_timeout,
        state
            .client
            .get(status_url)
            .header("x-build-server-auth", &executor.auth_secret)
            .send(),
    )
    .await
    {
        Ok(Ok(response)) => response,
        _ => {
            return json_error(
                StatusCode::BAD_GATEWAY,
                "pinned_executor_unreachable",
                "status lookup failed; the build was not resubmitted elsewhere",
            )
        }
    };
    let status = response.status();
    if !status.is_success() {
        return json_error(
            if status.is_client_error() {
                status
            } else {
                StatusCode::BAD_GATEWAY
            },
            "pinned_executor_status_failed",
            "status lookup failed; the build remains pinned to its accepting executor",
        );
    }
    let body = match bounded_response_body(response).await {
        Ok(body) => body,
        Err(code) => {
            return json_error(
                StatusCode::BAD_GATEWAY,
                code,
                "the accepting executor returned an invalid bounded status response",
            )
        }
    };
    let upstream: BuildJobResponse = match serde_json::from_slice(&body) {
        Ok(upstream) => upstream,
        Err(_) => {
            return json_error(
                StatusCode::BAD_GATEWAY,
                "invalid_status_json",
                "the accepting executor returned invalid status JSON",
            )
        }
    };
    if upstream.id != route.upstream_id || validate_upstream_id(&upstream.id).is_err() {
        return json_error(
            StatusCode::BAD_GATEWAY,
            "status_identity_mismatch",
            "the accepting executor returned a different build identity",
        );
    }
    if validate_status(&upstream.status).is_err() {
        return json_error(
            StatusCode::BAD_GATEWAY,
            "invalid_build_status",
            "the accepting executor returned an unsupported build status",
        );
    }
    update_route_status(&state, &route.request_id, &upstream.status).await;
    let current = RouteRecord {
        status: upstream.status,
        ..route
    };
    routed_response(&current, true, upstream.error.is_some())
}

async fn submit_to_executors(
    state: &AppState,
    request: &BuildRequest,
    request_hash: &str,
) -> SubmissionOutcome {
    for executor in &state.config.executors {
        let url = endpoint_url(&executor.base_url, "builds");
        let response = timeout(
            state.config.request_timeout,
            state
                .client
                .post(url)
                .header("x-build-server-auth", &executor.auth_secret)
                .json(request)
                .send(),
        )
        .await;
        let response = match response {
            Ok(Ok(response)) => response,
            Ok(Err(error)) if error.is_connect() => {
                warn!(executor = %executor.id, provider = %executor.provider.as_str(), "executor connect failed before acceptance");
                continue;
            }
            Ok(Err(_)) | Err(_) => {
                return SubmissionOutcome::Ambiguous(ambiguous_route(
                    request,
                    executor,
                    request_hash,
                    "submission_transport_ambiguous",
                ));
            }
        };
        let status = response.status();
        if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
            warn!(executor = %executor.id, provider = %executor.provider.as_str(), %status, "executor declined before acceptance; trying next provider");
            continue;
        }
        if status.is_client_error() {
            return SubmissionOutcome::Rejected {
                status,
                code: "executor_contract_rejected",
            };
        }
        if status != StatusCode::ACCEPTED {
            return SubmissionOutcome::Ambiguous(ambiguous_route(
                request,
                executor,
                request_hash,
                "unexpected_acceptance_status",
            ));
        }
        let body = match bounded_response_body(response).await {
            Ok(body) => body,
            Err(_) => {
                return SubmissionOutcome::Ambiguous(ambiguous_route(
                    request,
                    executor,
                    request_hash,
                    "invalid_acceptance_body",
                ))
            }
        };
        let upstream: BuildJobResponse = match serde_json::from_slice(&body) {
            Ok(upstream) => upstream,
            Err(_) => {
                return SubmissionOutcome::Ambiguous(ambiguous_route(
                    request,
                    executor,
                    request_hash,
                    "invalid_acceptance_json",
                ))
            }
        };
        if validate_upstream_id(&upstream.id).is_err() || validate_status(&upstream.status).is_err()
        {
            return SubmissionOutcome::Ambiguous(ambiguous_route(
                request,
                executor,
                request_hash,
                "invalid_acceptance_identity",
            ));
        }
        return SubmissionOutcome::Accepted(RouteRecord {
            request_id: request.request_id.clone(),
            route_id: namespaced_route_id(&executor.id, &upstream.id),
            executor_id: executor.id.clone(),
            provider: executor.provider,
            upstream_id: upstream.id,
            status: upstream.status,
        });
    }
    SubmissionOutcome::Rejected {
        status: StatusCode::SERVICE_UNAVAILABLE,
        code: "no_executor_accepted",
    }
}

async fn claim_route(state: &AppState, request_id: &str, request_hash: &str) -> Claim {
    let mut routes = state.routes.lock().await;
    if let Some(existing) = routes.get(request_id) {
        if existing.request_hash != request_hash {
            return Claim::Conflict;
        }
        return match &existing.state {
            RouteState::Pending(sender) => Claim::Wait(sender.subscribe()),
            RouteState::Accepted(route) => Claim::Existing(route.clone()),
            RouteState::Ambiguous(route) => Claim::Ambiguous(route.clone()),
        };
    }
    if routes.len() >= state.config.max_routes && !prune_oldest_accepted(&mut routes) {
        return Claim::Capacity;
    }
    let (sender, _receiver) = watch::channel(false);
    routes.insert(
        request_id.to_string(),
        RouteEntry {
            request_hash: request_hash.to_string(),
            created_at_ms: now_ms(),
            state: RouteState::Pending(sender.clone()),
        },
    );
    Claim::Submit(sender)
}

async fn finish_route(state: &AppState, request_id: &str, route_state: RouteState) {
    if let Some(entry) = state.routes.lock().await.get_mut(request_id) {
        entry.state = route_state;
    }
}

async fn release_pending_route(state: &AppState, request_id: &str, request_hash: &str) {
    let mut routes = state.routes.lock().await;
    let remove = routes
        .get(request_id)
        .map(|entry| {
            entry.request_hash == request_hash && matches!(&entry.state, RouteState::Pending(_))
        })
        .unwrap_or(false);
    if remove {
        routes.remove(request_id);
    }
}

async fn update_route_status(state: &AppState, request_id: &str, status: &str) {
    if let Some(entry) = state.routes.lock().await.get_mut(request_id) {
        if let RouteState::Accepted(route) = &mut entry.state {
            route.status = status.to_string();
        }
    }
}

fn prune_oldest_accepted(routes: &mut BTreeMap<String, RouteEntry>) -> bool {
    let victim = routes
        .iter()
        .filter(|(_, entry)| matches!(&entry.state, RouteState::Accepted(_)))
        .min_by_key(|(_, entry)| entry.created_at_ms)
        .map(|(request_id, _)| request_id.clone());
    if let Some(victim) = victim {
        routes.remove(&victim);
        true
    } else {
        false
    }
}

fn route_counts(routes: &BTreeMap<String, RouteEntry>) -> (usize, usize, usize) {
    routes.values().fold((0, 0, 0), |mut counts, entry| {
        match &entry.state {
            RouteState::Pending(_) => counts.0 += 1,
            RouteState::Accepted(_) => counts.1 += 1,
            RouteState::Ambiguous(_) => counts.2 += 1,
        }
        counts
    })
}

fn public_executors(executors: &[Executor]) -> Vec<Value> {
    executors
        .iter()
        .enumerate()
        .map(|(priority, executor)| {
            json!({
                "id": executor.id,
                "provider": executor.provider,
                "priority": priority,
                "configured": true
            })
        })
        .collect()
}

fn load_executors(specs: Vec<ExecutorSpec>) -> Result<Vec<Executor>, String> {
    if specs.len() > MAX_EXECUTORS {
        return Err(format!(
            "at most {MAX_EXECUTORS} ordered executors are supported"
        ));
    }
    let mut ids = BTreeSet::new();
    let mut providers = BTreeSet::new();
    let mut urls = BTreeSet::new();
    let mut secret_paths = BTreeSet::new();
    let mut executors = Vec::with_capacity(specs.len());
    for spec in specs {
        validate_identifier("executor id", &spec.id, 32)?;
        if !ids.insert(spec.id.clone()) {
            return Err(format!("duplicate executor id {:?}", spec.id));
        }
        let provider = Provider::parse(&spec.provider)?;
        if !providers.insert(provider.as_str()) {
            return Err(format!("duplicate executor provider {:?}", spec.provider));
        }
        let base_url = validate_base_url(&spec.base_url)?;
        let normalized_url = base_url.as_str().to_string();
        if !urls.insert(normalized_url) {
            return Err("executor base URLs must be unique".to_string());
        }
        validate_secret_path(&spec.auth_secret_path)?;
        if !secret_paths.insert(spec.auth_secret_path.clone()) {
            return Err("executor auth secret paths must be unique".to_string());
        }
        let auth_secret = read_secret("executor authSecretPath", &spec.auth_secret_path)?;
        executors.push(Executor {
            id: spec.id,
            provider,
            base_url,
            auth_secret,
        });
    }
    Ok(executors)
}

fn validate_base_url(value: &str) -> Result<Url, String> {
    if value.len() > 512 || value.chars().any(char::is_control) {
        return Err("executor baseUrl must be printable and at most 512 bytes".to_string());
    }
    let mut url = Url::parse(value).map_err(|_| "executor baseUrl is invalid".to_string())?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "executor baseUrl must not contain credentials, query parameters, or fragments"
                .to_string(),
        );
    }
    if !matches!(url.path(), "" | "/") {
        return Err("executor baseUrl path must be empty or /".to_string());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "executor baseUrl must include a host".to_string())?;
    let loopback = matches!(host, "localhost" | "127.0.0.1" | "::1");
    let cluster_dns = host.ends_with(".svc") || host.ends_with(".svc.cluster.local");
    match url.scheme() {
        "https" => {}
        "http" if loopback || cluster_dns => {}
        _ => {
            return Err(
                "executor baseUrl must use HTTPS, in-cluster HTTP service DNS, or loopback HTTP"
                    .to_string(),
            )
        }
    }
    url.set_path("/");
    Ok(url)
}

fn validate_secret_path(value: &str) -> Result<(), String> {
    if value.len() > 240 || value.chars().any(char::is_control) {
        return Err("secret path must be printable and at most 240 bytes".to_string());
    }
    let path = FsPath::new(value);
    if !path.is_absolute() || path == FsPath::new("/") {
        return Err("secret path must be an absolute mounted file path".to_string());
    }
    for component in path.components() {
        if matches!(component, Component::ParentDir | Component::CurDir) {
            return Err("secret path must not contain traversal components".to_string());
        }
    }
    Ok(())
}

fn read_secret(name: &str, path: &str) -> Result<String, String> {
    validate_secret_path(path)?;
    let value =
        fs::read_to_string(path).map_err(|error| format!("{name} is unreadable: {error}"))?;
    let value = value.trim().to_string();
    if value.len() < 16 || value.len() > 4096 || value.chars().any(char::is_control) {
        return Err(format!(
            "{name} must contain 16-4096 printable non-whitespace bytes"
        ));
    }
    Ok(value)
}

fn validate_build_request(request: &BuildRequest) -> Result<(), &'static str> {
    if request.schema_version != "build-server.v1" {
        return Err("schemaVersion must be build-server.v1");
    }
    if request.job_kind != "run-profile" {
        return Err("jobKind must be run-profile");
    }
    validate_repository_url(&request.repo_url)?;
    if request.git_ref.len() != 40 || !request.git_ref.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("gitRef must be a full immutable 40-hex commit SHA");
    }
    validate_identifier("profile", &request.profile, 64)
        .map_err(|_| "profile must be a bounded lowercase token")?;
    if request.request_id.is_empty()
        || request.request_id.len() > 128
        || !request
            .request_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'.' | b'_' | b'-'))
    {
        return Err("requestId must be a bounded deterministic token");
    }
    Ok(())
}

fn validate_repository_url(value: &str) -> Result<(), &'static str> {
    let url = Url::parse(value).map_err(|_| "repoUrl must be a canonical GitHub HTTPS URL")?;
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("repoUrl must be a canonical GitHub HTTPS URL");
    }
    let path = url.path();
    if !path.ends_with(".git") || path.matches('/').count() != 2 || path.contains("..") {
        return Err("repoUrl must identify exactly one owner/repository.git path");
    }
    Ok(())
}

fn validate_identifier(name: &str, value: &str, max_len: usize) -> Result<(), String> {
    if value.is_empty()
        || value.len() > max_len
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(format!(
            "{name} must be 1-{max_len} lowercase ASCII letters, digits, or hyphens"
        ));
    }
    Ok(())
}

fn validate_upstream_id(value: &str) -> Result<(), ()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        Err(())
    } else {
        Ok(())
    }
}

fn validate_status(value: &str) -> Result<(), ()> {
    if matches!(
        value,
        "queued" | "running" | "succeeded" | "failed" | "cancelled"
    ) {
        Ok(())
    } else {
        Err(())
    }
}

fn require_auth(headers: &HeaderMap, state: &AppState) -> Result<(), Response> {
    let Some(expected) = state.config.inbound_auth_secret.as_deref() else {
        return Err(json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "router_auth_unconfigured",
            "inbound router authentication is not configured",
        ));
    };
    let Some(actual) = headers.get("x-build-server-auth") else {
        return Err(json_error(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "missing router authentication",
        ));
    };
    let authorized = actual.as_bytes().len() == expected.len()
        && bool::from(actual.as_bytes().ct_eq(expected.as_bytes()));
    if authorized {
        Ok(())
    } else {
        Err(json_error(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "invalid router authentication",
        ))
    }
}

async fn bounded_response_body(response: reqwest::Response) -> Result<Bytes, &'static str> {
    let body = response
        .bytes()
        .await
        .map_err(|_| "upstream_response_read_failed")?;
    if body.len() > MAX_UPSTREAM_RESPONSE_BYTES {
        Err("upstream_response_too_large")
    } else {
        Ok(body)
    }
}

fn endpoint_url(base: &Url, suffix: &str) -> Url {
    base.join(suffix)
        .expect("validated base URL and safe suffix")
}

fn namespaced_route_id(executor_id: &str, upstream_id: &str) -> String {
    format!("{executor_id}:{upstream_id}")
}

fn ambiguous_route(
    request: &BuildRequest,
    executor: &Executor,
    request_hash: &str,
    reason: &'static str,
) -> AmbiguousRoute {
    AmbiguousRoute {
        request_id: request.request_id.clone(),
        route_id: format!("{}:ambiguous-{}", executor.id, &request_hash[..16]),
        executor_id: executor.id.clone(),
        provider: executor.provider,
        reason,
    }
}

fn accepted_response(route: &RouteRecord, reused: bool) -> Response {
    (
        StatusCode::ACCEPTED,
        Json(routed_body(route, reused, false)),
    )
        .into_response()
}

fn routed_response(route: &RouteRecord, reused: bool, upstream_error_present: bool) -> Response {
    (
        StatusCode::OK,
        Json(routed_body(route, reused, upstream_error_present)),
    )
        .into_response()
}

fn routed_body(
    route: &RouteRecord,
    reused: bool,
    upstream_error_present: bool,
) -> RoutedBuildResponse<'_> {
    RoutedBuildResponse {
        id: &route.route_id,
        status: &route.status,
        request_id: &route.request_id,
        executor_id: &route.executor_id,
        provider: route.provider,
        pinned: true,
        reused,
        upstream_error_present,
    }
}

fn ambiguous_response(route: &AmbiguousRoute, reused: bool) -> Response {
    (
        StatusCode::BAD_GATEWAY,
        Json(json!({
            "error": route.reason,
            "detail": "the request may have reached one executor; it was pinned and was not submitted to another provider",
            "id": route.route_id,
            "requestId": route.request_id,
            "executorId": route.executor_id,
            "provider": route.provider,
            "pinned": true,
            "reused": reused
        })),
    )
        .into_response()
}

fn json_error(status: StatusCode, code: &'static str, detail: impl Into<String>) -> Response {
    (
        status,
        Json(json!({
            "error": code,
            "detail": detail.into()
        })),
    )
        .into_response()
}

fn env_optional(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_bool(name: &str, default: bool) -> Result<bool, String> {
    match env::var(name) {
        Ok(value) => match value.trim() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(format!("{name} must be true or false")),
        },
        Err(_) => Ok(default),
    }
}

fn env_u16(name: &str, default: u16) -> Result<u16, String> {
    match env::var(name) {
        Ok(value) => value
            .trim()
            .parse::<u16>()
            .map_err(|_| format!("{name} must be an integer from 1 to 65535"))
            .and_then(|value| {
                if value == 0 {
                    Err(format!("{name} must be an integer from 1 to 65535"))
                } else {
                    Ok(value)
                }
            }),
        Err(_) => Ok(default),
    }
}

fn env_bounded_u64(name: &str, default: u64, minimum: u64, maximum: u64) -> Result<u64, String> {
    let value = match env::var(name) {
        Ok(value) => value
            .trim()
            .parse::<u64>()
            .map_err(|_| format!("{name} must be an integer"))?,
        Err(_) => default,
    };
    if (minimum..=maximum).contains(&value) {
        Ok(value)
    } else {
        Err(format!("{name} must be between {minimum} and {maximum}"))
    }
}

fn env_bounded_usize(
    name: &str,
    default: usize,
    minimum: usize,
    maximum: usize,
) -> Result<usize, String> {
    let value = match env::var(name) {
        Ok(value) => value
            .trim()
            .parse::<usize>()
            .map_err(|_| format!("{name} must be an integer"))?,
        Err(_) => default,
    };
    if (minimum..=maximum).contains(&value) {
        Ok(value)
    } else {
        Err(format!("{name} must be between {minimum} and {maximum}"))
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> BuildRequest {
        BuildRequest {
            schema_version: "build-server.v1".to_string(),
            job_kind: "run-profile".to_string(),
            repo_url: "https://github.com/ORESoftware/k8s-cluster.git".to_string(),
            git_ref: "0123456789abcdef0123456789abcdef01234567".to_string(),
            profile: "rust-verify".to_string(),
            request_id: "gha-clone:plan:job".to_string(),
        }
    }

    #[test]
    fn provider_names_are_exact() {
        assert_eq!(Provider::parse("aws").unwrap(), Provider::Aws);
        assert_eq!(Provider::parse("hetzner").unwrap(), Provider::Hetzner);
        assert!(Provider::parse("AWS").is_err());
        assert!(Provider::parse("gcp").is_err());
    }

    #[test]
    fn base_urls_allow_https_cluster_dns_and_loopback_only() {
        assert!(validate_base_url("https://builds.example.com").is_ok());
        assert!(
            validate_base_url("http://dd-build-server.dd-next-runtime.svc.cluster.local").is_ok()
        );
        assert!(validate_base_url("http://127.0.0.1:18080").is_ok());
        assert!(validate_base_url("http://builds.example.com").is_err());
        assert!(validate_base_url("ftp://builds.example.com").is_err());
    }

    #[test]
    fn base_urls_reject_credentials_query_fragment_and_paths() {
        for value in [
            "https://user:pass@builds.example.com",
            "https://builds.example.com?token=x",
            "https://builds.example.com#fragment",
            "https://builds.example.com/api",
        ] {
            assert!(validate_base_url(value).is_err(), "{value}");
        }
    }

    #[test]
    fn secret_paths_must_be_absolute_and_traversal_free() {
        assert!(validate_secret_path("/var/run/secrets/gha/aws-auth").is_ok());
        assert!(validate_secret_path("relative/secret").is_err());
        assert!(validate_secret_path("/var/run/secrets/../other").is_err());
        assert!(validate_secret_path("/").is_err());
    }

    #[test]
    fn build_request_is_exact_and_immutable() {
        assert!(validate_build_request(&request()).is_ok());
        let mut mutable = request();
        mutable.git_ref = "main".to_string();
        assert!(validate_build_request(&mutable).is_err());
        let mut command = request();
        command.job_kind = "build-image".to_string();
        assert!(validate_build_request(&command).is_err());
    }

    #[test]
    fn build_request_rejects_noncanonical_repository_and_profile() {
        let mut invalid = request();
        invalid.repo_url = "git@github.com:ORESoftware/k8s-cluster.git".to_string();
        assert!(validate_build_request(&invalid).is_err());
        invalid = request();
        invalid.profile = "sh -c evil".to_string();
        assert!(validate_build_request(&invalid).is_err());
    }

    #[test]
    fn unknown_request_fields_fail_json_deserialization() {
        let value = json!({
            "schemaVersion": "build-server.v1",
            "jobKind": "run-profile",
            "repoUrl": "https://github.com/ORESoftware/k8s-cluster.git",
            "gitRef": "0123456789abcdef0123456789abcdef01234567",
            "profile": "rust-verify",
            "requestId": "gha-clone:plan:job",
            "command": "curl evil"
        });
        assert!(serde_json::from_value::<BuildRequest>(value).is_err());
    }

    #[test]
    fn upstream_ids_and_statuses_are_bounded() {
        assert!(validate_upstream_id("job-123").is_ok());
        assert!(validate_upstream_id("provider:job").is_err());
        assert!(validate_upstream_id("../job").is_err());
        assert!(validate_status("running").is_ok());
        assert!(validate_status("unknown").is_err());
    }

    #[test]
    fn route_ids_are_namespaced_by_executor() {
        assert_eq!(
            namespaced_route_id("aws-primary", "job-123"),
            "aws-primary:job-123"
        );
    }

    #[test]
    fn pruning_removes_only_the_oldest_accepted_route() {
        let mut routes = BTreeMap::new();
        let accepted = |request_id: &str, created_at_ms| RouteEntry {
            request_hash: "hash".to_string(),
            created_at_ms,
            state: RouteState::Accepted(RouteRecord {
                request_id: request_id.to_string(),
                route_id: format!("aws:{request_id}"),
                executor_id: "aws".to_string(),
                provider: Provider::Aws,
                upstream_id: request_id.to_string(),
                status: "queued".to_string(),
            }),
        };
        routes.insert("old".to_string(), accepted("old", 1));
        routes.insert("new".to_string(), accepted("new", 2));
        let (sender, _receiver) = watch::channel(false);
        routes.insert(
            "pending".to_string(),
            RouteEntry {
                request_hash: "hash".to_string(),
                created_at_ms: 0,
                state: RouteState::Pending(sender),
            },
        );
        assert!(prune_oldest_accepted(&mut routes));
        assert!(!routes.contains_key("old"));
        assert!(routes.contains_key("new"));
        assert!(routes.contains_key("pending"));
    }

    #[test]
    fn constant_time_auth_checks_exact_bytes() {
        let expected = b"0123456789abcdef";
        assert!(bool::from(expected.ct_eq(b"0123456789abcdef")));
        assert!(!bool::from(expected.ct_eq(b"0123456789abcdeg")));
    }
}
