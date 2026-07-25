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

/// Build the `nerdctl` argument vector for a one-shot fallback worker.
///
/// `-d` (detached — required so the HTTP handler can return 202 immediately while
/// the worker publishes its result to NATS) is used WITHOUT `--rm`: nerdctl
/// rejects `-d --rm` together (unlike Docker — it fails with "flags -d and --rm
/// cannot be specified together"). The container is removed by the tracker's
/// `force_remove` on overrun/failure and by dd-idle-reaper as a backstop.
fn nerdctl_run_args(config: &Config, job: &TrackedJob, spec_b64: &str, max_ms: u64) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    let mut p = |s: String| a.push(s);
    for s in ["-n", &config.containerd_namespace, "run", "-d", "--name", &job.container_name] {
        p(s.to_string());
    }
    for kv in [
        ("--label", "dd.browser-job.managed=true".to_string()),
        ("--label", "dd.browser-job.service=dd-browser-job-runner".to_string()),
        ("--label", format!("dd.browser-job.job-id={}", job.job_id)),
        ("--label", format!("dd.browser-job.engine={}", job.engine)),
        ("--label", format!("dd.browser-job.created-at-ms={}", job.started_ms)),
        ("--label", format!("dd.browser-job.deadline-ms={}", job.deadline_ms)),
        ("--network", config.network.clone()),
        ("--cap-drop", "ALL".to_string()),
        ("--security-opt", "no-new-privileges".to_string()),
        ("--pids-limit", config.pids_limit.to_string()),
        ("--ulimit", format!("nofile={}:{}", config.nofile_limit, config.nofile_limit)),
        ("--memory", config.container_memory.clone()),
        ("--cpus", config.container_cpus.clone()),
        ("--shm-size", config.container_shm_size.clone()),
    ] {
        p(kv.0.to_string());
        p(kv.1);
    }
    p(format!("--pull={}", config.pull_policy));
    for env in [
        format!("JOB_SPEC_B64={spec_b64}"),
        format!("BROWSER_JOB_ID={}", job.job_id),
        format!("NATS_URL={}", config.nats_url),
        format!("BROWSER_JOB_RESULT_SUBJECT={}", job.result_subject),
        format!("BROWSER_JOB_RESULT_FANOUT_SUBJECT={}", config.result_fanout_subject),
        format!("BROWSER_JOB_EVENTS_SUBJECT={}", job.events_subject),
        format!("BROWSER_JOB_MAX_MS={max_ms}"),
        format!("BROWSER_JOB_HEADLESS={}", config.browser_headless),
        format!("BROWSER_JOB_ALLOW_EVALUATE={}", config.allow_evaluate),
        format!("BROWSER_JOB_MAX_SCREENSHOT_BYTES={}", config.max_screenshot_bytes),
    ] {
        p("--env".to_string());
        p(env);
    }
    p(config.image.clone());
    a
}

async fn spawn_job(config: &Config, job: &TrackedJob, spec_b64: &str, max_ms: u64) -> Result<(), String> {
    let mut command = Command::new(&config.nerdctl_bin);
    command.args(nerdctl_run_args(config, job, spec_b64, max_ms));

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

/// Effective per-job timeout (ms): the requested value floored at 1s and capped
/// by both the configured max timeout and the hard lifetime.
///
/// The floor is itself lowered to the ceiling when a (misconfigured) ceiling is
/// below 1s, so this never panics — `u64::clamp` panics when `min > max`, which
/// the previous inline `requested.clamp(1_000, max_timeout_ms)` did on every
/// authorized request whenever `BROWSER_JOB_MAX_TIMEOUT_MS` was set below 1000.
fn effective_max_ms(requested_ms: u64, max_timeout_ms: u64, max_lifetime_seconds: u64) -> u64 {
    let ceiling = max_timeout_ms.min(max_lifetime_seconds.saturating_mul(1000));
    requested_ms.clamp(1_000.min(ceiling), ceiling)
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

    let max_ms = effective_max_ms(
        request.timeout_ms.unwrap_or(state.config.default_timeout_ms),
        state.config.max_timeout_ms,
        state.config.max_lifetime_seconds,
    );

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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderName, HeaderValue};

    // ---- effective_max_ms ------------------------------------------------

    #[test]
    fn effective_max_ms_normal_floor_and_caps() {
        // Typical: honored as-is within bounds.
        assert_eq!(effective_max_ms(30_000, 540_000, 540), 30_000);
        // Floored at 1s.
        assert_eq!(effective_max_ms(100, 540_000, 540), 1_000);
        // Capped by the configured max timeout.
        assert_eq!(effective_max_ms(1_000_000, 60_000, 540), 60_000);
        // Capped by the hard lifetime (lifetime*1000 < max_timeout).
        assert_eq!(effective_max_ms(1_000_000, 600_000, 30), 30_000);
    }

    #[test]
    fn effective_max_ms_does_not_panic_when_ceiling_below_floor() {
        // Regression: a misconfigured ceiling < 1s must not panic (the old inline
        // `clamp(1_000, max_timeout_ms)` panicked on min > max). The ceiling wins.
        assert_eq!(effective_max_ms(50_000, 500, 540), 500);
        assert_eq!(effective_max_ms(50_000, 0, 540), 0);
        assert_eq!(effective_max_ms(50_000, 100_000, 0), 0);
    }

    // ---- env test harness -------------------------------------------------
    //
    // std::env::set_var / remove_var mutate global process state shared by every
    // thread, and cargo runs tests in parallel. Every test that reads or writes
    // the environment holds this lock, so those tests run one-at-a-time. We
    // recover from a poisoned lock so a panicking test can't cascade into
    // spurious failures elsewhere.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Saves an env var on construction and restores it (or removes it if it was
    /// absent) on drop, so env-mutating tests leave no residue for the next one.
    struct EnvGuard {
        key: String,
        prev: Option<String>,
    }
    impl EnvGuard {
        fn set(key: &str, value: &str) -> Self {
            let prev = env::var(key).ok();
            env::set_var(key, value);
            Self { key: key.to_string(), prev }
        }
        fn remove(key: &str) -> Self {
            let prev = env::var(key).ok();
            env::remove_var(key);
            Self { key: key.to_string(), prev }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(value) => env::set_var(&self.key, value),
                None => env::remove_var(&self.key),
            }
        }
    }

    // Every env var config_from_env() consults. Cleared so we can observe the
    // documented defaults regardless of the ambient shell (HOST in particular is
    // commonly set).
    const CONFIG_ENV_VARS: &[&str] = &[
        "HOST",
        "PORT",
        "SERVER_AUTH_SECRET",
        "BROWSER_JOB_SERVER_AUTH_SECRET",
        "BROWSER_JOB_ALLOW_UNAUTHENTICATED",
        "BROWSER_JOB_NERDCTL_BIN",
        "BROWSER_JOB_CONTAINERD_NAMESPACE",
        "BROWSER_JOB_NETWORK",
        "BROWSER_JOB_IMAGE",
        "BROWSER_JOB_PULL_POLICY",
        "BROWSER_JOB_MAX_CONCURRENT",
        "BROWSER_JOB_MAX_LIFETIME_SECONDS",
        "BROWSER_JOB_DEFAULT_TIMEOUT_MS",
        "BROWSER_JOB_MAX_TIMEOUT_MS",
        "BROWSER_JOB_MAX_STEPS",
        "BROWSER_JOB_MAX_SCREENSHOT_BYTES",
        "BROWSER_JOB_BROWSER_HEADLESS",
        "BROWSER_JOB_ALLOW_EVALUATE",
        "BROWSER_JOB_DEFAULT_ENGINE",
        "BROWSER_JOB_CONTAINER_MEMORY",
        "BROWSER_JOB_CONTAINER_CPUS",
        "BROWSER_JOB_CONTAINER_SHM_SIZE",
        "BROWSER_JOB_PIDS_LIMIT",
        "BROWSER_JOB_NOFILE_LIMIT",
        "BROWSER_JOB_NERDCTL_RUN_TIMEOUT_SECONDS",
        "BROWSER_JOB_TRACK_INTERVAL_SECONDS",
        "BROWSER_JOB_PRUNE_GRACE_MS",
        "NATS_URL",
        "BROWSER_JOB_NATS_SUBJECT_PREFIX",
        "BROWSER_JOB_NATS_RESULT_SUBJECT",
        "BROWSER_JOB_POOL_ENABLED",
        "BROWSER_JOB_POOL_SLUG",
        "BROWSER_JOB_POOL_SUBJECT",
        "BROWSER_JOB_POOL_REQUEST_TIMEOUT_MS",
    ];

    fn clear_config_env() -> Vec<EnvGuard> {
        CONFIG_ENV_VARS.iter().map(|key| EnvGuard::remove(key)).collect()
    }

    // ---- fixtures (no env) ------------------------------------------------

    /// A Config mirroring config_from_env()'s documented defaults, built without
    /// touching the environment so the pure-logic tests can run in parallel.
    fn sample_tracked_job() -> TrackedJob {
        TrackedJob {
            job_id: "abc123".to_string(),
            engine: "playwright".to_string(),
            container_name: "dd-browser-job-abc123".to_string(),
            started_ms: 1_000,
            deadline_ms: 61_000,
            result_subject: "dd.remote.browser_jobs.abc123.result".to_string(),
            events_subject: "dd.remote.browser_jobs.abc123.events".to_string(),
        }
    }

    #[test]
    fn nerdctl_run_args_are_detached_without_rm() {
        // Regression (found by E2E against the deployed runner): nerdctl rejects
        // `-d --rm` together, which made every nerdctl-fallback browser job fail.
        let args = nerdctl_run_args(&base_config(), &sample_tracked_job(), "c3BlYw==", 60_000);
        assert!(args.iter().any(|a| a == "-d"), "must run detached");
        assert!(
            !args.iter().any(|a| a == "--rm"),
            "must NOT pass --rm — nerdctl rejects `-d --rm`"
        );
        // Shape sanity: `-n <ns> run`, container name, env, image last.
        assert_eq!(args.first().unwrap(), "-n");
        assert!(args.contains(&"run".to_string()));
        assert!(args.windows(2).any(|w| w[0] == "--name" && w[1] == "dd-browser-job-abc123"));
        assert!(args.iter().any(|a| a == "JOB_SPEC_B64=c3BlYw=="));
        assert!(args.iter().any(|a| a == "BROWSER_JOB_ID=abc123"));
        assert_eq!(args.last().unwrap(), &base_config().image);
    }

    fn base_config() -> Config {
        Config {
            host: "0.0.0.0".to_string(),
            port: 8106,
            server_auth_secret: None,
            allow_unauthenticated: false,
            nerdctl_bin: "/usr/local/bin/nerdctl".to_string(),
            containerd_namespace: "dd-browser-jobs".to_string(),
            network: "host".to_string(),
            image: "docker.io/library/dd-browser-job-worker:dev".to_string(),
            pull_policy: "never".to_string(),
            max_concurrent: 4,
            max_lifetime_seconds: 540,
            default_timeout_ms: 60_000,
            max_timeout_ms: 540_000,
            max_steps: 64,
            max_screenshot_bytes: 1_500_000,
            browser_headless: true,
            allow_evaluate: false,
            default_engine: "playwright".to_string(),
            container_memory: "1g".to_string(),
            container_cpus: "1".to_string(),
            container_shm_size: "512m".to_string(),
            pids_limit: 512,
            nofile_limit: 8192,
            nerdctl_run_timeout_seconds: 30,
            track_interval_seconds: 5,
            prune_grace_ms: 8_000,
            nats_url: "nats://dd-nats.messaging.svc.cluster.local:4222".to_string(),
            result_subject_prefix: "dd.remote.browser_jobs".to_string(),
            result_fanout_subject: "dd.remote.browser_jobs.results".to_string(),
            pool_enabled: true,
            pool_slug: "browser-jobs".to_string(),
            pool_subject: "dd.remote.container_pool.browser-jobs.requests".to_string(),
            pool_request_timeout_ms: 570_000,
        }
    }

    fn app_state(config: Config) -> AppState {
        AppState {
            config: Arc::new(config),
            jobs: Arc::new(Mutex::new(HashMap::<String, TrackedJob>::new())),
            metrics: Arc::new(Metrics::default()),
            job_counter: Arc::new(AtomicU64::new(0)),
            server_started_at: Arc::new("0".to_string()),
            nats: Arc::new(Mutex::new(None::<NatsClient>)),
        }
    }

    fn job_request(steps: Vec<Value>) -> JobRequest {
        JobRequest {
            request_id: None,
            engine: None,
            url: None,
            steps,
            timeout_ms: None,
            viewport: None,
            user_agent: None,
            extra_headers: None,
            capture_final_screenshot: None,
            fail_on_console_error: None,
        }
    }

    fn goto_step(url: &str) -> Value {
        json!({ "action": "goto", "url": url })
    }

    fn sample_tracked() -> TrackedJob {
        TrackedJob {
            job_id: "job-xyz".to_string(),
            engine: "playwright".to_string(),
            container_name: "dd-browser-job-job-xyz".to_string(),
            started_ms: 111,
            deadline_ms: 222,
            result_subject: "dd.remote.browser_jobs.job-xyz.result".to_string(),
            events_subject: "dd.remote.browser_jobs.job-xyz.events".to_string(),
        }
    }

    fn headers_with(name: &str, value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_bytes(name.as_bytes()).unwrap(),
            HeaderValue::from_str(value).unwrap(),
        );
        headers
    }

    // ---- env parsing helpers ---------------------------------------------

    #[test]
    fn env_string_trims_and_treats_blank_as_absent() {
        let _lock = env_lock();
        let key = "BJT_ENV_STRING";
        {
            let _g = EnvGuard::remove(key);
            assert_eq!(env_string(key), None, "unset -> None");
        }
        {
            let _g = EnvGuard::set(key, "  hello  ");
            assert_eq!(env_string(key), Some("hello".to_string()), "trimmed");
        }
        {
            let _g = EnvGuard::set(key, "");
            assert_eq!(env_string(key), None, "empty -> None");
        }
        {
            let _g = EnvGuard::set(key, "   ");
            assert_eq!(env_string(key), None, "whitespace-only -> None");
        }
    }

    #[test]
    fn env_value_uses_fallback_when_absent_or_blank() {
        let _lock = env_lock();
        let key = "BJT_ENV_VALUE";
        {
            let _g = EnvGuard::remove(key);
            assert_eq!(env_value(key, "fb"), "fb");
        }
        {
            let _g = EnvGuard::set(key, "  set  ");
            assert_eq!(env_value(key, "fb"), "set");
        }
        {
            let _g = EnvGuard::set(key, "   ");
            assert_eq!(env_value(key, "fb"), "fb", "whitespace-only -> fallback");
        }
    }

    #[test]
    fn env_u64_parses_or_falls_back() {
        let _lock = env_lock();
        let key = "BJT_ENV_U64";
        {
            let _g = EnvGuard::remove(key);
            assert_eq!(env_u64(key, 7), 7, "unset -> fallback");
        }
        {
            let _g = EnvGuard::set(key, "123");
            assert_eq!(env_u64(key, 7), 123);
        }
        {
            let _g = EnvGuard::set(key, "  456  ");
            assert_eq!(env_u64(key, 7), 456, "trimmed then parsed");
        }
        for bad in ["notanumber", "-5", "3.5", ""] {
            let _g = EnvGuard::set(key, bad);
            assert_eq!(env_u64(key, 7), 7, "malformed {bad:?} -> fallback");
        }
    }

    #[test]
    fn env_usize_parses_or_falls_back() {
        let _lock = env_lock();
        let key = "BJT_ENV_USIZE";
        {
            let _g = EnvGuard::remove(key);
            assert_eq!(env_usize(key, 9), 9);
        }
        {
            let _g = EnvGuard::set(key, "42");
            assert_eq!(env_usize(key, 9), 42);
        }
        {
            let _g = EnvGuard::set(key, "oops");
            assert_eq!(env_usize(key, 9), 9, "malformed -> fallback");
        }
    }

    #[test]
    fn env_bool_recognizes_truthy_tokens_case_insensitively() {
        let _lock = env_lock();
        let key = "BJT_ENV_BOOL";
        for truthy in ["1", "true", "TRUE", "Yes", "on", "ON"] {
            let _g = EnvGuard::set(key, truthy);
            assert!(env_bool(key, false), "{truthy:?} should parse true");
        }
        // A set-but-unrecognized value resolves to false, IGNORING the fallback.
        for falsy in ["0", "false", "no", "off", "2", "garbage"] {
            let _g = EnvGuard::set(key, falsy);
            assert!(
                !env_bool(key, true),
                "{falsy:?} should be false even though fallback is true"
            );
        }
        {
            let _g = EnvGuard::remove(key);
            assert!(env_bool(key, true), "unset -> fallback true");
            assert!(!env_bool(key, false), "unset -> fallback false");
        }
        {
            let _g = EnvGuard::set(key, "   ");
            assert!(env_bool(key, true), "whitespace-only treated as absent -> fallback");
        }
    }

    #[test]
    fn normalize_engine_lowercases_known_and_falls_back_otherwise() {
        assert_eq!(normalize_engine("playwright", "playwright"), "playwright");
        assert_eq!(normalize_engine("Puppeteer", "playwright"), "puppeteer");
        assert_eq!(normalize_engine("  PLAYWRIGHT  ", "puppeteer"), "playwright");
        assert_eq!(normalize_engine("chromium", "playwright"), "playwright", "unknown -> fallback");
        assert_eq!(normalize_engine("", "puppeteer"), "puppeteer", "empty -> fallback");
        // The fallback is returned verbatim and is NOT itself validated.
        assert_eq!(normalize_engine("nope", "WeIrD"), "WeIrD");
    }

    // ---- config_from_env --------------------------------------------------

    #[test]
    fn config_from_env_defaults_when_unset() {
        let _lock = env_lock();
        let _guards = clear_config_env();
        let c = config_from_env();
        assert_eq!(c.host, "0.0.0.0");
        assert_eq!(c.port, 8106);
        assert_eq!(c.server_auth_secret, None);
        assert!(!c.allow_unauthenticated);
        assert_eq!(c.nerdctl_bin, "/usr/local/bin/nerdctl");
        assert_eq!(c.containerd_namespace, "dd-browser-jobs");
        assert_eq!(c.network, "host");
        assert_eq!(c.image, "docker.io/library/dd-browser-job-worker:dev");
        assert_eq!(c.pull_policy, "never");
        assert_eq!(c.max_concurrent, 4);
        assert_eq!(c.max_lifetime_seconds, 540);
        assert_eq!(c.default_timeout_ms, 60_000);
        assert_eq!(c.max_timeout_ms, 540_000, "defaults to max_lifetime * 1000");
        assert_eq!(c.max_steps, 64);
        assert_eq!(c.max_screenshot_bytes, 1_500_000);
        assert!(c.browser_headless);
        assert!(!c.allow_evaluate);
        assert_eq!(c.default_engine, "playwright");
        assert_eq!(c.container_memory, "1g");
        assert_eq!(c.container_cpus, "1");
        assert_eq!(c.container_shm_size, "512m");
        assert_eq!(c.pids_limit, 512);
        assert_eq!(c.nofile_limit, 8192);
        assert_eq!(c.nerdctl_run_timeout_seconds, 30);
        assert_eq!(c.track_interval_seconds, 5);
        assert_eq!(c.prune_grace_ms, 8_000);
        assert_eq!(c.nats_url, "nats://dd-nats.messaging.svc.cluster.local:4222");
        assert_eq!(c.result_subject_prefix, "dd.remote.browser_jobs");
        assert_eq!(c.result_fanout_subject, "dd.remote.browser_jobs.results");
        assert!(c.pool_enabled);
        assert_eq!(c.pool_slug, "browser-jobs");
        assert_eq!(c.pool_subject, "dd.remote.container_pool.browser-jobs.requests");
        assert_eq!(c.pool_request_timeout_ms, 570_000, "max_lifetime*1000 + 30_000");
    }

    #[test]
    fn config_from_env_applies_overrides_and_derived_timeouts() {
        let _lock = env_lock();
        let _guards = clear_config_env();
        let _e1 = EnvGuard::set("BROWSER_JOB_MAX_LIFETIME_SECONDS", "60");
        let _e2 = EnvGuard::set("BROWSER_JOB_POOL_ENABLED", "false");
        let _e3 = EnvGuard::set("BROWSER_JOB_POOL_SLUG", "custom-pool");
        let _e4 = EnvGuard::set("BROWSER_JOB_DEFAULT_ENGINE", "Puppeteer");
        let _e5 = EnvGuard::set("BROWSER_JOB_NERDCTL_BIN", "/opt/nerdctl");
        let c = config_from_env();
        assert_eq!(c.max_lifetime_seconds, 60);
        // Both derived timeouts track the (already clamped) lifetime.
        assert_eq!(c.max_timeout_ms, 60_000, "default max_timeout = lifetime * 1000");
        assert_eq!(c.pool_request_timeout_ms, 90_000, "lifetime * 1000 + 30_000");
        assert!(!c.pool_enabled);
        assert_eq!(c.pool_slug, "custom-pool");
        assert_eq!(c.default_engine, "puppeteer", "engine is normalized");
        assert_eq!(c.nerdctl_bin, "/opt/nerdctl");
    }

    #[test]
    fn config_from_env_clamps_lifetime_and_floors_counts() {
        let _lock = env_lock();
        let _guards = clear_config_env();
        {
            let _e = EnvGuard::set("BROWSER_JOB_MAX_LIFETIME_SECONDS", "99999");
            assert_eq!(config_from_env().max_lifetime_seconds, 540, "clamped to 540 ceiling");
        }
        {
            let _e = EnvGuard::set("BROWSER_JOB_MAX_LIFETIME_SECONDS", "1");
            assert_eq!(config_from_env().max_lifetime_seconds, 30, "clamped to 30 floor");
        }
        {
            let _a = EnvGuard::set("BROWSER_JOB_MAX_CONCURRENT", "0");
            let _b = EnvGuard::set("BROWSER_JOB_MAX_STEPS", "0");
            let _c = EnvGuard::set("BROWSER_JOB_TRACK_INTERVAL_SECONDS", "0");
            let cfg = config_from_env();
            assert_eq!(cfg.max_concurrent, 1, "floored to >= 1");
            assert_eq!(cfg.max_steps, 1, "floored to >= 1");
            assert_eq!(cfg.track_interval_seconds, 1, "floored to >= 1");
        }
    }

    #[test]
    fn config_from_env_prefers_server_auth_secret_over_browser_job_variant() {
        let _lock = env_lock();
        let _guards = clear_config_env();
        {
            let _s = EnvGuard::set("BROWSER_JOB_SERVER_AUTH_SECRET", "browser-var");
            assert_eq!(
                config_from_env().server_auth_secret.as_deref(),
                Some("browser-var"),
                "falls back to BROWSER_JOB_SERVER_AUTH_SECRET"
            );
        }
        {
            let _p = EnvGuard::set("SERVER_AUTH_SECRET", "primary");
            let _f = EnvGuard::set("BROWSER_JOB_SERVER_AUTH_SECRET", "browser-var");
            assert_eq!(
                config_from_env().server_auth_secret.as_deref(),
                Some("primary"),
                "SERVER_AUTH_SECRET takes precedence"
            );
        }
    }

    // ---- constant_time_equals / auth -------------------------------------

    #[test]
    fn constant_time_equals_matches_only_identical_strings() {
        assert!(constant_time_equals("", ""));
        assert!(constant_time_equals("secret", "secret"));
        assert!(!constant_time_equals("secret", "secrets"), "different length");
        assert!(!constant_time_equals("secret", "SECRET"), "case sensitive");
        assert!(!constant_time_equals("secret", "sacret"), "same length, one byte off");
    }

    #[test]
    fn request_is_authorized_allows_when_unauthenticated_flag_set() {
        let mut config = base_config();
        config.allow_unauthenticated = true;
        config.server_auth_secret = None;
        assert!(
            request_is_authorized(&HeaderMap::new(), &config),
            "allow_unauthenticated bypasses auth even with no secret and no header"
        );
    }

    #[test]
    fn request_is_authorized_denies_when_no_secret_configured() {
        let mut config = base_config();
        config.allow_unauthenticated = false;
        config.server_auth_secret = None;
        assert!(!request_is_authorized(&headers_with("x-server-auth", "anything"), &config));
    }

    #[test]
    fn request_is_authorized_accepts_secret_across_headers_and_bearer_forms() {
        let mut config = base_config();
        config.allow_unauthenticated = false;
        config.server_auth_secret = Some("s3cr3t".to_string());

        assert!(request_is_authorized(&headers_with("x-server-auth", "s3cr3t"), &config));
        assert!(request_is_authorized(&headers_with("x-auth", "s3cr3t"), &config));
        assert!(
            request_is_authorized(&headers_with("authorization", "Bearer s3cr3t"), &config),
            "strips 'Bearer ' prefix"
        );
        assert!(
            request_is_authorized(&headers_with("authorization", "bearer s3cr3t"), &config),
            "strips lowercase 'bearer ' prefix"
        );
        assert!(
            request_is_authorized(&headers_with("authorization", "s3cr3t"), &config),
            "a raw secret without a bearer prefix is also accepted"
        );

        assert!(!request_is_authorized(&headers_with("x-server-auth", "wrong"), &config));
        assert!(!request_is_authorized(&HeaderMap::new(), &config), "no header -> denied");
    }

    // ---- validate_job -----------------------------------------------------

    #[test]
    fn validate_job_requires_at_least_one_step() {
        let config = base_config();
        assert_eq!(
            validate_job(&job_request(vec![]), &config),
            Err("steps_required".to_string())
        );
    }

    #[test]
    fn validate_job_enforces_max_steps() {
        let mut config = base_config();
        config.max_steps = 2;
        let req = job_request(vec![goto_step("https://a"), goto_step("https://b"), goto_step("https://c")]);
        assert_eq!(validate_job(&req, &config), Err("too_many_steps (max 2)".to_string()));
    }

    #[test]
    fn validate_job_uses_default_engine_and_normalizes_requested() {
        let config = base_config(); // default_engine = "playwright"
        let ok = job_request(vec![goto_step("https://example.com")]);
        assert_eq!(validate_job(&ok, &config), Ok("playwright".to_string()));

        let mut puppeteer = job_request(vec![goto_step("https://example.com")]);
        puppeteer.engine = Some("Puppeteer".to_string());
        assert_eq!(validate_job(&puppeteer, &config), Ok("puppeteer".to_string()));

        // An unknown engine is coerced to the default rather than rejected.
        let mut unknown = job_request(vec![goto_step("https://example.com")]);
        unknown.engine = Some("webkit".to_string());
        assert_eq!(validate_job(&unknown, &config), Ok("playwright".to_string()));
    }

    #[test]
    fn validate_job_rejects_malformed_steps() {
        let config = base_config();
        assert_eq!(
            validate_job(&job_request(vec![json!("not-an-object")]), &config),
            Err("step 0 is not an object".to_string())
        );
        assert_eq!(
            validate_job(&job_request(vec![json!({ "selector": "h1" })]), &config),
            Err("step 0 is missing a string \"action\"".to_string())
        );
        assert_eq!(
            validate_job(&job_request(vec![json!({ "action": "teleport" })]), &config),
            Err("step 0 has unknown action \"teleport\"".to_string())
        );
    }

    #[test]
    fn validate_job_gates_evaluate_on_allow_evaluate() {
        let mut config = base_config();
        let req = job_request(vec![json!({ "action": "evaluate", "script": "1+1" })]);
        config.allow_evaluate = false;
        assert!(validate_job(&req, &config).is_err(), "evaluate blocked by default");
        config.allow_evaluate = true;
        assert_eq!(
            validate_job(&req, &config),
            Ok("playwright".to_string()),
            "evaluate allowed once opted in"
        );
    }

    #[test]
    fn validate_job_requires_selector_for_selector_actions() {
        let config = base_config();
        for action in ["click", "fill", "select", "waitForSelector", "extractText", "extractAttribute"] {
            let missing = job_request(vec![json!({ "action": action })]);
            let err = validate_job(&missing, &config).unwrap_err();
            assert!(err.contains("requires a non-empty \"selector\""), "action {action}: {err}");

            let blank = job_request(vec![json!({ "action": action, "selector": "   " })]);
            assert!(validate_job(&blank, &config).is_err(), "blank selector for {action} is rejected");
        }
    }

    #[test]
    fn validate_job_requires_url_for_navigation_actions() {
        let config = base_config();
        for action in ["goto", "waitForUrl"] {
            let req = job_request(vec![json!({ "action": action })]);
            let err = validate_job(&req, &config).unwrap_err();
            assert!(err.contains("requires a non-empty \"url\""), "action {action}: {err}");
        }
    }

    #[test]
    fn validate_job_accepts_a_well_formed_scenario() {
        let config = base_config();
        let req = job_request(vec![
            json!({ "action": "goto", "url": "https://example.com" }),
            json!({ "action": "extractText", "selector": "h1", "name": "heading" }),
            json!({ "action": "screenshot", "name": "shot" }),
            json!({ "action": "waitForTimeout", "ms": 100 }),
        ]);
        assert_eq!(validate_job(&req, &config), Ok("playwright".to_string()));
    }

    // ---- build_job_spec ---------------------------------------------------

    #[test]
    fn build_job_spec_shapes_the_worker_payload() {
        let mut req = job_request(vec![json!({ "action": "goto", "url": "https://example.com" })]);
        req.request_id = Some("req-1".to_string());
        req.url = Some("https://example.com".to_string());
        req.timeout_ms = Some(1234);
        req.viewport = Some(json!({ "width": 800, "height": 600 }));
        req.user_agent = Some("UA/1".to_string());
        req.extra_headers = Some(json!({ "x-test": "1" }));
        req.capture_final_screenshot = Some(true);
        req.fail_on_console_error = Some(false);

        let spec = build_job_spec(&req, "playwright", "job-abc", 5000);
        assert_eq!(spec["jobId"], json!("job-abc"));
        assert_eq!(spec["requestId"], json!("req-1"));
        assert_eq!(spec["engine"], json!("playwright"));
        assert_eq!(spec["url"], json!("https://example.com"));
        assert_eq!(spec["steps"], json!([{ "action": "goto", "url": "https://example.com" }]));
        assert_eq!(spec["timeoutMs"], json!(1234));
        assert_eq!(spec["viewport"], json!({ "width": 800, "height": 600 }));
        assert_eq!(spec["userAgent"], json!("UA/1"));
        assert_eq!(spec["extraHeaders"], json!({ "x-test": "1" }));
        assert_eq!(spec["captureFinalScreenshot"], json!(true));
        assert_eq!(spec["failOnConsoleError"], json!(false));
        assert_eq!(spec["maxMs"], json!(5000));
    }

    #[test]
    fn build_job_spec_serializes_absent_optionals_as_null() {
        let req = job_request(vec![goto_step("https://x")]);
        let spec = build_job_spec(&req, "puppeteer", "job-2", 1000);
        assert_eq!(spec["requestId"], Value::Null);
        assert_eq!(spec["url"], Value::Null);
        assert_eq!(spec["timeoutMs"], Value::Null);
        assert_eq!(spec["viewport"], Value::Null);
        assert_eq!(spec["userAgent"], Value::Null);
        assert_eq!(spec["engine"], json!("puppeteer"));
        assert_eq!(spec["maxMs"], json!(1000));
    }

    // ---- JobRequest deserialization --------------------------------------

    #[test]
    fn job_request_deserializes_camel_case() {
        let req: JobRequest = serde_json::from_value(json!({
            "requestId": "r-9",
            "engine": "puppeteer",
            "url": "https://example.com",
            "steps": [{ "action": "goto", "url": "https://example.com" }],
            "timeoutMs": 4321,
            "viewport": { "width": 1280, "height": 800 },
            "userAgent": "UA",
            "extraHeaders": { "a": "b" },
            "captureFinalScreenshot": true,
            "failOnConsoleError": false
        }))
        .expect("valid body should deserialize");
        assert_eq!(req.request_id.as_deref(), Some("r-9"));
        assert_eq!(req.engine.as_deref(), Some("puppeteer"));
        assert_eq!(req.url.as_deref(), Some("https://example.com"));
        assert_eq!(req.steps.len(), 1);
        assert_eq!(req.timeout_ms, Some(4321));
        assert_eq!(req.user_agent.as_deref(), Some("UA"));
        assert_eq!(req.capture_final_screenshot, Some(true));
        assert_eq!(req.fail_on_console_error, Some(false));
        assert!(req.viewport.is_some());
        assert!(req.extra_headers.is_some());
    }

    #[test]
    fn job_request_defaults_missing_optionals_and_steps() {
        // steps carries #[serde(default)], everything else is Option, so an empty
        // object is a valid parse (validation, not parsing, rejects zero steps).
        let req: JobRequest = serde_json::from_value(json!({})).expect("empty object deserializes");
        assert!(req.request_id.is_none());
        assert!(req.engine.is_none());
        assert!(req.url.is_none());
        assert!(req.steps.is_empty(), "steps defaults to an empty vec");
        assert!(req.timeout_ms.is_none());
        assert!(req.viewport.is_none());
        assert!(req.capture_final_screenshot.is_none());
    }

    #[test]
    fn job_request_ignores_snake_case_keys() {
        // rename_all = "camelCase" plus serde's default of ignoring unknown fields
        // means snake_case keys silently do NOT populate their camelCase fields.
        let req: JobRequest = serde_json::from_value(json!({
            "request_id": "r",
            "timeout_ms": 10,
            "steps": []
        }))
        .expect("unknown keys are ignored");
        assert!(req.request_id.is_none(), "snake_case request_id is not mapped");
        assert!(req.timeout_ms.is_none(), "snake_case timeout_ms is not mapped");
    }

    // ---- failed_result_value ---------------------------------------------

    #[test]
    fn failed_result_value_shapes_a_failure_envelope() {
        let tracked = sample_tracked();
        let v = failed_result_value(&tracked, "boom");
        assert_eq!(v["ok"], json!(false));
        assert_eq!(v["jobId"], json!("job-xyz"));
        assert_eq!(v["engine"], json!("playwright"));
        assert_eq!(v["durationMs"], json!(0));
        assert_eq!(v["startedAt"], json!(111));
        assert_eq!(v["error"], json!("boom"));
        assert_eq!(v["steps"], json!([]));
        assert_eq!(v["extracted"], json!({}));
        assert_eq!(v["screenshots"], json!([]));
        assert_eq!(v["consoleEntries"], json!([]));
        assert_eq!(v["pageErrors"], json!([]));
    }

    // ---- descriptors / metrics -------------------------------------------

    #[test]
    fn service_descriptor_reflects_config() {
        let d = service_descriptor(&app_state(base_config()));
        assert_eq!(d["service"], json!("dd-browser-job-runner"));
        assert_eq!(d["defaultEngine"], json!("playwright"));
        assert_eq!(d["engines"], json!(["playwright", "puppeteer"]));
        assert_eq!(d["pool"]["enabled"], json!(true));
        assert_eq!(d["pool"]["slug"], json!("browser-jobs"));
        assert_eq!(d["pool"]["subject"], json!("dd.remote.container_pool.browser-jobs.requests"));
        assert_eq!(d["resultSubjectPrefix"], json!("dd.remote.browser_jobs"));
        assert_eq!(d["resultFanoutSubject"], json!("dd.remote.browser_jobs.results"));
    }

    #[test]
    fn render_metrics_emits_prometheus_lines() {
        let state = app_state(base_config());
        state.metrics.spawned_total.store(3, Ordering::Relaxed);
        state.metrics.pool_dispatched_total.store(5, Ordering::Relaxed);
        let out = render_metrics(&state, 2);
        assert!(out.contains("browser_job_in_flight 2"), "gauge reflects in_flight arg");
        assert!(out.contains("browser_job_spawned_total 3"));
        assert!(out.contains("browser_job_pool_dispatched_total 5"));
        assert!(out.contains("# TYPE browser_job_pool_failures_total counter"));
        assert!(out.ends_with('\n'), "output ends with a trailing newline");
    }

    // ---- handle_run integration (subject construction, auth, validation) --
    //
    // These exercise the ONLY place per-job subjects are built (inline in
    // handle_run). POST /run is async: it returns 202 with the subjects, then
    // spawns the job in the background. pool_enabled=false plus a nonexistent
    // nerdctl binary makes that detached task fail immediately and harmlessly;
    // no assertion depends on it.

    #[tokio::test]
    async fn handle_run_accepts_and_constructs_exact_subjects() {
        let mut config = base_config();
        config.allow_unauthenticated = true;
        config.pool_enabled = false;
        config.nerdctl_bin = "/nonexistent/dd-nerdctl-for-tests".to_string();
        let state = app_state(config);

        let req = job_request(vec![json!({ "action": "goto", "url": "https://example.com" })]);
        let resp = handle_run(State(state), HeaderMap::new(), Json(req)).await.into_response();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(body["ok"], json!(true));
        assert_eq!(body["status"], json!("accepted"));
        assert_eq!(body["engine"], json!("playwright"));

        let job_id = body["jobId"].as_str().expect("jobId present").to_string();
        assert!(!job_id.is_empty());
        // Exact per-job subjects for the returned jobId (prefix "dd.remote.browser_jobs").
        assert_eq!(body["resultSubject"], json!(format!("dd.remote.browser_jobs.{job_id}.result")));
        assert_eq!(body["eventsSubject"], json!(format!("dd.remote.browser_jobs.{job_id}.events")));
        assert_eq!(body["resultFanoutSubject"], json!("dd.remote.browser_jobs.results"));
        assert_eq!(body["poolSubject"], json!("dd.remote.container_pool.browser-jobs.requests"));
        // max_ms = clamp(default_timeout 60000, [1000, 540000]).min(540*1000) = 60000.
        assert_eq!(body["maxMs"], json!(60_000));
        assert!(body["deadlineMs"].as_u64().unwrap() > 0);
    }

    #[tokio::test]
    async fn handle_run_rejects_unauthorized_requests() {
        let mut config = base_config();
        config.allow_unauthenticated = false;
        config.server_auth_secret = Some("s".to_string());
        config.pool_enabled = false;
        let state = app_state(config);

        let req = job_request(vec![goto_step("https://example.com")]);
        let resp = handle_run(State(state), HeaderMap::new(), Json(req)).await.into_response();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["ok"], json!(false));
        assert_eq!(body["error"], json!("unauthorized"));
    }

    #[tokio::test]
    async fn handle_run_rejects_invalid_job_with_400() {
        let mut config = base_config();
        config.allow_unauthenticated = true;
        config.pool_enabled = false;
        let state = app_state(config);

        let req = job_request(vec![]); // no steps
        let resp = handle_run(State(state), HeaderMap::new(), Json(req)).await.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["ok"], json!(false));
        assert_eq!(body["error"], json!("steps_required"));
    }
}
