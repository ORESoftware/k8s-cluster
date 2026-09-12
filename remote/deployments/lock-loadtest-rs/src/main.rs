//! Mutex-broker load tester (Rust).
//!
//! Drives a configurable acquire/release workload against any broker
//! that speaks the live-mutex NDJSON TCP protocol — including the npm
//! `dd-live-mutex`, the submodule fork `dd-live-mutex-submodule`, and
//! the Rust port `dd-rust-network-mutex`. Exposes an HTTP trigger API
//! so an operator can run head-to-head benchmarks from inside the
//! cluster without having to redeploy the tester pod between runs.
//!
//! HTTP surface:
//!   POST /runs              start a run; body picks broker + workload
//!   GET  /runs/active       mid-run snapshot or `null`
//!   GET  /runs/last         last completed run summary or `null`
//!   GET  /healthz           always 200; returned by liveness probe
//!   GET  /metrics           Prometheus exposition (counters + last-run gauges)
//!
//! Run summary intentionally mirrors the Node tester's shape
//! (`remote/deployments/live-mutex-loadtest-node/src/main.js`) so a
//! shared dashboard can plot both side-by-side.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Json;
use axum::Router;
use hdrhistogram::Histogram;
use live_mutex_client::{Client, LockOpts};
use parking_lot::Mutex;
use prometheus_client::encoding::text::encode;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::registry::Registry;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Run config + state
// ---------------------------------------------------------------------------

/// Operator-supplied run config. All fields are optional; defaults are
/// chosen to give a meaningful 60s benchmark even with an empty body.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunRequest {
    /// Broker hostname. Defaults to `BROKER_HOST` env, then
    /// `dd-rust-network-mutex.default.svc.cluster.local`.
    #[serde(default)]
    broker_host: Option<String>,
    /// Broker TCP port. Defaults to `BROKER_PORT` env, then `6970`.
    #[serde(default)]
    broker_port: Option<u16>,
    /// Wall-clock duration of the run. Capped at 600s.
    #[serde(default)]
    duration_seconds: Option<u64>,
    /// Number of concurrent worker tasks (and therefore TCP
    /// connections to the broker). Capped at 1024.
    #[serde(default)]
    workers: Option<usize>,
    /// Cardinality of the keyspace the workers cycle through. `1`
    /// gives a single-key contention storm; high values give a wide,
    /// uncontended sweep.
    #[serde(default)]
    keys: Option<usize>,
    /// Per-broker target acquire rate. We translate this into a
    /// per-worker think-time. `0` or unset = "as fast as possible".
    #[serde(default)]
    target_rps: Option<u64>,
    /// Lock TTL hint passed to the broker (`ttl` field). `None`
    /// = broker default. We always use much shorter TTLs than the
    /// run duration so the centralised TTL sweeper actually fires
    /// during the run if a worker goes wedged.
    #[serde(default)]
    ttl_ms: Option<u64>,
    /// `max` (semaphore) field per acquire. `None` = mutex (`1`).
    /// `Some(n)` exercises the semaphore code path; `Some(0)` is
    /// rejected by the broker, useful for negative testing.
    #[serde(default)]
    semaphore_max: Option<u32>,
    /// If `true`, every worker periodically issues an `acquire-many`
    /// over a 3-key subset of the keyspace and validates fencing
    /// tokens for *all* of them. Stresses the multi-key code paths.
    #[serde(default)]
    use_acquire_many: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunSummary {
    run_id: String,
    broker_host: String,
    broker_port: u16,
    started_at_ms: u64,
    finished_at_ms: u64,
    duration_seconds: u64,
    workers: usize,
    keys: usize,
    target_rps: u64,
    ttl_ms: Option<u64>,
    semaphore_max: Option<u32>,
    use_acquire_many: bool,
    // Aggregated counters across all workers
    acquired: u64,
    released: u64,
    failed_acquires: u64,
    failed_releases: u64,
    fencing_violations: u64,
    // Latency in microseconds — broker round-trip for `acquire`.
    // We report micros (not millis) because a fast in-cluster grant
    // is sub-millisecond and millis-rounded values lose all signal.
    acquire_latency_us_p50: u64,
    acquire_latency_us_p95: u64,
    acquire_latency_us_p99: u64,
    acquire_latency_us_max: u64,
    actual_rps: f64,
}

/// Live (mid-run) snapshot. Cheaper than `RunSummary` because we
/// don't compute percentiles until the run finishes.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LiveSnapshot {
    run_id: String,
    elapsed_seconds: f64,
    acquired: u64,
    released: u64,
    failed_acquires: u64,
    failed_releases: u64,
    actual_rps: f64,
}

#[derive(Default)]
struct RunCounters {
    acquired: AtomicU64,
    released: AtomicU64,
    failed_acquires: AtomicU64,
    failed_releases: AtomicU64,
    fencing_violations: AtomicU64,
}

/// Active run handle. The Axum HTTP layer holds an `Option<ActiveRun>`
/// behind a `Mutex` so it can answer GET /runs/active without coupling
/// to the async runtime. `started_at_ms`, `config`, and `cancel` are
/// only read by the spawned driver task via independent clones today,
/// but we keep them on the struct so a future DELETE /runs endpoint
/// can introspect/cancel the active run without re-plumbing.
#[allow(dead_code)]
struct ActiveRun {
    run_id: String,
    started_at: Instant,
    started_at_ms: u64,
    config: RunRequest,
    counters: Arc<RunCounters>,
    cancel: Arc<AtomicBool>,
}

#[derive(Clone)]
struct AppState {
    /// At most one run can be in-flight; subsequent requests get a 409.
    active: Arc<Mutex<Option<ActiveRun>>>,
    last: Arc<Mutex<Option<RunSummary>>>,
    metrics: Arc<MetricsRegistry>,
    /// Default broker host, used when the request body omits it.
    default_host: String,
    default_port: u16,
}

// ---------------------------------------------------------------------------
// Prometheus surface
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Hash, PartialEq, Eq, prometheus_client::encoding::EncodeLabelSet)]
struct BrokerLabel {
    broker: String,
}

struct MetricsRegistry {
    registry: Mutex<Registry>,
    runs_started: Counter,
    runs_completed: Counter,
    runs_failed: Counter,
    last_acquired: Family<BrokerLabel, Gauge>,
    last_failed: Family<BrokerLabel, Gauge>,
    last_p50_us: Family<BrokerLabel, Gauge>,
    last_p95_us: Family<BrokerLabel, Gauge>,
    last_p99_us: Family<BrokerLabel, Gauge>,
    last_max_us: Family<BrokerLabel, Gauge>,
    last_rps: Family<BrokerLabel, Gauge<f64, std::sync::atomic::AtomicU64>>,
    last_fencing_violations: Family<BrokerLabel, Gauge>,
}

impl MetricsRegistry {
    fn new() -> Self {
        let mut registry = Registry::default();

        let runs_started = Counter::default();
        let runs_completed = Counter::default();
        let runs_failed = Counter::default();
        let last_acquired = Family::<BrokerLabel, Gauge>::default();
        let last_failed = Family::<BrokerLabel, Gauge>::default();
        let last_p50_us = Family::<BrokerLabel, Gauge>::default();
        let last_p95_us = Family::<BrokerLabel, Gauge>::default();
        let last_p99_us = Family::<BrokerLabel, Gauge>::default();
        let last_max_us = Family::<BrokerLabel, Gauge>::default();
        let last_rps =
            Family::<BrokerLabel, Gauge<f64, std::sync::atomic::AtomicU64>>::default();
        let last_fencing_violations = Family::<BrokerLabel, Gauge>::default();

        registry.register(
            "lock_loadtest_runs_started_total",
            "Number of /runs invocations accepted by the trigger",
            runs_started.clone(),
        );
        registry.register(
            "lock_loadtest_runs_completed_total",
            "Number of runs that finished without crashing",
            runs_completed.clone(),
        );
        registry.register(
            "lock_loadtest_runs_failed_total",
            "Runs that aborted before completion (bind error, broker unreachable, etc)",
            runs_failed.clone(),
        );
        registry.register(
            "lock_loadtest_last_acquired",
            "Acquired count from the most recent run, labeled by broker",
            last_acquired.clone(),
        );
        registry.register(
            "lock_loadtest_last_failed",
            "Failed acquire count from the most recent run, labeled by broker",
            last_failed.clone(),
        );
        registry.register(
            "lock_loadtest_acquire_latency_us_p50",
            "p50 acquire round-trip latency from the most recent run",
            last_p50_us.clone(),
        );
        registry.register(
            "lock_loadtest_acquire_latency_us_p95",
            "p95 acquire round-trip latency from the most recent run",
            last_p95_us.clone(),
        );
        registry.register(
            "lock_loadtest_acquire_latency_us_p99",
            "p99 acquire round-trip latency from the most recent run",
            last_p99_us.clone(),
        );
        registry.register(
            "lock_loadtest_acquire_latency_us_max",
            "Max acquire round-trip latency from the most recent run",
            last_max_us.clone(),
        );
        registry.register(
            "lock_loadtest_last_rps",
            "Acquire+release throughput from the most recent run",
            last_rps.clone(),
        );
        registry.register(
            "lock_loadtest_last_fencing_violations",
            "Fencing-token monotonicity violations observed during the most recent run. Should be 0.",
            last_fencing_violations.clone(),
        );

        Self {
            registry: Mutex::new(registry),
            runs_started,
            runs_completed,
            runs_failed,
            last_acquired,
            last_failed,
            last_p50_us,
            last_p95_us,
            last_p99_us,
            last_max_us,
            last_rps,
            last_fencing_violations,
        }
    }

    fn record(&self, summary: &RunSummary) {
        // Use the broker hostname (without the cluster suffix) as the
        // label so dashboards stay readable. Prometheus best practice
        // discourages high-cardinality labels, but the broker count is
        // O(1) — only the three deployed brokers.
        let label = BrokerLabel {
            broker: shorten_broker(&summary.broker_host, summary.broker_port),
        };
        self.last_acquired.get_or_create(&label).set(summary.acquired as i64);
        self.last_failed.get_or_create(&label).set(summary.failed_acquires as i64);
        self.last_p50_us.get_or_create(&label).set(summary.acquire_latency_us_p50 as i64);
        self.last_p95_us.get_or_create(&label).set(summary.acquire_latency_us_p95 as i64);
        self.last_p99_us.get_or_create(&label).set(summary.acquire_latency_us_p99 as i64);
        self.last_max_us.get_or_create(&label).set(summary.acquire_latency_us_max as i64);
        self.last_rps.get_or_create(&label).set(summary.actual_rps);
        self.last_fencing_violations
            .get_or_create(&label)
            .set(summary.fencing_violations as i64);
    }
}

fn shorten_broker(host: &str, port: u16) -> String {
    // `dd-foo.default.svc.cluster.local` -> `dd-foo:6970`
    let head = host.split('.').next().unwrap_or(host);
    format!("{head}:{port}")
}

// ---------------------------------------------------------------------------
// HTTP handlers
// ---------------------------------------------------------------------------

async fn handle_healthz() -> &'static str {
    "ok\n"
}

async fn handle_metrics(State(state): State<AppState>) -> impl IntoResponse {
    let mut buf = String::new();
    {
        let registry = state.metrics.registry.lock();
        if let Err(e) = encode(&mut buf, &registry) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                [("content-type", "text/plain")],
                format!("encode failed: {e}"),
            );
        }
    }
    (
        StatusCode::OK,
        [("content-type", "application/openmetrics-text; version=1.0.0; charset=utf-8")],
        buf,
    )
}

async fn handle_runs_active(State(state): State<AppState>) -> Json<Option<LiveSnapshot>> {
    let snapshot = {
        let active = state.active.lock();
        active.as_ref().map(|run| {
            let elapsed = run.started_at.elapsed().as_secs_f64();
            let acquired = run.counters.acquired.load(Ordering::Relaxed);
            let released = run.counters.released.load(Ordering::Relaxed);
            let failed_acquires = run.counters.failed_acquires.load(Ordering::Relaxed);
            let failed_releases = run.counters.failed_releases.load(Ordering::Relaxed);
            LiveSnapshot {
                run_id: run.run_id.clone(),
                elapsed_seconds: elapsed,
                acquired,
                released,
                failed_acquires,
                failed_releases,
                actual_rps: if elapsed > 0.0 { acquired as f64 / elapsed } else { 0.0 },
            }
        })
    };
    Json(snapshot)
}

async fn handle_runs_last(State(state): State<AppState>) -> Json<Option<RunSummary>> {
    Json(state.last.lock().clone())
}

async fn handle_runs_post(
    State(state): State<AppState>,
    Json(req): Json<RunRequest>,
) -> impl IntoResponse {
    // Only one run at a time. Returning 409 (vs 429) signals to the
    // operator "ask again after the current run finishes", which is
    // what the Node trigger does.
    {
        let active = state.active.lock();
        if active.is_some() {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({"error": "a run is already in progress"})),
            );
        }
    }

    let config = clamp_config(&req, &state);
    let run_id = Uuid::new_v4().to_string();
    let counters = Arc::new(RunCounters::default());
    let cancel = Arc::new(AtomicBool::new(false));
    let started_at = Instant::now();
    let started_at_ms = unix_now_ms();

    {
        let mut active = state.active.lock();
        *active = Some(ActiveRun {
            run_id: run_id.clone(),
            started_at,
            started_at_ms,
            config: config.clone(),
            counters: counters.clone(),
            cancel: cancel.clone(),
        });
    }

    state.metrics.runs_started.inc();

    // Drive the run on a fresh task so the HTTP handler returns
    // immediately. The `tokio::main` runtime owns the spawned task;
    // it lives until the run finishes or the pod terminates.
    let state_clone = state.clone();
    let config_clone = config.clone();
    let counters_clone = counters.clone();
    let cancel_clone = cancel.clone();
    let run_id_clone = run_id.clone();
    tokio::spawn(async move {
        let result = drive_run(
            run_id_clone.clone(),
            started_at,
            started_at_ms,
            config_clone,
            counters_clone,
            cancel_clone,
        )
        .await;
        match result {
            Ok(summary) => {
                state_clone.metrics.runs_completed.inc();
                state_clone.metrics.record(&summary);
                let mut last = state_clone.last.lock();
                *last = Some(summary);
            }
            Err(e) => {
                state_clone.metrics.runs_failed.inc();
                tracing::error!(run_id = %run_id_clone, error = %e, "run failed");
            }
        }
        let mut active = state_clone.active.lock();
        *active = None;
    });

    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "runId": run_id,
            "brokerHost": config.broker_host.unwrap_or_default(),
            "brokerPort": config.broker_port.unwrap_or(0),
            "durationSeconds": config.duration_seconds.unwrap_or(0),
            "workers": config.workers.unwrap_or(0),
        })),
    )
}

fn clamp_config(req: &RunRequest, state: &AppState) -> RunRequest {
    // `Some` after this clamping pass is a strong invariant — every
    // worker task can `.unwrap()` without checking again.
    let host = req.broker_host.clone().unwrap_or_else(|| state.default_host.clone());
    let port = req.broker_port.unwrap_or(state.default_port);
    let duration_seconds = req.duration_seconds.unwrap_or(60).clamp(1, 600);
    let workers = req.workers.unwrap_or(16).clamp(1, 1024);
    let keys = req.keys.unwrap_or(64).clamp(1, 65_536);
    RunRequest {
        broker_host: Some(host),
        broker_port: Some(port),
        duration_seconds: Some(duration_seconds),
        workers: Some(workers),
        keys: Some(keys),
        target_rps: Some(req.target_rps.unwrap_or(0)),
        ttl_ms: req.ttl_ms.or(Some(4_000)),
        semaphore_max: req.semaphore_max,
        use_acquire_many: req.use_acquire_many,
    }
}

// ---------------------------------------------------------------------------
// Run driver
// ---------------------------------------------------------------------------

async fn drive_run(
    run_id: String,
    started_at: Instant,
    started_at_ms: u64,
    config: RunRequest,
    counters: Arc<RunCounters>,
    cancel: Arc<AtomicBool>,
) -> anyhow::Result<RunSummary> {
    let host = config.broker_host.clone().expect("clamped above");
    let port = config.broker_port.expect("clamped above");
    let duration = Duration::from_secs(config.duration_seconds.expect("clamped above"));
    let workers = config.workers.expect("clamped above");
    let keys = config.keys.expect("clamped above");
    let target_rps = config.target_rps.unwrap_or(0);
    let ttl_ms = config.ttl_ms;
    let semaphore_max = config.semaphore_max;
    let use_acquire_many = config.use_acquire_many.unwrap_or(false);

    tracing::info!(
        run_id = %run_id, host = %host, port, workers, keys, ?duration,
        "starting run"
    );

    // Per-key fencing-token monotonicity tracker. A `parking_lot`
    // mutex is fine because workers contend on it briefly per acquire
    // (single integer compare + insert).
    let fencing_high_water: Arc<Mutex<HashMap<String, u64>>> = Arc::new(Mutex::new(HashMap::new()));

    // Histograms-per-worker, merged at the end. Avoids a global lock
    // on every acquire; merging O(workers) histograms once is cheap.
    let mut handles = Vec::with_capacity(workers);
    for worker_id in 0..workers {
        let host = host.clone();
        let counters = counters.clone();
        let cancel = cancel.clone();
        let fencing_high_water = fencing_high_water.clone();
        let handle = tokio::spawn(async move {
            run_worker(
                worker_id, host, port, duration, keys, target_rps, workers,
                ttl_ms, semaphore_max, use_acquire_many,
                counters, cancel, fencing_high_water,
            )
            .await
        });
        handles.push(handle);
    }

    // Stop after `duration` regardless of progress.
    tokio::time::sleep(duration).await;
    cancel.store(true, Ordering::Relaxed);

    // Merge per-worker histograms.
    let mut merged: Histogram<u64> = Histogram::new_with_bounds(1, 60_000_000, 3)
        .map_err(|e| anyhow::anyhow!("hdr: {e}"))?;
    for h in handles {
        match h.await {
            Ok(Ok(local)) => {
                merged.add(local).map_err(|e| anyhow::anyhow!("hdr add: {e:?}"))?;
            }
            Ok(Err(e)) => tracing::warn!("worker error (non-fatal): {e}"),
            Err(e) => tracing::warn!("worker join error (non-fatal): {e}"),
        }
    }

    let elapsed = started_at.elapsed();
    let acquired = counters.acquired.load(Ordering::Relaxed);
    let released = counters.released.load(Ordering::Relaxed);
    let failed_acquires = counters.failed_acquires.load(Ordering::Relaxed);
    let failed_releases = counters.failed_releases.load(Ordering::Relaxed);
    let fencing_violations = counters.fencing_violations.load(Ordering::Relaxed);
    let actual_rps = if elapsed.as_secs_f64() > 0.0 {
        acquired as f64 / elapsed.as_secs_f64()
    } else {
        0.0
    };

    Ok(RunSummary {
        run_id,
        broker_host: host,
        broker_port: port,
        started_at_ms,
        finished_at_ms: unix_now_ms(),
        duration_seconds: duration.as_secs(),
        workers,
        keys,
        target_rps,
        ttl_ms,
        semaphore_max,
        use_acquire_many,
        acquired,
        released,
        failed_acquires,
        failed_releases,
        fencing_violations,
        acquire_latency_us_p50: merged.value_at_quantile(0.50),
        acquire_latency_us_p95: merged.value_at_quantile(0.95),
        acquire_latency_us_p99: merged.value_at_quantile(0.99),
        acquire_latency_us_max: merged.max(),
        actual_rps,
    })
}

#[allow(clippy::too_many_arguments)]
async fn run_worker(
    worker_id: usize,
    host: String,
    port: u16,
    duration: Duration,
    keys: usize,
    target_rps: u64,
    workers: usize,
    ttl_ms: Option<u64>,
    semaphore_max: Option<u32>,
    use_acquire_many: bool,
    counters: Arc<RunCounters>,
    cancel: Arc<AtomicBool>,
    fencing_high_water: Arc<Mutex<HashMap<String, u64>>>,
) -> anyhow::Result<Histogram<u64>> {
    let addr = format!("{host}:{port}");
    let client = Client::connect_with_timeout(&addr, Duration::from_secs(15)).await?;
    // Per-worker histogram avoids a hot lock at high RPS. We merge
    // them in `drive_run` once cancel is set.
    let mut hist: Histogram<u64> = Histogram::new_with_bounds(1, 60_000_000, 3)
        .map_err(|e| anyhow::anyhow!("hdr: {e}"))?;

    // Per-worker target inter-acquire delay. Total target_rps is split
    // evenly across workers; rounding bias is harmless.
    let per_worker_delay = if target_rps > 0 {
        Duration::from_secs_f64(workers as f64 / target_rps as f64)
    } else {
        Duration::ZERO
    };

    let deadline = Instant::now() + duration;
    let mut iter: u64 = 0;
    while !cancel.load(Ordering::Relaxed) && Instant::now() < deadline {
        let key_id = (worker_id as u64).wrapping_add(iter) as usize % keys;
        let key = format!("loadtest-key-{key_id:06}");

        if use_acquire_many && iter % 16 == 0 {
            // Every 16th iteration: stress acquire-many over a 3-key
            // window, validating fencing tokens for all keys.
            let k0 = format!("loadtest-key-{:06}", key_id);
            let k1 = format!("loadtest-key-{:06}", (key_id + 1) % keys);
            let k2 = format!("loadtest-key-{:06}", (key_id + 2) % keys);
            let keys_slice = [k0.as_str(), k1.as_str(), k2.as_str()];
            let started = Instant::now();
            match client.acquire_many(&keys_slice, ttl_ms).await {
                Ok(grant) => {
                    let micros = started.elapsed().as_micros() as u64;
                    let _ = hist.record(micros.max(1));
                    counters.acquired.fetch_add(1, Ordering::Relaxed);
                    check_fencing(&fencing_high_water, &grant.fencing_tokens, &counters);
                    if let Err(e) = client.release_many(&grant.lock_uuid).await {
                        tracing::debug!("release_many: {e}");
                        counters.failed_releases.fetch_add(1, Ordering::Relaxed);
                    } else {
                        counters.released.fetch_add(1, Ordering::Relaxed);
                    }
                }
                Err(e) => {
                    tracing::debug!("acquire_many: {e}");
                    counters.failed_acquires.fetch_add(1, Ordering::Relaxed);
                }
            }
        } else {
            let opts = LockOpts { ttl_ms, max: semaphore_max };
            let started = Instant::now();
            match client.acquire(&key, Some(opts)).await {
                Ok(grant) => {
                    let micros = started.elapsed().as_micros() as u64;
                    let _ = hist.record(micros.max(1));
                    counters.acquired.fetch_add(1, Ordering::Relaxed);
                    if let Some(token) = grant.fencing_token {
                        let mut tokens = HashMap::new();
                        tokens.insert(key.clone(), token);
                        check_fencing(&fencing_high_water, &tokens, &counters);
                    }
                    if let Err(e) = client.release(&key, &grant.lock_uuid, false).await {
                        tracing::debug!("release: {e}");
                        counters.failed_releases.fetch_add(1, Ordering::Relaxed);
                    } else {
                        counters.released.fetch_add(1, Ordering::Relaxed);
                    }
                }
                Err(e) => {
                    tracing::debug!("acquire: {e}");
                    counters.failed_acquires.fetch_add(1, Ordering::Relaxed);
                }
            }
        }

        iter = iter.wrapping_add(1);
        if !per_worker_delay.is_zero() {
            tokio::time::sleep(per_worker_delay).await;
        }
    }

    Ok(hist)
}

/// Validates per-key strict monotonicity. A regression here is a
/// real correctness bug in either the broker or the wire protocol;
/// dashboards should alert on `lock_loadtest_last_fencing_violations`.
fn check_fencing(
    high_water: &Arc<Mutex<HashMap<String, u64>>>,
    observed: &HashMap<String, u64>,
    counters: &RunCounters,
) {
    let mut high = high_water.lock();
    for (key, token) in observed {
        let prev = high.get(key).copied().unwrap_or(0);
        if *token <= prev {
            counters.fencing_violations.fetch_add(1, Ordering::Relaxed);
        }
        if *token > prev {
            high.insert(key.clone(), *token);
        }
    }
}

fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Bootstrap
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .compact()
        .init();

    let bind: SocketAddr = std::env::var("HTTP_BIND")
        .unwrap_or_else(|_| "0.0.0.0:8120".to_string())
        .parse()?;
    let default_host = std::env::var("BROKER_HOST")
        .unwrap_or_else(|_| "dd-rust-network-mutex.default.svc.cluster.local".to_string());
    let default_port: u16 = std::env::var("BROKER_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(6970);

    let state = AppState {
        active: Arc::new(Mutex::new(None)),
        last: Arc::new(Mutex::new(None)),
        metrics: Arc::new(MetricsRegistry::new()),
        default_host,
        default_port,
    };

    let app = Router::new()
        .route("/healthz", get(handle_healthz))
        .route("/metrics", get(handle_metrics))
        .route("/runs", post(handle_runs_post))
        .route("/runs/active", get(handle_runs_active))
        .route("/runs/last", get(handle_runs_last))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(?bind, "dd-lock-loadtest-rs listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            // Honour both SIGINT (developer Ctrl-C) and SIGTERM
            // (kubelet sending us a clean shutdown).
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
