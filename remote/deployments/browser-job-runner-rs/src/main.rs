use std::{
    collections::{HashMap, HashSet},
    env,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_nats::Client as NatsClient;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::{process::Command, sync::Mutex, time::sleep};

// dd-browser-job-runner
//
// One pod, one HTTP API, and one bounded Playwright/Puppeteer scenario per
// POST /run. The result is always published to NATS (POST /run is async and
// returns only a jobId). There are two execution paths:
//
//   primary — dd-container-pool: we NATS request/reply the pool's subject
//     (dd.remote.container_pool.browser-jobs.requests). The pool leases a warm
//     dd-browser-job-worker container, HTTP-dispatches the scenario to its /run,
//     and replies with the worker's RunResult. We republish that to the per-job
//     subject + fanout. The warm worker self-exits after one job, so the pool
//     reconciles a fresh replacement (one clean browser per job).
//
//   fallback — direct nerdctl: when the pool is down or cannot serve (no
//     responders / dispatch error / saturated), we spawn a short-lived
//     `nerdctl run` worker ourselves (one-shot mode), which publishes its own
//     result to NATS. This mirrors dd-container-pool / dd-gleam-lambda-runner:
//     a privileged, host-network pod that drives the node's containerd.
//
// Hard rules:
// - Every fallback container is labeled and lives no longer than
//   BROWSER_JOB_MAX_LIFETIME_SECONDS (default 540s / 9 min). This server kills
//   overruns; dd-idle-reaper backstops leaks. Pool containers are reaped by the
//   pool itself.
// - The scenario DSL is bounded (no arbitrary script eval unless explicitly enabled).

const ALLOWED_ACTIONS: &[&str] = &[
    "goto",
    "click",
    "fill",
    "select",
    "press",
    "waitForSelector",
    "waitForUrl",
    "waitForTimeout",
    "extractText",
    "extractAttribute",
    "screenshot",
    "evaluate",
];

const ENGINES: &[&str] = &["playwright", "puppeteer"];

#[derive(Clone)]
struct Config {
    host: String,
    port: u16,
    server_auth_secret: Option<String>,
    allow_unauthenticated: bool,

    nerdctl_bin: String,
    containerd_namespace: String,
    network: String,
    image: String,
    pull_policy: String,

    max_concurrent: usize,
    max_lifetime_seconds: u64,
    default_timeout_ms: u64,
    max_timeout_ms: u64,
    max_steps: usize,
    max_screenshot_bytes: u64,
    browser_headless: bool,
    allow_evaluate: bool,
    default_engine: String,

    container_memory: String,
    container_cpus: String,
    container_shm_size: String,
    pids_limit: u64,
    nofile_limit: u64,

    nerdctl_run_timeout_seconds: u64,
    track_interval_seconds: u64,
    prune_grace_ms: u128,

    nats_url: String,
    result_subject_prefix: String,
    result_fanout_subject: String,

    pool_enabled: bool,
    pool_slug: String,
    pool_subject: String,
    pool_request_timeout_ms: u64,
}

#[derive(Clone)]
struct TrackedJob {
    job_id: String,
    engine: String,
    container_name: String,
    started_ms: u128,
    deadline_ms: u128,
    result_subject: String,
    events_subject: String,
}

#[derive(Default)]
struct Metrics {
    spawned_total: AtomicU64,
    spawn_failures_total: AtomicU64,
    completed_total: AtomicU64,
    killed_total: AtomicU64,
    rejected_total: AtomicU64,
    pool_dispatched_total: AtomicU64,
    pool_failures_total: AtomicU64,
    fallback_total: AtomicU64,
}

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    jobs: Arc<Mutex<HashMap<String, TrackedJob>>>,
    metrics: Arc<Metrics>,
    job_counter: Arc<AtomicU64>,
    server_started_at: Arc<String>,
    nats: Arc<Mutex<Option<NatsClient>>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JobRequest {
    request_id: Option<String>,
    engine: Option<String>,
    url: Option<String>,
    #[serde(default)]
    steps: Vec<Value>,
    timeout_ms: Option<u64>,
    viewport: Option<Value>,
    user_agent: Option<String>,
    extra_headers: Option<Value>,
    capture_final_screenshot: Option<bool>,
    fail_on_console_error: Option<bool>,
}

fn env_string(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_value(name: &str, fallback: &str) -> String {
    env_string(name).unwrap_or_else(|| fallback.to_string())
}

fn env_u64(name: &str, fallback: u64) -> u64 {
    env_string(name)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(fallback)
}

fn env_usize(name: &str, fallback: usize) -> usize {
    env_string(name)
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(fallback)
}

fn env_bool(name: &str, fallback: bool) -> bool {
    env_string(name)
        .map(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(fallback)
}

fn normalize_engine(value: &str, fallback: &str) -> String {
    let lower = value.trim().to_ascii_lowercase();
    if ENGINES.contains(&lower.as_str()) {
        lower
    } else {
        fallback.to_string()
    }
}

fn config_from_env() -> Config {
    let default_engine = normalize_engine(&env_value("BROWSER_JOB_DEFAULT_ENGINE", "playwright"), "playwright");
    let max_lifetime_seconds = env_u64("BROWSER_JOB_MAX_LIFETIME_SECONDS", 540).clamp(30, 540);

    Config {
        host: env_value("HOST", "0.0.0.0"),
        port: env_value("PORT", "8106").parse::<u16>().unwrap_or(8106),
        server_auth_secret: env_string("SERVER_AUTH_SECRET")
            .or_else(|| env_string("BROWSER_JOB_SERVER_AUTH_SECRET")),
        allow_unauthenticated: env_bool("BROWSER_JOB_ALLOW_UNAUTHENTICATED", false),

        nerdctl_bin: env_value("BROWSER_JOB_NERDCTL_BIN", "/usr/local/bin/nerdctl"),
        containerd_namespace: env_value("BROWSER_JOB_CONTAINERD_NAMESPACE", "dd-browser-jobs"),
        network: env_value("BROWSER_JOB_NETWORK", "host"),
        image: env_value(
            "BROWSER_JOB_IMAGE",
            "docker.io/library/dd-browser-job-worker:dev",
        ),
        pull_policy: env_value("BROWSER_JOB_PULL_POLICY", "never"),

        max_concurrent: env_usize("BROWSER_JOB_MAX_CONCURRENT", 4).max(1),
        max_lifetime_seconds,
        default_timeout_ms: env_u64("BROWSER_JOB_DEFAULT_TIMEOUT_MS", 60_000),
        max_timeout_ms: env_u64("BROWSER_JOB_MAX_TIMEOUT_MS", max_lifetime_seconds * 1000),
        max_steps: env_usize("BROWSER_JOB_MAX_STEPS", 64).max(1),
        max_screenshot_bytes: env_u64("BROWSER_JOB_MAX_SCREENSHOT_BYTES", 1_500_000),
        browser_headless: env_bool("BROWSER_JOB_BROWSER_HEADLESS", true),
        allow_evaluate: env_bool("BROWSER_JOB_ALLOW_EVALUATE", false),
        default_engine,

        container_memory: env_value("BROWSER_JOB_CONTAINER_MEMORY", "1g"),
        container_cpus: env_value("BROWSER_JOB_CONTAINER_CPUS", "1"),
        container_shm_size: env_value("BROWSER_JOB_CONTAINER_SHM_SIZE", "512m"),
        pids_limit: env_u64("BROWSER_JOB_PIDS_LIMIT", 512),
        nofile_limit: env_u64("BROWSER_JOB_NOFILE_LIMIT", 8192),

        nerdctl_run_timeout_seconds: env_u64("BROWSER_JOB_NERDCTL_RUN_TIMEOUT_SECONDS", 30),
        track_interval_seconds: env_u64("BROWSER_JOB_TRACK_INTERVAL_SECONDS", 5).max(1),
        prune_grace_ms: env_u64("BROWSER_JOB_PRUNE_GRACE_MS", 8_000) as u128,

        nats_url: env_value("NATS_URL", "nats://dd-nats.messaging.svc.cluster.local:4222"),
        result_subject_prefix: env_value(
            "BROWSER_JOB_NATS_SUBJECT_PREFIX",
            "dd.remote.browser_jobs",
        ),
        result_fanout_subject: env_value(
            "BROWSER_JOB_NATS_RESULT_SUBJECT",
            "dd.remote.browser_jobs.results",
        ),

        pool_enabled: env_bool("BROWSER_JOB_POOL_ENABLED", true),
        pool_slug: env_value("BROWSER_JOB_POOL_SLUG", "browser-jobs"),
        pool_subject: env_value(
            "BROWSER_JOB_POOL_SUBJECT",
            "dd.remote.container_pool.browser-jobs.requests",
        ),
        // Wait at least as long as the pool itself may take (its per-pool request
        // timeout, up to the 9 min lifetime) plus headroom, so a slow-but-working
        // pool returns a real result instead of us prematurely double-spawning.
        pool_request_timeout_ms: env_u64(
            "BROWSER_JOB_POOL_REQUEST_TIMEOUT_MS",
            max_lifetime_seconds * 1000 + 30_000,
        ),
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_millis()
}

fn constant_time_equals(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn request_is_authorized(headers: &HeaderMap, config: &Config) -> bool {
    if config.allow_unauthenticated {
        return true;
    }
    let Some(secret) = config.server_auth_secret.as_deref() else {
        return false;
    };
    ["x-server-auth", "authorization", "x-auth"]
        .iter()
        .filter_map(|name| headers.get(*name))
        .filter_map(|value| value.to_str().ok())
        .map(|value| value.trim_start_matches("Bearer ").trim_start_matches("bearer "))
        .any(|candidate| constant_time_equals(candidate, secret))
}

fn validate_job(request: &JobRequest, config: &Config) -> Result<String, String> {
    let engine = normalize_engine(
        request.engine.as_deref().unwrap_or(&config.default_engine),
        &config.default_engine,
    );
    if !ENGINES.contains(&engine.as_str()) {
        return Err(format!("engine must be one of {ENGINES:?}"));
    }
    if request.steps.is_empty() {
        return Err("steps_required".to_string());
    }
    if request.steps.len() > config.max_steps {
        return Err(format!("too_many_steps (max {})", config.max_steps));
    }
    for (index, step) in request.steps.iter().enumerate() {
        let Some(object) = step.as_object() else {
            return Err(format!("step {index} is not an object"));
        };
        let Some(action) = object.get("action").and_then(Value::as_str) else {
            return Err(format!("step {index} is missing a string \"action\""));
        };
        if !ALLOWED_ACTIONS.contains(&action) {
            return Err(format!("step {index} has unknown action \"{action}\""));
        }
        if action == "evaluate" && !config.allow_evaluate {
            return Err(
                "evaluate steps are disabled (set BROWSER_JOB_ALLOW_EVALUATE=true to enable)"
                    .to_string(),
            );
        }
        let needs_selector = matches!(
            action,
            "click" | "fill" | "select" | "waitForSelector" | "extractText" | "extractAttribute"
        );
        if needs_selector
            && object
                .get("selector")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_none()
        {
            return Err(format!("step {index} ({action}) requires a non-empty \"selector\""));
        }
        if matches!(action, "goto" | "waitForUrl")
            && object
                .get("url")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_none()
        {
            return Err(format!("step {index} ({action}) requires a non-empty \"url\""));
        }
    }
    Ok(engine)
}

fn build_job_spec(request: &JobRequest, engine: &str, job_id: &str, max_ms: u64) -> Value {
    json!({
        "jobId": job_id,
        "requestId": request.request_id,
        "engine": engine,
        "url": request.url,
        "steps": request.steps,
        "timeoutMs": request.timeout_ms,
        "viewport": request.viewport,
        "userAgent": request.user_agent,
        "extraHeaders": request.extra_headers,
        "captureFinalScreenshot": request.capture_final_screenshot,
        "failOnConsoleError": request.fail_on_console_error,
        "maxMs": max_ms,
    })
}

async fn spawn_job(config: &Config, job: &TrackedJob, spec_b64: &str, max_ms: u64) -> Result<(), String> {
    let ns = &config.containerd_namespace;
    let mut command = Command::new(&config.nerdctl_bin);
    command.args(["-n", ns, "run", "-d", "--rm"]);
    command.args(["--name", &job.container_name]);
    command.args(["--label", "dd.browser-job.managed=true"]);
    command.args(["--label", "dd.browser-job.service=dd-browser-job-runner"]);
    command.arg("--label").arg(format!("dd.browser-job.job-id={}", job.job_id));
    command.arg("--label").arg(format!("dd.browser-job.engine={}", job.engine));
    command.arg("--label").arg(format!("dd.browser-job.created-at-ms={}", job.started_ms));
    command.arg("--label").arg(format!("dd.browser-job.deadline-ms={}", job.deadline_ms));
    command.args(["--network", &config.network]);
    command.args(["--cap-drop", "ALL"]);
    command.args(["--security-opt", "no-new-privileges"]);
    command.arg("--pids-limit").arg(config.pids_limit.to_string());
    command.arg("--ulimit").arg(format!("nofile={}:{}", config.nofile_limit, config.nofile_limit));
    command.args(["--memory", &config.container_memory]);
    command.args(["--cpus", &config.container_cpus]);
    command.args(["--shm-size", &config.container_shm_size]);
    command.arg(format!("--pull={}", config.pull_policy));
    command.arg("--env").arg(format!("JOB_SPEC_B64={spec_b64}"));
    command.arg("--env").arg(format!("BROWSER_JOB_ID={}", job.job_id));
    command.arg("--env").arg(format!("NATS_URL={}", config.nats_url));
    command.arg("--env").arg(format!("BROWSER_JOB_RESULT_SUBJECT={}", job.result_subject));
    command.arg("--env").arg(format!("BROWSER_JOB_RESULT_FANOUT_SUBJECT={}", config.result_fanout_subject));
    command.arg("--env").arg(format!("BROWSER_JOB_EVENTS_SUBJECT={}", job.events_subject));
    command.arg("--env").arg(format!("BROWSER_JOB_MAX_MS={max_ms}"));
    command.arg("--env").arg(format!("BROWSER_JOB_HEADLESS={}", config.browser_headless));
    command.arg("--env").arg(format!("BROWSER_JOB_ALLOW_EVALUATE={}", config.allow_evaluate));
    command.arg("--env").arg(format!("BROWSER_JOB_MAX_SCREENSHOT_BYTES={}", config.max_screenshot_bytes));
    command.arg(&config.image);

    let run = tokio::time::timeout(
        Duration::from_secs(config.nerdctl_run_timeout_seconds),
        command.output(),
    )
    .await
    .map_err(|_| "nerdctl run timed out".to_string())?
    .map_err(|error| format!("nerdctl run failed to start: {error}"))?;

    if run.status.success() {
        Ok(())
    } else {
        Err(format!(
            "nerdctl run exited with {}: {}",
            run.status,
            String::from_utf8_lossy(&run.stderr).trim()
        ))
    }
}

async fn force_remove(config: &Config, container_name: &str) {
    let mut command = Command::new(&config.nerdctl_bin);
    command.args(["-n", &config.containerd_namespace, "rm", "-f", container_name]);
    let _ = command.output().await;
}

async fn list_alive_job_ids(config: &Config) -> Option<HashSet<String>> {
    let mut command = Command::new(&config.nerdctl_bin);
    command.args([
        "-n",
        &config.containerd_namespace,
        "ps",
        "--filter",
        "label=dd.browser-job.managed=true",
        "--format",
        "{{.Names}}",
    ]);
    let output = command.output().await.ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let ids = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter_map(|name| name.strip_prefix("dd-browser-job-"))
        .map(ToString::to_string)
        .collect::<HashSet<String>>();
    Some(ids)
}

async fn run_tracker_loop(state: AppState) {
    let interval = Duration::from_secs(state.config.track_interval_seconds);
    loop {
        sleep(interval).await;
        let alive = list_alive_job_ids(&state.config).await;
        let now = now_ms();

        let mut finished: Vec<String> = Vec::new();
        let mut overruns: Vec<(String, String)> = Vec::new();
        {
            let jobs = state.jobs.lock().await;
            for (id, job) in jobs.iter() {
                let alive_now = alive.as_ref().map(|set| set.contains(id));
                match alive_now {
                    // We could read the live set: a tracked job missing from it
                    // (after a startup grace) has exited, so its --rm container
                    // is gone. Treat that as completion.
                    Some(false) if now.saturating_sub(job.started_ms) > state.config.prune_grace_ms => {
                        finished.push(id.clone());
                    }
                    _ if now >= job.deadline_ms => {
                        overruns.push((id.clone(), job.container_name.clone()));
                    }
                    _ => {}
                }
            }
        }

        for id in &finished {
            let mut jobs = state.jobs.lock().await;
            if jobs.remove(id).is_some() {
                state.metrics.completed_total.fetch_add(1, Ordering::Relaxed);
            }
        }
        for (id, container_name) in &overruns {
            force_remove(&state.config, container_name).await;
            let mut jobs = state.jobs.lock().await;
            if jobs.remove(id).is_some() {
                state.metrics.killed_total.fetch_add(1, Ordering::Relaxed);
                tracing::error!("browser-job killed overrun job={id} container={container_name}");
            }
        }
    }
}

// Connect to NATS once at startup (with retry) and store the client. async-nats
// reconnects internally afterwards. While the client is absent we skip the pool
// path and go straight to the nerdctl fallback.
async fn connect_nats_loop(state: AppState) {
    let mut attempt: u32 = 0;
    loop {
        match async_nats::ConnectOptions::new()
            .name("dd-browser-job-runner")
            .connect(&state.config.nats_url)
            .await
        {
            Ok(client) => {
                *state.nats.lock().await = Some(client);
                tracing::info!("dd-browser-job-runner connected to NATS at {}", state.config.nats_url);
                return;
            }
            Err(error) => {
                attempt = attempt.saturating_add(1);
                tracing::error!(
                    "dd-browser-job-runner NATS connect attempt {attempt} failed: {error}; retrying"
                );
                sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

// Ask dd-container-pool to run the scenario on a warm worker. Returns the
// worker's RunResult on success (whether the scenario passed or failed), or an
// Err describing why the pool could not serve, in which case we fall back.
async fn dispatch_via_pool(
    config: &Config,
    client: &NatsClient,
    tracked: &TrackedJob,
    spec: &Value,
) -> Result<Value, String> {
    let request = json!({
        "requestId": tracked.job_id,
        "poolSlug": config.pool_slug,
        "payload": spec,
    });
    let payload = serde_json::to_vec(&request).map_err(|error| error.to_string())?;
    let timeout = Duration::from_millis(config.pool_request_timeout_ms);
    let reply = client
        .send_request(
            config.pool_subject.clone(),
            async_nats::Request::new().payload(payload.into()).timeout(Some(timeout)),
        )
        .await
        .map_err(|error| format!("pool request failed: {error}"))?;

    let value: Value = serde_json::from_slice(&reply.payload)
        .map_err(|error| format!("pool reply was not JSON: {error}"))?;

    // A DispatchResponse carries the worker's RunResult under "body" plus the
    // worker's HTTP "status". A dispatch-level failure (no warm container,
    // lease/transport error) comes back as {"ok": false, "error": ...} with no
    // "body". HTTP 409 means we raced onto a warm worker that already ran its one
    // job — treat that, and any missing body, as "fall back to nerdctl".
    let status = value.get("status").and_then(Value::as_u64).unwrap_or(0);
    match value.get("body") {
        Some(body) if status != 409 => Ok(body.clone()),
        Some(_) => Err("pool worker already consumed (409)".to_string()),
        None => {
            let reason = value
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("pool dispatch returned no result");
            Err(reason.to_string())
        }
    }
}

async fn publish_run_result(
    client: &NatsClient,
    tracked: &TrackedJob,
    result: &Value,
    config: &Config,
) {
    let payload = match serde_json::to_vec(result) {
        Ok(payload) => payload,
        Err(error) => {
            tracing::error!("browser-job result serialize failed job={}: {error}", tracked.job_id);
            return;
        }
    };
    if let Err(error) = client
        .publish(tracked.result_subject.clone(), payload.clone().into())
        .await
    {
        tracing::error!("browser-job result publish failed job={}: {error}", tracked.job_id);
    }
    if !config.result_fanout_subject.is_empty() {
        let _ = client
            .publish(config.result_fanout_subject.clone(), payload.into())
            .await;
    }
    let _ = client.flush().await;
}

fn failed_result_value(tracked: &TrackedJob, error: &str) -> Value {
    json!({
        "ok": false,
        "jobId": tracked.job_id,
        "engine": tracked.engine,
        "durationMs": 0,
        "startedAt": tracked.started_ms,
        "steps": [],
        "extracted": {},
        "screenshots": [],
        "consoleEntries": [],
        "pageErrors": [],
        "error": error,
    })
}

// Drive one accepted job to completion: pool first, nerdctl fallback. Runs
// detached from the HTTP request (POST /run already returned 202).
async fn process_job(state: AppState, tracked: TrackedJob, spec: Value, max_ms: u64) {
    if state.config.pool_enabled {
        let client = state.nats.lock().await.clone();
        if let Some(client) = client {
            match dispatch_via_pool(&state.config, &client, &tracked, &spec).await {
                Ok(result) => {
                    publish_run_result(&client, &tracked, &result, &state.config).await;
                    state.metrics.pool_dispatched_total.fetch_add(1, Ordering::Relaxed);
                    state.metrics.completed_total.fetch_add(1, Ordering::Relaxed);
                    return;
                }
                Err(reason) => {
                    state.metrics.pool_failures_total.fetch_add(1, Ordering::Relaxed);
                    tracing::error!(
                        "browser-job pool path unavailable job={} ({reason}); falling back to nerdctl",
                        tracked.job_id
                    );
                }
            }
        } else {
            tracing::error!(
                "browser-job pool path skipped job={} (no NATS client yet); using nerdctl fallback",
                tracked.job_id
            );
        }
    }

    fallback_spawn(&state, &tracked, &spec, max_ms).await;
}

// Fallback: spawn a one-shot worker directly via nerdctl. The worker publishes
// its own result to NATS, so we only own the spawn, the concurrency slot, and
// the deadline (enforced by run_tracker_loop).
async fn fallback_spawn(state: &AppState, tracked: &TrackedJob, spec: &Value, max_ms: u64) {
    {
        let mut jobs = state.jobs.lock().await;
        if jobs.len() >= state.config.max_concurrent {
            state.metrics.rejected_total.fetch_add(1, Ordering::Relaxed);
            drop(jobs);
            if let Some(client) = state.nats.lock().await.clone() {
                let failed = failed_result_value(tracked, "browser job fallback concurrency limit reached");
                publish_run_result(&client, tracked, &failed, &state.config).await;
            }
            tracing::error!("browser-job fallback rejected job={} (concurrency limit)", tracked.job_id);
            return;
        }
        jobs.insert(tracked.job_id.clone(), tracked.clone());
    }

    let spec_b64 = base64::engine::general_purpose::STANDARD.encode(spec.to_string().as_bytes());
    match spawn_job(&state.config, tracked, &spec_b64, max_ms).await {
        Ok(()) => {
            state.metrics.spawned_total.fetch_add(1, Ordering::Relaxed);
            state.metrics.fallback_total.fetch_add(1, Ordering::Relaxed);
        }
        Err(error) => {
            state.metrics.spawn_failures_total.fetch_add(1, Ordering::Relaxed);
            state.jobs.lock().await.remove(&tracked.job_id);
            force_remove(&state.config, &tracked.container_name).await;
            tracing::error!("browser-job fallback spawn failed job={}: {error}", tracked.job_id);
            if let Some(client) = state.nats.lock().await.clone() {
                let failed = failed_result_value(tracked, &format!("fallback spawn failed: {error}"));
                publish_run_result(&client, tracked, &failed, &state.config).await;
            }
        }
    }
}

fn service_descriptor(state: &AppState) -> Value {
    json!({
        "service": "dd-browser-job-runner",
        "ok": true,
        "model": "per POST /run, runs one bounded scenario on a dd-container-pool warm worker (NATS request/reply), falling back to a direct nerdctl worker when the pool is unavailable; the JSON result is published to NATS",
        "engines": ENGINES,
        "defaultEngine": state.config.default_engine,
        "endpoints": {
            "run": "POST /run",
            "jobs": "GET /browser-jobs/jobs",
            "status": "GET /browser-jobs/status",
            "healthz": "GET /browser-jobs/healthz",
            "metrics": "GET /browser-jobs/metrics",
        },
        "resultSubjectPrefix": state.config.result_subject_prefix,
        "resultFanoutSubject": state.config.result_fanout_subject,
        "pool": {
            "enabled": state.config.pool_enabled,
            "slug": state.config.pool_slug,
            "subject": state.config.pool_subject,
        },
        "maxLifetimeSeconds": state.config.max_lifetime_seconds,
        "allowEvaluate": state.config.allow_evaluate,
    })
}

fn tools_descriptor(state: &AppState) -> Value {
    json!({
        "default": state.config.default_engine,
        "engines": ENGINES.iter().map(|engine| json!({
            "name": engine,
            "supportsHeadless": true,
            "supportsEvaluate": state.config.allow_evaluate,
        })).collect::<Vec<_>>(),
        "image": state.config.image,
    })
}

async fn status_descriptor(state: &AppState) -> Value {
    let in_flight = state.jobs.lock().await.len();
    let nats_connected = state.nats.lock().await.is_some();
    json!({
        "ok": true,
        "service": "dd-browser-job-runner",
        "serverStartedAt": state.server_started_at.as_str(),
        // inFlight counts only fallback nerdctl containers we track; pool jobs are
        // tracked by dd-container-pool, not here.
        "inFlight": in_flight,
        "maxConcurrent": state.config.max_concurrent,
        "maxLifetimeSeconds": state.config.max_lifetime_seconds,
        "maxSteps": state.config.max_steps,
        "containerdNamespace": state.config.containerd_namespace,
        "network": state.config.network,
        "image": state.config.image,
        "natsUrl": state.config.nats_url,
        "natsConnected": nats_connected,
        "poolEnabled": state.config.pool_enabled,
        "poolSlug": state.config.pool_slug,
        "poolSubject": state.config.pool_subject,
        "spawnedTotal": state.metrics.spawned_total.load(Ordering::Relaxed),
        "poolDispatchedTotal": state.metrics.pool_dispatched_total.load(Ordering::Relaxed),
        "poolFailuresTotal": state.metrics.pool_failures_total.load(Ordering::Relaxed),
        "fallbackTotal": state.metrics.fallback_total.load(Ordering::Relaxed),
        "completedTotal": state.metrics.completed_total.load(Ordering::Relaxed),
        "killedTotal": state.metrics.killed_total.load(Ordering::Relaxed),
    })
}

async fn jobs_descriptor(state: &AppState) -> Value {
    let jobs = state.jobs.lock().await;
    let now = now_ms();
    let entries = jobs
        .values()
        .map(|job| {
            json!({
                "jobId": job.job_id,
                "engine": job.engine,
                "containerName": job.container_name,
                "startedAtMs": job.started_ms,
                "deadlineMs": job.deadline_ms,
                "remainingMs": job.deadline_ms.saturating_sub(now),
                "resultSubject": job.result_subject,
                "eventsSubject": job.events_subject,
            })
        })
        .collect::<Vec<_>>();
    json!({ "ok": true, "count": entries.len(), "jobs": entries })
}

fn health_descriptor(state: &AppState) -> Value {
    json!({
        "ok": true,
        "service": "dd-browser-job-runner",
        "serverStartedAt": state.server_started_at.as_str(),
    })
}

fn render_metrics(state: &AppState, in_flight: usize) -> String {
    let m = &state.metrics;
    let mut lines = Vec::new();
    lines.push("# HELP browser_job_in_flight Currently tracked (running) browser job containers.".to_string());
    lines.push("# TYPE browser_job_in_flight gauge".to_string());
    lines.push(format!("browser_job_in_flight {in_flight}"));
    lines.push("# HELP browser_job_spawned_total Total worker containers spawned.".to_string());
    lines.push("# TYPE browser_job_spawned_total counter".to_string());
    lines.push(format!("browser_job_spawned_total {}", m.spawned_total.load(Ordering::Relaxed)));
    lines.push("# HELP browser_job_spawn_failures_total Total nerdctl spawn failures.".to_string());
    lines.push("# TYPE browser_job_spawn_failures_total counter".to_string());
    lines.push(format!("browser_job_spawn_failures_total {}", m.spawn_failures_total.load(Ordering::Relaxed)));
    lines.push("# HELP browser_job_completed_total Total jobs observed to finish on their own.".to_string());
    lines.push("# TYPE browser_job_completed_total counter".to_string());
    lines.push(format!("browser_job_completed_total {}", m.completed_total.load(Ordering::Relaxed)));
    lines.push("# HELP browser_job_killed_total Total jobs force-killed for exceeding their lifetime.".to_string());
    lines.push("# TYPE browser_job_killed_total counter".to_string());
    lines.push(format!("browser_job_killed_total {}", m.killed_total.load(Ordering::Relaxed)));
    lines.push("# HELP browser_job_rejected_total Total fallback spawns rejected over the concurrency cap.".to_string());
    lines.push("# TYPE browser_job_rejected_total counter".to_string());
    lines.push(format!("browser_job_rejected_total {}", m.rejected_total.load(Ordering::Relaxed)));
    lines.push("# HELP browser_job_pool_dispatched_total Total jobs served by the dd-container-pool warm pool.".to_string());
    lines.push("# TYPE browser_job_pool_dispatched_total counter".to_string());
    lines.push(format!("browser_job_pool_dispatched_total {}", m.pool_dispatched_total.load(Ordering::Relaxed)));
    lines.push("# HELP browser_job_pool_failures_total Total pool dispatch attempts that fell back to nerdctl.".to_string());
    lines.push("# TYPE browser_job_pool_failures_total counter".to_string());
    lines.push(format!("browser_job_pool_failures_total {}", m.pool_failures_total.load(Ordering::Relaxed)));
    lines.push("# HELP browser_job_fallback_total Total jobs spawned via the direct nerdctl fallback.".to_string());
    lines.push("# TYPE browser_job_fallback_total counter".to_string());
    lines.push(format!("browser_job_fallback_total {}", m.fallback_total.load(Ordering::Relaxed)));
    format!("{}\n", lines.join("\n"))
}

async fn handle_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<JobRequest>,
) -> impl IntoResponse {
    if !request_is_authorized(&headers, &state.config) {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "ok": false, "error": "unauthorized" })));
    }

    let engine = match validate_job(&request, &state.config) {
        Ok(engine) => engine,
        Err(error) => {
            return (StatusCode::BAD_REQUEST, Json(json!({ "ok": false, "error": error })));
        }
    };

    let max_ms = {
        let requested = request.timeout_ms.unwrap_or(state.config.default_timeout_ms);
        requested.clamp(1_000, state.config.max_timeout_ms).min(state.config.max_lifetime_seconds * 1000)
    };

    let job_id = {
        let seq = state.job_counter.fetch_add(1, Ordering::Relaxed);
        format!("{:x}{:04x}", now_ms(), seq & 0xffff)
    };
    let started_ms = now_ms();
    let deadline_ms = started_ms + (state.config.max_lifetime_seconds as u128) * 1000;
    let container_name = format!("dd-browser-job-{job_id}");
    let result_subject = format!("{}.{job_id}.result", state.config.result_subject_prefix);
    let events_subject = format!("{}.{job_id}.events", state.config.result_subject_prefix);

    let tracked = TrackedJob {
        job_id: job_id.clone(),
        engine: engine.clone(),
        container_name: container_name.clone(),
        started_ms,
        deadline_ms,
        result_subject: result_subject.clone(),
        events_subject: events_subject.clone(),
    };

    let spec = build_job_spec(&request, &engine, &job_id, max_ms);

    // POST /run is async: accept the job, then drive pool-first / nerdctl-fallback
    // in the background. The result always lands on resultSubject (+ fanout).
    tokio::spawn(process_job(state.clone(), tracked, spec, max_ms));

    (
        StatusCode::ACCEPTED,
        Json(json!({
            "ok": true,
            "status": "accepted",
            "jobId": job_id,
            "engine": engine,
            "deadlineMs": deadline_ms,
            "maxMs": max_ms,
            "resultSubject": result_subject,
            "eventsSubject": events_subject,
            "resultFanoutSubject": state.config.result_fanout_subject,
            "poolSubject": state.config.pool_subject,
        })),
    )
}

fn router(state: AppState) -> Router {
    let descriptor_state = state.clone();
    Router::new()
        .route("/", get({
            let state = descriptor_state.clone();
            move || { let state = state.clone(); async move { Json(service_descriptor(&state)) } }
        }))
        .route("/browser-jobs", get({
            let state = descriptor_state.clone();
            move || { let state = state.clone(); async move { Json(service_descriptor(&state)) } }
        }))
        .route("/tools", get({
            let state = descriptor_state.clone();
            move || { let state = state.clone(); async move { Json(tools_descriptor(&state)) } }
        }))
        .route("/browser-jobs/tools", get({
            let state = descriptor_state.clone();
            move || { let state = state.clone(); async move { Json(tools_descriptor(&state)) } }
        }))
        .route("/status", get({
            let state = descriptor_state.clone();
            move || { let state = state.clone(); async move { Json(status_descriptor(&state).await) } }
        }))
        .route("/browser-jobs/status", get({
            let state = descriptor_state.clone();
            move || { let state = state.clone(); async move { Json(status_descriptor(&state).await) } }
        }))
        .route("/jobs", get({
            let state = descriptor_state.clone();
            move || { let state = state.clone(); async move { Json(jobs_descriptor(&state).await) } }
        }))
        .route("/browser-jobs/jobs", get({
            let state = descriptor_state.clone();
            move || { let state = state.clone(); async move { Json(jobs_descriptor(&state).await) } }
        }))
        .route("/healthz", get({
            let state = descriptor_state.clone();
            move || { let state = state.clone(); async move { Json(health_descriptor(&state)) } }
        }))
        .route("/browser-jobs/healthz", get({
            let state = descriptor_state.clone();
            move || { let state = state.clone(); async move { Json(health_descriptor(&state)) } }
        }))
        .route("/readyz", get(|| async { Json(json!({ "status": "ready" })) }))
        .route("/metrics", get({
            let state = descriptor_state.clone();
            move || async move {
                let in_flight = state.jobs.lock().await.len();
                metrics_response(render_metrics(&state, in_flight))
            }
        }))
        .route("/browser-jobs/metrics", get({
            let state = descriptor_state.clone();
            move || async move {
                let in_flight = state.jobs.lock().await.len();
                metrics_response(render_metrics(&state, in_flight))
            }
        }))
        .route("/run", post(handle_run))
        .route("/browser-jobs/run", post(handle_run))
        .with_state(state)
}

fn metrics_response(body: String) -> impl IntoResponse {
    (
        StatusCode::OK,
        [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
        body,
    )
}

#[tokio::main]
async fn main() {
    let _otel = dd_telemetry::init("dd-browser-job-runner");

    let config = Arc::new(config_from_env());
    if config.server_auth_secret.is_none() && !config.allow_unauthenticated {
        tracing::error!(
            "dd-browser-job-runner: SERVER_AUTH_SECRET is unset and BROWSER_JOB_ALLOW_UNAUTHENTICATED \
             is false; POST /run will reject every request until a secret is provided"
        );
    }

    let state = AppState {
        config: config.clone(),
        jobs: Arc::new(Mutex::new(HashMap::new())),
        metrics: Arc::new(Metrics::default()),
        job_counter: Arc::new(AtomicU64::new(0)),
        server_started_at: Arc::new(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis().to_string())
                .unwrap_or_else(|_| "0".to_string()),
        ),
        nats: Arc::new(Mutex::new(None)),
    };

    tokio::spawn(run_tracker_loop(state.clone()));
    tokio::spawn(connect_nats_loop(state.clone()));

    let bind = format!("{}:{}", config.host, config.port);
    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .unwrap_or_else(|error| panic!("failed to bind {bind}: {error}"));
    tracing::info!(
        "dd-browser-job-runner listening on {bind} (namespace={} image={} maxConcurrent={} maxLifetime={}s)",
        config.containerd_namespace, config.image, config.max_concurrent, config.max_lifetime_seconds
    );

    axum::serve(listener, router(state).layer(dd_telemetry::http_trace_layer()))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("dd-browser-job-runner shutting down");
        })
        .await
        .expect("server error");
}
