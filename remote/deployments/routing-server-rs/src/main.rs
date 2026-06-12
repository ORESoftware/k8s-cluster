//! dd-routing-server — a distributed VRP/TSP solver with a live canvas dashboard.
//!
//! Roles (selected with ROUTING_NODE_ROLE):
//!   master  — serves the dashboard + `/api/solve`, fans out `restarts` independent
//!             multi-start jobs over NATS JetStream, and keeps a live *incumbent*
//!             (best routes so far) that the dashboard polls and renders.
//!   worker  — a JetStream consumer that runs one randomized construction + 2-opt
//!             restart per job and publishes the resulting tour back.
//!
//! With no NATS_URL the master runs every restart locally, so a single pod still
//! solves and animates. The dispatch shape mirrors the in-house MIP solver node.

mod dashboard;
mod tsp;

use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::{
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{RwLock, Semaphore};
use uuid::Uuid;

use dd_nats_subject_defs::{
    DD_REMOTE_ROUTING_STREAM_NAME, DD_REMOTE_ROUTING_STREAM_SUBJECTS, ROUTING_EVENTS_SUBJECT,
    ROUTING_JOBS_SUBJECT, ROUTING_RESULTS_SUBJECT, ROUTING_WORKERS_QUEUE_GROUP,
};

const SERVICE_NAME: &str = "dd-routing-server";
const MAX_HTTP_BODY_BYTES: usize = 8 * 1024 * 1024;
const MAX_NATS_PAYLOAD_BYTES: usize = 8 * 1024 * 1024;
const MAX_RESTARTS: usize = 512;
const MAX_TRACKED_SOLVES: usize = 48;
/// Most solves that may run (in background) at once. Each holds a NATS subscription
/// and tracked state; bounding it prevents unbounded fan-out from clients.
const MAX_CONCURRENT_SOLVES: usize = 16;
const GEN_WIDTH: f64 = 1000.0;
const GEN_HEIGHT: f64 = 600.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum NodeRole {
    Master,
    Worker,
}

impl NodeRole {
    fn as_str(self) -> &'static str {
        match self {
            NodeRole::Master => "master",
            NodeRole::Worker => "worker",
        }
    }
}

#[derive(Default)]
struct Metrics {
    http_requests_total: AtomicU64,
    solve_requests_total: AtomicU64,
    jobs_published_total: AtomicU64,
    results_collected_total: AtomicU64,
    worker_jobs_processed_total: AtomicU64,
    improvements_total: AtomicU64,
    rejected_requests_total: AtomicU64,
    errors_total: AtomicU64,
}

#[derive(Clone)]
struct AppState {
    role: NodeRole,
    node_id: String,
    nats: Option<async_nats::Client>,
    jobs_subject: String,
    results_subject: String,
    events_subject: String,
    metrics: Arc<Metrics>,
    solves: Arc<RwLock<HashMap<String, SolveState>>>,
    /// Optional shared secret guarding `POST /api/solve` (dashboard reads stay open).
    auth_secret: Option<String>,
    /// Bounds concurrent background solves.
    solve_permits: Arc<Semaphore>,
}

/// Master-side live view of one solve. Serialized verbatim to the dashboard.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SolveState {
    solve_id: String,
    status: String,
    distributed: bool,
    stops: Vec<tsp::Stop>,
    depot_index: Option<usize>,
    vehicles: usize,
    best_distance: f64,
    routes: Vec<Vec<usize>>,
    restarts_total: usize,
    restarts_done: usize,
    improvements: usize,
    started_ms: u128,
    updated_ms: u128,
}

// ---------------------------------------------------------------------------
// Wire messages
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct RestartJob {
    solve_id: String,
    request_id: String,
    restart_id: usize,
    problem: tsp::Problem,
    local_passes: usize,
    seed: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct RestartResult {
    solve_id: String,
    request_id: String,
    restart_id: usize,
    worker_node: String,
    routes: Vec<Vec<usize>>,
    distance: f64,
    elapsed_ms: f64,
}

// ---------------------------------------------------------------------------
// HTTP request shapes
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SolveRequest {
    #[serde(default)]
    stops: Option<Vec<tsp::Stop>>,
    #[serde(default)]
    depot_index: Option<usize>,
    #[serde(default)]
    vehicle_count: Option<usize>,
    #[serde(default)]
    restarts: Option<usize>,
    #[serde(default)]
    local_passes: Option<usize>,
    #[serde(default)]
    seed: Option<u64>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    generate: Option<GenerateSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GenerateSpec {
    count: usize,
    #[serde(default)]
    vehicles: Option<usize>,
    #[serde(default)]
    seed: Option<u64>,
}

struct SolvePlan {
    problem: tsp::Problem,
    restarts: usize,
    local_passes: usize,
    base_seed: u64,
    timeout: Duration,
}

fn plan_solve(request: SolveRequest) -> Result<SolvePlan, String> {
    let problem = if let Some(stops) = request.stops {
        let vehicles = request.vehicle_count.unwrap_or(1).clamp(1, tsp::MAX_VEHICLES);
        let depot_index = request
            .depot_index
            .or(if vehicles > 1 { Some(0) } else { None });
        tsp::Problem {
            stops,
            depot_index,
            vehicles,
        }
    } else if let Some(generate) = request.generate {
        let vehicles = generate.vehicles.unwrap_or(1).clamp(1, tsp::MAX_VEHICLES);
        let count = generate.count.clamp(2, tsp::MAX_STOPS);
        let seed = generate.seed.unwrap_or_else(|| now_ms() as u64);
        tsp::generate_problem(count, vehicles, GEN_WIDTH, GEN_HEIGHT, seed)
    } else {
        return Err("provide either `stops` or `generate`".into());
    };
    problem.validate()?;

    Ok(SolvePlan {
        problem,
        restarts: request.restarts.unwrap_or(24).clamp(1, MAX_RESTARTS),
        local_passes: request.local_passes.unwrap_or(30).clamp(1, tsp::MAX_LOCAL_PASSES),
        base_seed: request.seed.unwrap_or_else(|| now_ms() as u64),
        timeout: Duration::from_millis(request.timeout_ms.unwrap_or(120_000).clamp(1_000, 600_000)),
    })
}

// ---------------------------------------------------------------------------
// Master: launch + track a solve
// ---------------------------------------------------------------------------

async fn run_solve(state: AppState, solve_id: String, plan: SolvePlan) {
    let distributed = state.nats.is_some();
    publish_event(
        &state,
        "solve-started",
        json!({
            "solveId": &solve_id,
            "stops": plan.problem.stops.len(),
            "vehicles": plan.problem.vehicles,
            "restarts": plan.restarts,
            "distributed": distributed,
        }),
    )
    .await;

    let timed_out = if distributed {
        dispatch_distributed(&state, &solve_id, &plan).await
    } else {
        run_local(&state, &solve_id, &plan).await;
        false
    };

    finalize(&state, &solve_id, if timed_out { "timeout" } else { "complete" }).await;
    let best = state
        .solves
        .read()
        .await
        .get(&solve_id)
        .map(|s| s.best_distance)
        .unwrap_or_default();
    publish_event(
        &state,
        "solve-finished",
        json!({"solveId": &solve_id, "bestDistance": best, "timedOut": timed_out}),
    )
    .await;
}

async fn run_local(state: &AppState, solve_id: &str, plan: &SolvePlan) {
    // Honour the same total budget as the distributed path so a big local run cannot
    // peg a CPU indefinitely in the background.
    let deadline = Instant::now() + plan.timeout;
    for restart_id in 0..plan.restarts {
        if Instant::now() >= deadline {
            break;
        }
        let seed = restart_seed(plan.base_seed, restart_id);
        let problem = plan.problem.clone();
        let passes = plan.local_passes;
        match tokio::task::spawn_blocking(move || tsp::solve_restart(&problem, seed, passes)).await {
            Ok(solution) => apply_result(state, solve_id, solution.routes, solution.distance).await,
            Err(error) => {
                state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                tracing::error!("{SERVICE_NAME} local restart task failed: {error}");
                bump_done(state, solve_id).await;
            }
        }
    }
}

/// Publish one job per restart and fold incoming worker tours into the incumbent
/// until every restart reports or the deadline passes. Returns whether it timed out.
async fn dispatch_distributed(state: &AppState, solve_id: &str, plan: &SolvePlan) -> bool {
    let Some(nats) = state.nats.clone() else {
        return true;
    };
    let mut subscription = match nats.subscribe(state.results_subject.clone()).await {
        Ok(subscription) => subscription,
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            tracing::error!("{SERVICE_NAME} result subscribe failed: {error}");
            return true;
        }
    };

    for restart_id in 0..plan.restarts {
        let job = RestartJob {
            solve_id: solve_id.to_string(),
            request_id: solve_id.to_string(),
            restart_id,
            problem: plan.problem.clone(),
            local_passes: plan.local_passes,
            seed: restart_seed(plan.base_seed, restart_id),
        };
        match serde_json::to_vec(&job) {
            Ok(payload) => {
                if let Err(error) = nats
                    .publish(state.jobs_subject.clone(), payload.into())
                    .await
                {
                    state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                    tracing::error!("{SERVICE_NAME} job publish failed: {error}");
                } else {
                    state.metrics.jobs_published_total.fetch_add(1, Ordering::Relaxed);
                }
            }
            Err(error) => tracing::error!("{SERVICE_NAME} job serialize failed: {error}"),
        }
    }
    let _ = nats.flush().await;

    let deadline = Instant::now() + plan.timeout;
    let mut collected = 0usize;
    while collected < plan.restarts {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return true;
        }
        match tokio::time::timeout(remaining, subscription.next()).await {
            Ok(Some(message)) => {
                if let Ok(result) = serde_json::from_slice::<RestartResult>(&message.payload) {
                    if result.solve_id == solve_id {
                        collected += 1;
                        state.metrics.results_collected_total.fetch_add(1, Ordering::Relaxed);
                        apply_result(state, solve_id, result.routes, result.distance).await;
                    }
                }
            }
            Ok(None) | Err(_) => return true,
        }
    }
    false
}

fn restart_seed(base: u64, restart_id: usize) -> u64 {
    base.wrapping_mul(0x100_0001).wrapping_add(restart_id as u64 + 1)
}

/// Fold one candidate solution in: always counts as a finished restart, and replaces
/// the incumbent when it is the first or strictly shorter. Returns nothing; emits an
/// improvement event + metric when the incumbent changes.
async fn apply_result(state: &AppState, solve_id: &str, routes: Vec<Vec<usize>>, distance: f64) {
    let mut improved = false;
    {
        let mut solves = state.solves.write().await;
        if let Some(entry) = solves.get_mut(solve_id) {
            entry.restarts_done += 1;
            entry.updated_ms = now_ms();
            if distance.is_finite() && (entry.routes.is_empty() || distance + 1e-9 < entry.best_distance)
            {
                entry.best_distance = distance;
                entry.routes = routes;
                entry.improvements += 1;
                improved = true;
            }
        }
    }
    if improved {
        state.metrics.improvements_total.fetch_add(1, Ordering::Relaxed);
        publish_event(
            state,
            "incumbent-improved",
            json!({"solveId": solve_id, "bestDistance": distance}),
        )
        .await;
    }
}

async fn bump_done(state: &AppState, solve_id: &str) {
    let mut solves = state.solves.write().await;
    if let Some(entry) = solves.get_mut(solve_id) {
        entry.restarts_done += 1;
        entry.updated_ms = now_ms();
    }
}

async fn finalize(state: &AppState, solve_id: &str, status: &str) {
    let mut solves = state.solves.write().await;
    if let Some(entry) = solves.get_mut(solve_id) {
        entry.status = status.to_string();
        entry.updated_ms = now_ms();
    }
}

// ---------------------------------------------------------------------------
// Worker (JetStream consumer)
// ---------------------------------------------------------------------------

async fn run_worker(state: AppState) -> Result<(), Box<dyn Error + Send + Sync>> {
    let Some(nats) = state.nats.clone() else {
        tracing::error!("{SERVICE_NAME} worker role requires NATS_URL");
        return Ok(());
    };
    let jetstream = async_nats::jetstream::new(nats.clone());
    let stream = jetstream
        .get_or_create_stream(async_nats::jetstream::stream::Config {
            name: DD_REMOTE_ROUTING_STREAM_NAME.to_string(),
            subjects: DD_REMOTE_ROUTING_STREAM_SUBJECTS
                .iter()
                .map(|subject| subject.to_string())
                .collect(),
            retention: async_nats::jetstream::stream::RetentionPolicy::Limits,
            max_age: Duration::from_secs(60 * 60 * 24),
            max_message_size: MAX_NATS_PAYLOAD_BYTES as i32,
            ..Default::default()
        })
        .await?;
    let consumer_name = env_value("ROUTING_NATS_CONSUMER", ROUTING_WORKERS_QUEUE_GROUP);
    let consumer = stream
        .get_or_create_consumer::<async_nats::jetstream::consumer::pull::Config>(
            &consumer_name,
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some(consumer_name.clone()),
                filter_subject: state.jobs_subject.clone(),
                ack_wait: Duration::from_secs(env_u64("ROUTING_ACK_WAIT_SECONDS", 300)),
                max_ack_pending: env_u64("ROUTING_MAX_ACK_PENDING", 16) as i64,
                max_deliver: 4,
                ..Default::default()
            },
        )
        .await?;

    tracing::info!("{SERVICE_NAME} worker ready: consumer={consumer_name} jobs={}", state.jobs_subject);
    let mut messages = consumer.messages().await?;
    while let Some(message) = messages.next().await {
        let message = match message {
            Ok(message) => message,
            Err(error) => {
                tracing::error!("{SERVICE_NAME} worker fetch failed: {error}");
                continue;
            }
        };
        if message.payload.len() > MAX_NATS_PAYLOAD_BYTES {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            tracing::error!(
                "{SERVICE_NAME} rejected oversize routing job: {} bytes",
                message.payload.len()
            );
            let _ = message.ack().await;
            continue;
        }
        let job = match serde_json::from_slice::<RestartJob>(&message.payload) {
            Ok(job) => job,
            Err(error) => {
                tracing::error!("{SERVICE_NAME} invalid routing job: {error}");
                let _ = message.ack().await;
                continue;
            }
        };
        // Validate a job that may have been published straight to the subject, bypassing
        // the master's plan_solve checks — a malformed problem must not pin a worker.
        if let Err(reason) = job.problem.validate() {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            tracing::error!("{SERVICE_NAME} rejected invalid routing problem: {reason}");
            let _ = message.ack().await;
            continue;
        }
        let node = state.node_id.clone();
        let started = Instant::now();
        let problem = job.problem.clone();
        let passes = job.local_passes.clamp(1, tsp::MAX_LOCAL_PASSES);
        let seed = job.seed;
        let solution =
            match tokio::task::spawn_blocking(move || tsp::solve_restart(&problem, seed, passes)).await
            {
                Ok(solution) => solution,
                Err(error) => {
                    state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                    tracing::error!("{SERVICE_NAME} worker solve task failed: {error}");
                    let _ = message
                        .ack_with(async_nats::jetstream::AckKind::Nak(Some(Duration::from_secs(5))))
                        .await;
                    continue;
                }
            };
        let result = RestartResult {
            solve_id: job.solve_id,
            request_id: job.request_id,
            restart_id: job.restart_id,
            worker_node: node,
            routes: solution.routes,
            distance: solution.distance,
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        };
        match serde_json::to_vec(&result) {
            Ok(payload) => {
                if let Err(error) = nats
                    .publish(state.results_subject.clone(), payload.into())
                    .await
                {
                    tracing::error!("{SERVICE_NAME} worker result publish failed: {error}");
                }
            }
            Err(error) => tracing::error!("{SERVICE_NAME} worker result serialize failed: {error}"),
        }
        state.metrics.worker_jobs_processed_total.fetch_add(1, Ordering::Relaxed);
        if let Err(error) = message.ack().await {
            tracing::error!("{SERVICE_NAME} worker ack failed: {error}");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// HTTP handlers
// ---------------------------------------------------------------------------

async fn dashboard_page(State(state): State<AppState>) -> Html<&'static str> {
    state.metrics.http_requests_total.fetch_add(1, Ordering::Relaxed);
    Html(dashboard::DASHBOARD_HTML)
}

async fn solve_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SolveRequest>,
) -> Response {
    state.metrics.http_requests_total.fetch_add(1, Ordering::Relaxed);
    state.metrics.solve_requests_total.fetch_add(1, Ordering::Relaxed);
    if !authorized(&state, &headers) {
        state.metrics.rejected_requests_total.fetch_add(1, Ordering::Relaxed);
        return error_response(StatusCode::UNAUTHORIZED, "missing or invalid auth token");
    }
    if state.role != NodeRole::Master {
        return error_response(StatusCode::BAD_REQUEST, "submit /api/solve to the master node");
    }
    // Bound concurrent background solves; shed load rather than fan out without limit.
    let permit = match state.solve_permits.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            state.metrics.rejected_requests_total.fetch_add(1, Ordering::Relaxed);
            return error_response(StatusCode::SERVICE_UNAVAILABLE, "solver at capacity; retry shortly");
        }
    };
    let plan = match plan_solve(request) {
        Ok(plan) => plan,
        Err(message) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            return error_response(StatusCode::BAD_REQUEST, &message);
        }
    };

    let solve_id = format!("route-{}", Uuid::new_v4());
    let now = now_ms();
    let entry = SolveState {
        solve_id: solve_id.clone(),
        status: "running".to_string(),
        distributed: state.nats.is_some(),
        stops: plan.problem.stops.clone(),
        depot_index: plan.problem.depot_index,
        vehicles: plan.problem.vehicles,
        best_distance: 0.0,
        routes: Vec::new(),
        restarts_total: plan.restarts,
        restarts_done: 0,
        improvements: 0,
        started_ms: now,
        updated_ms: now,
    };
    {
        let mut solves = state.solves.write().await;
        evict_if_full(&mut solves);
        solves.insert(solve_id.clone(), entry);
    }

    let task_state = state.clone();
    let task_id = solve_id.clone();
    // Hold the permit for the whole background solve so the cap reflects live work.
    tokio::spawn(async move {
        let _permit = permit;
        run_solve(task_state, task_id, plan).await;
    });

    Json(json!({"ok": true, "solveId": solve_id})).into_response()
}

/// Opt-in bearer auth for `POST /api/solve`. When `auth_secret` is unset, all requests
/// pass; when set, the request must present it via `Authorization: Bearer <secret>` or
/// `X-DD-Auth`. Dashboard reads (`/`, `/api/solve/{id}`) are always open.
fn authorized(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(secret) = state.auth_secret.as_deref() else {
        return true;
    };
    let presented = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer ").or(Some(value)))
        .or_else(|| headers.get("x-dd-auth").and_then(|value| value.to_str().ok()));
    presented.is_some_and(|token| constant_time_eq(token.trim().as_bytes(), secret.as_bytes()))
}

/// Length-independent constant-time comparison to avoid leaking the secret by timing.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

async fn solve_state_http(State(state): State<AppState>, Path(solve_id): Path<String>) -> Response {
    state.metrics.http_requests_total.fetch_add(1, Ordering::Relaxed);
    match state.solves.read().await.get(&solve_id) {
        Some(entry) => Json(entry).into_response(),
        None => error_response(StatusCode::NOT_FOUND, "unknown solveId"),
    }
}

fn evict_if_full(solves: &mut HashMap<String, SolveState>) {
    while solves.len() >= MAX_TRACKED_SOLVES {
        if let Some(oldest) = solves
            .values()
            .min_by_key(|s| s.started_ms)
            .map(|s| s.solve_id.clone())
        {
            solves.remove(&oldest);
        } else {
            break;
        }
    }
}

async fn healthz() -> impl IntoResponse {
    Json(json!({"ok": true, "service": SERVICE_NAME}))
}

async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "role": state.role.as_str(),
        "nats": state.nats.is_some(),
    }))
}

async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    let m = &state.metrics;
    let body = format!(
        concat!(
            "# HELP dd_routing_http_requests_total HTTP requests served.\n",
            "# TYPE dd_routing_http_requests_total counter\n",
            "dd_routing_http_requests_total {}\n",
            "# HELP dd_routing_solve_requests_total Solve requests received.\n",
            "# TYPE dd_routing_solve_requests_total counter\n",
            "dd_routing_solve_requests_total {}\n",
            "# HELP dd_routing_jobs_published_total Restart jobs published to NATS.\n",
            "# TYPE dd_routing_jobs_published_total counter\n",
            "dd_routing_jobs_published_total {}\n",
            "# HELP dd_routing_results_collected_total Worker tours aggregated by the master.\n",
            "# TYPE dd_routing_results_collected_total counter\n",
            "dd_routing_results_collected_total {}\n",
            "# HELP dd_routing_worker_jobs_processed_total Restarts solved by this worker.\n",
            "# TYPE dd_routing_worker_jobs_processed_total counter\n",
            "dd_routing_worker_jobs_processed_total {}\n",
            "# HELP dd_routing_improvements_total Incumbent improvements across all solves.\n",
            "# TYPE dd_routing_improvements_total counter\n",
            "dd_routing_improvements_total {}\n",
            "# HELP dd_routing_rejected_requests_total Requests rejected by auth or capacity limits.\n",
            "# TYPE dd_routing_rejected_requests_total counter\n",
            "dd_routing_rejected_requests_total {}\n",
            "# HELP dd_routing_errors_total Errors across HTTP and NATS paths.\n",
            "# TYPE dd_routing_errors_total counter\n",
            "dd_routing_errors_total {}\n",
        ),
        m.http_requests_total.load(Ordering::Relaxed),
        m.solve_requests_total.load(Ordering::Relaxed),
        m.jobs_published_total.load(Ordering::Relaxed),
        m.results_collected_total.load(Ordering::Relaxed),
        m.worker_jobs_processed_total.load(Ordering::Relaxed),
        m.improvements_total.load(Ordering::Relaxed),
        m.rejected_requests_total.load(Ordering::Relaxed),
        m.errors_total.load(Ordering::Relaxed),
    );
    (
        [(axum::http::header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        body,
    )
}

fn error_response(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"ok": false, "error": message}))).into_response()
}

async fn publish_event(state: &AppState, event_name: &str, payload: Value) {
    let Some(nats) = &state.nats else {
        return;
    };
    let event = json!({
        "schema": "dd.routing.event.v1",
        "service": SERVICE_NAME,
        "nodeId": state.node_id,
        "role": state.role.as_str(),
        "eventName": event_name,
        "payload": payload,
        "timeMs": now_ms(),
    });
    if let Ok(bytes) = serde_json::to_vec(&event) {
        let _ = nats.publish(state.events_subject.clone(), bytes.into()).await;
    }
}

// ---------------------------------------------------------------------------
// Bootstrap
// ---------------------------------------------------------------------------

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn env_value(key: &str, default: &str) -> String {
    env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn env_u64(key: &str, default: u64) -> u64 {
    env::var(key).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

/// Connect to NATS with bounded retries so a worker (useless without NATS) or a master
/// started just before NATS is reachable does not silently end up in a degraded state.
async fn connect_nats(url: &str) -> Option<async_nats::Client> {
    let attempts = env_u64("ROUTING_NATS_CONNECT_ATTEMPTS", 30).max(1);
    let retry = Duration::from_secs(env_u64("ROUTING_NATS_CONNECT_RETRY_SECONDS", 2).max(1));
    for attempt in 1..=attempts {
        match async_nats::connect(url).await {
            Ok(client) => {
                tracing::info!("{SERVICE_NAME} connected to NATS at {url}");
                return Some(client);
            }
            Err(error) => {
                tracing::error!("{SERVICE_NAME} NATS connect attempt {attempt}/{attempts} failed: {error}");
                if attempt < attempts {
                    tokio::time::sleep(retry).await;
                }
            }
        }
    }
    None
}

/// Idempotently ensure the JetStream stream exists so the master's job publishes are
/// captured even if no worker pod has started yet.
async fn ensure_stream(client: &async_nats::Client) {
    let jetstream = async_nats::jetstream::new(client.clone());
    if let Err(error) = jetstream
        .get_or_create_stream(async_nats::jetstream::stream::Config {
            name: DD_REMOTE_ROUTING_STREAM_NAME.to_string(),
            subjects: DD_REMOTE_ROUTING_STREAM_SUBJECTS
                .iter()
                .map(|subject| subject.to_string())
                .collect(),
            retention: async_nats::jetstream::stream::RetentionPolicy::Limits,
            max_age: Duration::from_secs(60 * 60 * 24),
            max_message_size: MAX_NATS_PAYLOAD_BYTES as i32,
            ..Default::default()
        })
        .await
    {
        tracing::error!("{SERVICE_NAME} ensure stream failed: {error}");
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let _otel = dd_telemetry::init("dd-routing-server");

    let host = env_value("HOST", "0.0.0.0");
    let port = env_value("PORT", "8132").parse::<u16>()?;
    let role = match env_value("ROUTING_NODE_ROLE", "master").to_ascii_lowercase().as_str() {
        "worker" | "slave" => NodeRole::Worker,
        _ => NodeRole::Master,
    };
    let node_id = env::var("HOSTNAME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("route-{}", Uuid::new_v4()));

    let nats = match env::var("NATS_URL").ok().filter(|v| !v.trim().is_empty()) {
        Some(url) => connect_nats(&url).await,
        None => None,
    };
    if let Some(client) = &nats {
        ensure_stream(client).await;
    } else if role == NodeRole::Worker {
        tracing::error!("{SERVICE_NAME} worker role has no NATS connection; it will idle until restarted");
    }

    let auth_secret = env::var("ROUTING_AUTH_SECRET")
        .ok()
        .filter(|value| !value.trim().is_empty());
    if auth_secret.is_some() {
        tracing::info!("{SERVICE_NAME} POST /api/solve requires a bearer token (ROUTING_AUTH_SECRET set)");
    }

    let state = AppState {
        role,
        node_id,
        nats,
        jobs_subject: env_value("ROUTING_JOBS_SUBJECT", ROUTING_JOBS_SUBJECT),
        results_subject: env_value("ROUTING_RESULTS_SUBJECT", ROUTING_RESULTS_SUBJECT),
        events_subject: env_value("ROUTING_EVENTS_SUBJECT", ROUTING_EVENTS_SUBJECT),
        metrics: Arc::new(Metrics::default()),
        solves: Arc::new(RwLock::new(HashMap::new())),
        auth_secret,
        solve_permits: Arc::new(Semaphore::new(MAX_CONCURRENT_SOLVES)),
    };

    if role == NodeRole::Worker {
        let worker_state = state.clone();
        tokio::spawn(async move {
            if let Err(error) = run_worker(worker_state).await {
                tracing::error!("{SERVICE_NAME} worker loop exited: {error}");
            }
        });
    }

    let app = Router::new()
        .route("/", get(dashboard_page))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
        .route("/api/solve", post(solve_http))
        .route("/api/solve/:solve_id", get(solve_state_http))
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .with_state(state);

    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    tracing::info!("{SERVICE_NAME} ({}) listening on http://{addr}", role.as_str());
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app.layer(dd_telemetry::http_trace_layer()))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
