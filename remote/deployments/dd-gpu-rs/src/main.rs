// dd-gpu-rs
//
// GPU job scheduler. "Submit work, get result."
//
// Assigns a batch of AI/ML jobs onto a fleet of GPUs over HTTP and NATS. Each
// GPU has a VRAM capacity and may run several jobs *concurrently* as long as the
// sum of the resident jobs' VRAM never exceeds that capacity — the natural model
// for AI workloads, where you pack models/inference onto a card while memory fits
// and time-share when it does not. The core is a greedy list-scheduling scheme:
// jobs are ordered by priority then longest duration (LPT), and each is placed on
// the GPU + earliest feasible start that keeps concurrent VRAM within capacity and
// yields the earliest finish (ties broken toward the GPU with more free headroom).
//
// This complements the slot-based dd-constraint-scheduler with a memory-capacity,
// concurrent-occupancy placer purpose-built for GPU fleets.

use std::{
    collections::HashMap,
    env,
    error::Error,
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{DefaultBodyLimit, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use dd_nats_subject_defs::{
    GPU_JOB_REQUESTS_QUEUE_GROUP, GPU_JOB_REQUESTS_SUBJECT, GPU_JOB_RESULTS_SUBJECT,
    RUNTIME_EVENTS_SUBJECT,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;

const MAX_HTTP_BODY_BYTES: usize = 1024 * 1024;
const MAX_NATS_PAYLOAD_BYTES: usize = 1024 * 1024;
const MAX_JOBS: usize = 2_000;
const MAX_GPUS: usize = 256;
/// Per-entity tag-list ceiling. Bounds the cost of tag-superset matching
/// (jobTags x gpuTags per job x gpu) so a tag-stuffed 1 MiB body cannot make
/// placement quadratic in tag count.
const MAX_TAGS: usize = 64;
/// VRAM ceiling (MiB). Bounds memory sums so capacity arithmetic stays in u64.
const MAX_VRAM_MIB: u64 = 10_000_000;
/// Per-job duration / release ceiling (ms). Bounds the timeline so
/// `start + duration` can never overflow u64.
const MAX_TIME_MS: u64 = 1_000_000_000;
/// GPU speed multiplier bounds (effective duration = ceil(duration / speed)).
const MIN_GPU_SPEED: f64 = 0.01;
const MAX_GPU_SPEED: f64 = 1_000.0;
const DEFAULT_MAX_INFLIGHT: usize = 16;

#[derive(Clone)]
struct AppState {
    nats: Option<async_nats::Client>,
    result_subject: String,
    event_subject: String,
    metrics: Arc<Metrics>,
    /// Bounds concurrent placements so a request/NATS flood cannot spawn
    /// unbounded CPU-heavy work.
    inflight: Arc<tokio::sync::Semaphore>,
}

#[derive(Default)]
struct Metrics {
    requests_total: AtomicU64,
    schedules_total: AtomicU64,
    jobs_scheduled_total: AtomicU64,
    jobs_rejected_total: AtomicU64,
    errors_total: AtomicU64,
    rejected_busy_total: AtomicU64,
    nats_messages_total: AtomicU64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScheduleRequest {
    request_id: Option<String>,
    kind: Option<String>,
    gpus: Vec<GpuInput>,
    jobs: Vec<JobInput>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GpuInput {
    id: String,
    memory_mib: u64,
    /// Throughput multiplier; a job's effective duration on this GPU is
    /// ceil(duration / speed). Defaults to 1.0.
    speed: Option<f64>,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JobInput {
    id: String,
    vram_mib: u64,
    duration_ms: u64,
    priority: Option<i64>,
    /// Pin the job to a specific GPU id (hard affinity).
    gpu: Option<String>,
    /// Job runs only on a GPU whose tags are a superset of these.
    #[serde(default)]
    requires_tags: Vec<String>,
    /// Earliest the job may start (ms). Defaults to 0.
    release_ms: Option<u64>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ScheduleResponse {
    ok: bool,
    request_id: String,
    kind: String,
    feasible: bool,
    makespan: u64,
    scheduled: usize,
    rejected: usize,
    total_wait: u64,
    max_wait: u64,
    assignments: Vec<JobAssignment>,
    gpu_utilization: Vec<GpuUtilization>,
    rejections: Vec<JobRejection>,
    warnings: Vec<String>,
    generated_at_ms: u128,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct JobAssignment {
    id: String,
    gpu: String,
    start: u64,
    finish: u64,
    duration: u64,
    vram_mib: u64,
    wait: u64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct GpuUtilization {
    gpu: String,
    memory_mib: u64,
    peak_vram_mib: u64,
    job_count: usize,
    span: u64,
    /// Time-integrated memory usage divided by (memory * span): how fully the
    /// card's VRAM was kept occupied over its active window.
    memory_utilization: f64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct JobRejection {
    id: String,
    reason: String,
}

fn env_value(key: &str, fallback: &str) -> String {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn env_usize(key: &str, fallback: usize) -> usize {
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|&value| value > 0)
        .unwrap_or(fallback)
}

struct Gpu {
    id: String,
    memory_mib: u64,
    speed: f64,
    tags: Vec<String>,
    /// Placed jobs as (start, finish, vram).
    intervals: Vec<(u64, u64, u64)>,
    /// Running sum of vram*duration committed to this card — maintained as jobs
    /// are placed and used as an O(1) load-balancing tie-break (cheaper than
    /// recomputing peak VRAM in the hot loop).
    committed_mem_time: u128,
}

struct Job {
    id: String,
    vram_mib: u64,
    duration_ms: u64,
    priority: i64,
    affinity: Option<String>,
    requires_tags: Vec<String>,
    release_ms: u64,
}

/// Effective duration of `duration` ms on a GPU running at `speed`, clamped to
/// at least 1 ms and the timeline ceiling.
fn effective_duration(duration: u64, speed: f64) -> u64 {
    let scaled = (duration as f64 / speed).ceil();
    let bounded = scaled.clamp(1.0, MAX_TIME_MS as f64);
    bounded as u64
}

fn schedule(request: ScheduleRequest) -> Result<ScheduleResponse, String> {
    let request_id = request
        .request_id
        .clone()
        .unwrap_or_else(|| format!("gpu-schedule-{}", now_ms()));

    if request.gpus.is_empty() {
        return Err("gpus must not be empty".to_string());
    }
    if request.gpus.len() > MAX_GPUS {
        return Err(format!("too many gpus; max {MAX_GPUS}"));
    }
    if request.jobs.is_empty() {
        return Err("jobs must not be empty".to_string());
    }
    if request.jobs.len() > MAX_JOBS {
        return Err(format!("too many jobs; max {MAX_JOBS}"));
    }

    // Build the GPU fleet, validating ids and capacities.
    let mut gpu_index: HashMap<String, usize> = HashMap::with_capacity(request.gpus.len());
    let mut gpus: Vec<Gpu> = Vec::with_capacity(request.gpus.len());
    for gpu in &request.gpus {
        if gpu.memory_mib == 0 {
            return Err(format!("gpu {} memoryMib must be >= 1", gpu.id));
        }
        if gpu.memory_mib > MAX_VRAM_MIB {
            return Err(format!("gpu {} memoryMib must be <= {MAX_VRAM_MIB}", gpu.id));
        }
        let speed = gpu.speed.unwrap_or(1.0);
        if !speed.is_finite() || !(MIN_GPU_SPEED..=MAX_GPU_SPEED).contains(&speed) {
            return Err(format!(
                "gpu {} speed must be finite in [{MIN_GPU_SPEED}, {MAX_GPU_SPEED}]",
                gpu.id
            ));
        }
        if gpu.tags.len() > MAX_TAGS {
            return Err(format!("gpu {} has too many tags; max {MAX_TAGS}", gpu.id));
        }
        if gpu_index.insert(gpu.id.clone(), gpus.len()).is_some() {
            return Err(format!("duplicate gpu id {}", gpu.id));
        }
        gpus.push(Gpu {
            id: gpu.id.clone(),
            memory_mib: gpu.memory_mib,
            speed,
            tags: gpu.tags.clone(),
            intervals: Vec::new(),
            committed_mem_time: 0,
        });
    }

    // Validate jobs.
    let mut job_ids: HashMap<&str, ()> = HashMap::with_capacity(request.jobs.len());
    let mut jobs: Vec<Job> = Vec::with_capacity(request.jobs.len());
    for job in &request.jobs {
        if job_ids.insert(job.id.as_str(), ()).is_some() {
            return Err(format!("duplicate job id {}", job.id));
        }
        if job.vram_mib == 0 {
            return Err(format!("job {} vramMib must be >= 1", job.id));
        }
        if job.vram_mib > MAX_VRAM_MIB {
            return Err(format!("job {} vramMib must be <= {MAX_VRAM_MIB}", job.id));
        }
        if job.duration_ms == 0 {
            return Err(format!("job {} durationMs must be >= 1", job.id));
        }
        if job.duration_ms > MAX_TIME_MS {
            return Err(format!("job {} durationMs must be <= {MAX_TIME_MS}", job.id));
        }
        if job.release_ms.unwrap_or(0) > MAX_TIME_MS {
            return Err(format!("job {} releaseMs must be <= {MAX_TIME_MS}", job.id));
        }
        if job.requires_tags.len() > MAX_TAGS {
            return Err(format!("job {} has too many requiresTags; max {MAX_TAGS}", job.id));
        }
        if let Some(affinity) = &job.gpu {
            if !gpu_index.contains_key(affinity) {
                return Err(format!("job {} pins unknown gpu {}", job.id, affinity));
            }
        }
        jobs.push(Job {
            id: job.id.clone(),
            vram_mib: job.vram_mib,
            duration_ms: job.duration_ms,
            priority: job.priority.unwrap_or(0),
            affinity: job.gpu.clone(),
            requires_tags: job.requires_tags.clone(),
            release_ms: job.release_ms.unwrap_or(0),
        });
    }

    // Schedule highest priority first, then longest duration (LPT packs better),
    // tie-break by id for determinism.
    let mut order: Vec<usize> = (0..jobs.len()).collect();
    order.sort_by(|&a, &b| {
        jobs[b]
            .priority
            .cmp(&jobs[a].priority)
            .then_with(|| jobs[b].duration_ms.cmp(&jobs[a].duration_ms))
            .then_with(|| jobs[a].id.cmp(&jobs[b].id))
    });

    let mut assignments: Vec<JobAssignment> = Vec::with_capacity(jobs.len());
    let mut rejections: Vec<JobRejection> = Vec::new();

    for &j in &order {
        let job = &jobs[j];
        // Find the eligible GPU yielding the earliest finish; tie-break toward
        // the least-loaded card (smallest committed vram*time) to spread work,
        // then by gpu id for determinism.
        let mut best: Option<(usize, u64, u64)> = None; // (gpu, start, finish)
        let mut had_eligible = false;
        for (g, gpu) in gpus.iter().enumerate() {
            if let Some(affinity) = &job.affinity {
                if &gpu.id != affinity {
                    continue;
                }
            }
            if gpu.memory_mib < job.vram_mib {
                continue;
            }
            if !job
                .requires_tags
                .iter()
                .all(|tag| gpu.tags.iter().any(|t| t == tag))
            {
                continue;
            }
            had_eligible = true;
            let duration = effective_duration(job.duration_ms, gpu.speed);
            let start = earliest_feasible_start(
                &gpu.intervals,
                job.release_ms,
                duration,
                job.vram_mib,
                gpu.memory_mib,
            );
            let finish = start + duration;
            let replace = match best {
                None => true,
                Some((bg, _, bf)) => {
                    (finish, gpu.committed_mem_time, gpu.id.as_str())
                        < (bf, gpus[bg].committed_mem_time, gpus[bg].id.as_str())
                }
            };
            if replace {
                best = Some((g, start, finish));
            }
        }

        match best {
            Some((g, start, finish)) => {
                let duration = finish - start;
                gpus[g].intervals.push((start, finish, job.vram_mib));
                gpus[g].committed_mem_time += duration as u128 * job.vram_mib as u128;
                assignments.push(JobAssignment {
                    id: job.id.clone(),
                    gpu: gpus[g].id.clone(),
                    start,
                    finish,
                    duration,
                    vram_mib: job.vram_mib,
                    wait: start - job.release_ms,
                });
            }
            None => {
                let reason = if had_eligible {
                    // Unreachable in practice: an eligible GPU always admits the
                    // job after its current load drains. Kept as a guard.
                    "no feasible placement found".to_string()
                } else if job.affinity.is_some() {
                    format!(
                        "pinned gpu cannot host job (insufficient VRAM {} MiB or tag mismatch)",
                        job.vram_mib
                    )
                } else {
                    format!(
                        "no gpu has {} MiB VRAM and the required tags {:?}",
                        job.vram_mib, job.requires_tags
                    )
                };
                rejections.push(JobRejection {
                    id: job.id.clone(),
                    reason,
                });
            }
        }
    }

    assignments.sort_by(|a, b| {
        a.start
            .cmp(&b.start)
            .then_with(|| a.gpu.cmp(&b.gpu))
            .then_with(|| a.id.cmp(&b.id))
    });
    rejections.sort_by(|a, b| a.id.cmp(&b.id));

    let makespan = assignments.iter().map(|a| a.finish).max().unwrap_or(0);
    let total_wait: u64 = assignments.iter().map(|a| a.wait).sum();
    let max_wait = assignments.iter().map(|a| a.wait).max().unwrap_or(0);

    let mut gpu_utilization: Vec<GpuUtilization> = gpus
        .iter()
        .map(|gpu| {
            let span = gpu.intervals.iter().map(|(_, f, _)| *f).max().unwrap_or(0);
            let memory_time = memory_time_integral(&gpu.intervals);
            let denom = (span.max(1) as u128) * gpu.memory_mib as u128;
            GpuUtilization {
                gpu: gpu.id.clone(),
                memory_mib: gpu.memory_mib,
                peak_vram_mib: peak_vram(&gpu.intervals),
                job_count: gpu.intervals.len(),
                span,
                memory_utilization: memory_time as f64 / denom as f64,
            }
        })
        .collect();
    gpu_utilization.sort_by(|a, b| a.gpu.cmp(&b.gpu));

    let mut warnings = Vec::new();
    if !rejections.is_empty() {
        warnings.push(format!(
            "{} job(s) could not be placed; add GPUs, raise per-GPU VRAM, or relax tag/affinity constraints",
            rejections.len()
        ));
    }

    Ok(ScheduleResponse {
        ok: true,
        request_id,
        kind: request.kind.clone().unwrap_or_else(|| "gpu.placement".to_string()),
        feasible: rejections.is_empty(),
        makespan,
        scheduled: assignments.len(),
        rejected: rejections.len(),
        total_wait,
        max_wait,
        assignments,
        gpu_utilization,
        rejections,
        warnings,
        generated_at_ms: now_ms(),
    })
}

/// Peak concurrent VRAM (MiB) resident on a GPU across its timeline. Concurrency
/// only rises at interval starts, so sampling those points suffices.
fn peak_vram(intervals: &[(u64, u64, u64)]) -> u64 {
    let mut peak = 0u64;
    for &(start, _, _) in intervals {
        let concurrent: u64 = intervals
            .iter()
            .filter(|&&(s, f, _)| s <= start && start < f)
            .map(|&(_, _, v)| v)
            .sum();
        peak = peak.max(concurrent);
    }
    peak
}

/// Sum over intervals of vram * duration — the area under the memory-vs-time
/// curve, used as the numerator of memory utilisation.
fn memory_time_integral(intervals: &[(u64, u64, u64)]) -> u128 {
    intervals
        .iter()
        .map(|&(s, f, v)| (f - s) as u128 * v as u128)
        .sum()
}

/// Earliest start >= est such that placing a job of `vram` MiB for `duration` on
/// the GPU keeps concurrent VRAM <= `capacity` at every instant.
///
/// Implemented as an O(k log k) sweep over the card's interval boundaries (k =
/// placed jobs): the resident-VRAM step function is scanned once to collect the
/// maximal "blocked" regions where adding this job would exceed capacity, then
/// the earliest gap of length `duration` at or after `est` that clears every
/// such region is returned. This avoids the naive per-candidate rescan, which
/// was cubic per placement and quartic across a single saturated card — a DoS
/// vector under the 2 000-job ceiling.
fn earliest_feasible_start(
    intervals: &[(u64, u64, u64)],
    est: u64,
    duration: u64,
    vram: u64,
    capacity: u64,
) -> u64 {
    // Caller guarantees vram <= capacity (GPUs with less VRAM are filtered out),
    // but guard defensively: an oversize job can never fit, so never start it.
    if vram > capacity {
        return est;
    }
    // `threshold` is the most "other" VRAM that may be resident during our window
    // while still leaving room for this job (used + vram <= capacity).
    let threshold = (capacity - vram) as i128;

    // Boundary events: +vram at each interval start, -vram at each finish.
    let mut events: Vec<(u64, i128)> = Vec::with_capacity(intervals.len() * 2);
    for &(start, finish, v) in intervals {
        events.push((start, v as i128));
        events.push((finish, -(v as i128)));
    }
    events.sort_unstable_by_key(|event| event.0);

    // Sweep the step function, collecting maximal regions where resident VRAM
    // exceeds `threshold` (i.e. our job cannot be running). Events are grouped by
    // time so the level is evaluated only after all deltas at that instant apply.
    let mut blocked: Vec<(u64, u64)> = Vec::new();
    let mut used: i128 = 0;
    let mut region_start: Option<u64> = None;
    let mut i = 0;
    while i < events.len() {
        let time = events[i].0;
        while i < events.len() && events[i].0 == time {
            used += events[i].1;
            i += 1;
        }
        match (used > threshold, region_start) {
            (true, None) => region_start = Some(time),
            (false, Some(start)) => {
                blocked.push((start, time));
                region_start = None;
            }
            _ => {}
        }
    }
    // `used` returns to 0 at the last finish, so a region is always closed.

    // Walk the blocked regions (already sorted by start) advancing the candidate
    // past any region the [candidate, candidate + duration) window would overlap.
    let mut candidate = est;
    for &(start, end) in &blocked {
        if candidate < end && candidate.saturating_add(duration) > start {
            candidate = end;
        }
    }
    candidate
}

async fn schedule_in_background(request: ScheduleRequest) -> Result<ScheduleResponse, String> {
    tokio::task::spawn_blocking(move || schedule(request))
        .await
        .map_err(|error| format!("schedule task join failed: {error}"))?
}

async fn publish_result(state: &AppState, response: &ScheduleResponse) {
    let Some(nats) = &state.nats else {
        return;
    };
    let payload = match serde_json::to_vec(&json!({
        "messageKind": "gpu.schedule.result",
        "schemaVersion": "gpu.schedule.v1",
        "source": "dd-gpu-rs",
        "result": response,
    })) {
        Ok(payload) => payload,
        Err(error) => {
            eprintln!("failed to encode gpu schedule result: {error}");
            return;
        }
    };
    if let Err(error) = nats
        .publish(state.result_subject.clone(), payload.into())
        .await
    {
        eprintln!("failed to publish gpu schedule result: {error}");
        return;
    }
    let _ = nats
        .publish(
            state.event_subject.clone(),
            json!({
                "type": "gpu.schedule.result",
                "source": "dd-gpu-rs",
                "requestId": response.request_id,
                "makespan": response.makespan,
                "scheduled": response.scheduled,
                "rejected": response.rejected,
                "atMs": now_ms(),
            })
            .to_string()
            .into(),
        )
        .await;
}

async fn healthz() -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "service": "dd-gpu-rs",
        "mode": "gpu-vram-placement-nats",
        "atMs": now_ms(),
    }))
}

async fn metrics(State(state): State<AppState>) -> Response {
    let m = &state.metrics;
    let body = format!(
        "# HELP dd_gpu_rs_requests_total HTTP schedule requests.\n\
         # TYPE dd_gpu_rs_requests_total counter\n\
         dd_gpu_rs_requests_total {}\n\
         # HELP dd_gpu_rs_schedules_total Placements computed.\n\
         # TYPE dd_gpu_rs_schedules_total counter\n\
         dd_gpu_rs_schedules_total {}\n\
         # HELP dd_gpu_rs_jobs_scheduled_total Jobs placed onto a GPU.\n\
         # TYPE dd_gpu_rs_jobs_scheduled_total counter\n\
         dd_gpu_rs_jobs_scheduled_total {}\n\
         # HELP dd_gpu_rs_jobs_rejected_total Jobs that could not be placed.\n\
         # TYPE dd_gpu_rs_jobs_rejected_total counter\n\
         dd_gpu_rs_jobs_rejected_total {}\n\
         # HELP dd_gpu_rs_errors_total Schedule or message errors.\n\
         # TYPE dd_gpu_rs_errors_total counter\n\
         dd_gpu_rs_errors_total {}\n\
         # HELP dd_gpu_rs_rejected_busy_total Requests shed because the inflight cap was full.\n\
         # TYPE dd_gpu_rs_rejected_busy_total counter\n\
         dd_gpu_rs_rejected_busy_total {}\n\
         # HELP dd_gpu_rs_nats_messages_total NATS schedule requests received.\n\
         # TYPE dd_gpu_rs_nats_messages_total counter\n\
         dd_gpu_rs_nats_messages_total {}\n",
        m.requests_total.load(Ordering::Relaxed),
        m.schedules_total.load(Ordering::Relaxed),
        m.jobs_scheduled_total.load(Ordering::Relaxed),
        m.jobs_rejected_total.load(Ordering::Relaxed),
        m.errors_total.load(Ordering::Relaxed),
        m.rejected_busy_total.load(Ordering::Relaxed),
        m.nats_messages_total.load(Ordering::Relaxed),
    );
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        body,
    )
        .into_response()
}

fn record_success(metrics: &Metrics, response: &ScheduleResponse) {
    metrics.schedules_total.fetch_add(1, Ordering::Relaxed);
    metrics
        .jobs_scheduled_total
        .fetch_add(response.scheduled as u64, Ordering::Relaxed);
    metrics
        .jobs_rejected_total
        .fetch_add(response.rejected as u64, Ordering::Relaxed);
}

async fn schedule_http(State(state): State<AppState>, Json(request): Json<ScheduleRequest>) -> Response {
    state.metrics.requests_total.fetch_add(1, Ordering::Relaxed);
    let Ok(_permit) = state.inflight.clone().try_acquire_owned() else {
        state
            .metrics
            .rejected_busy_total
            .fetch_add(1, Ordering::Relaxed);
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "ok": false, "error": "server busy; retry later" })),
        )
            .into_response();
    };
    match schedule_in_background(request).await {
        Ok(response) => {
            record_success(&state.metrics, &response);
            publish_result(&state, &response).await;
            Json(response).into_response()
        }
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            (
                StatusCode::BAD_REQUEST,
                Json(json!({ "ok": false, "error": error })),
            )
                .into_response()
        }
    }
}

async fn run_nats_loop(state: AppState, subject: String, queue_group: String) {
    let Some(nats) = state.nats.clone() else {
        println!("dd-gpu-rs nats loop disabled: NATS_URL is not configured");
        return;
    };
    println!(
        "dd-gpu-rs nats loop starting: subject={subject} queue_group={queue_group} resultSubject={}",
        state.result_subject
    );
    let mut subscription = match nats.queue_subscribe(subject, queue_group).await {
        Ok(subscription) => subscription,
        Err(error) => {
            eprintln!("dd-gpu-rs nats subscribe failed: {error}");
            return;
        }
    };
    while let Some(message) = subscription.next().await {
        state
            .metrics
            .nats_messages_total
            .fetch_add(1, Ordering::Relaxed);
        let payload = message.payload.to_vec();
        if payload.len() > MAX_NATS_PAYLOAD_BYTES {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            eprintln!(
                "dd-gpu-rs rejected oversize nats request: bytes={} max={MAX_NATS_PAYLOAD_BYTES}",
                payload.len()
            );
            continue;
        }
        // Backpressure: wait for an inflight slot before taking on more work so a
        // NATS flood can't spawn unbounded placements. NATS buffers/redelivers.
        let Ok(permit) = state.inflight.clone().acquire_owned().await else {
            continue;
        };
        let task_state = state.clone();
        tokio::spawn(async move {
            let _permit = permit;
            match serde_json::from_slice::<ScheduleRequest>(&payload) {
                Ok(request) => match schedule_in_background(request).await {
                    Ok(response) => {
                        record_success(&task_state.metrics, &response);
                        publish_result(&task_state, &response).await;
                    }
                    Err(error) => {
                        task_state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                        eprintln!("dd-gpu-rs failed nats schedule: {error}");
                    }
                },
                Err(error) => {
                    task_state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                    eprintln!("dd-gpu-rs invalid nats request: {error}");
                }
            }
        });
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let host = env_value("HOST", "0.0.0.0");
    let port = env_value("PORT", "8136").parse::<u16>()?;
    let nats = match env::var("NATS_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        Some(url) => Some(async_nats::connect(url).await?),
        None => None,
    };
    let max_inflight = env_usize("GPU_MAX_INFLIGHT", DEFAULT_MAX_INFLIGHT);
    let state = AppState {
        nats,
        result_subject: env_value("GPU_RESULT_SUBJECT", GPU_JOB_RESULTS_SUBJECT),
        event_subject: env_value("GPU_EVENT_SUBJECT", RUNTIME_EVENTS_SUBJECT),
        metrics: Arc::new(Metrics::default()),
        inflight: Arc::new(tokio::sync::Semaphore::new(max_inflight)),
    };
    let subject = env_value("GPU_JOB_SUBJECT", GPU_JOB_REQUESTS_SUBJECT);
    let queue_group = env_value("GPU_QUEUE_GROUP", GPU_JOB_REQUESTS_QUEUE_GROUP);
    tokio::spawn(run_nats_loop(state.clone(), subject, queue_group));

    let app = Router::new()
        .route("/", get(healthz))
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics))
        .route("/schedule", post(schedule_http))
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .with_state(state)
        .merge(dd_runtime_config_client::router());

    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    println!("dd-gpu-rs listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    tokio::time::sleep(Duration::from_millis(10)).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gpu(id: &str, memory_mib: u64) -> GpuInput {
        GpuInput {
            id: id.to_string(),
            memory_mib,
            speed: None,
            tags: Vec::new(),
        }
    }

    fn job(id: &str, vram_mib: u64, duration_ms: u64) -> JobInput {
        JobInput {
            id: id.to_string(),
            vram_mib,
            duration_ms,
            priority: None,
            gpu: None,
            requires_tags: Vec::new(),
            release_ms: None,
        }
    }

    #[test]
    fn jobs_that_fit_together_run_concurrently() {
        // Two 8 GiB jobs on a 24 GiB card share the timeline -> makespan = max.
        let request = ScheduleRequest {
            request_id: None,
            kind: None,
            gpus: vec![gpu("a100", 24_000)],
            jobs: vec![job("train", 8_000, 100), job("infer", 8_000, 60)],
        };
        let response = schedule(request).unwrap();
        assert!(response.feasible);
        assert_eq!(response.scheduled, 2);
        assert_eq!(response.makespan, 100);
    }

    #[test]
    fn jobs_that_exceed_vram_time_share() {
        // Two 16 GiB jobs cannot coexist on a 24 GiB card -> they serialise.
        let request = ScheduleRequest {
            request_id: None,
            kind: None,
            gpus: vec![gpu("a100", 24_000)],
            jobs: vec![job("a", 16_000, 50), job("b", 16_000, 70)],
        };
        let response = schedule(request).unwrap();
        assert_eq!(response.makespan, 120);
        let peak = response.gpu_utilization[0].peak_vram_mib;
        assert_eq!(peak, 16_000);
    }

    #[test]
    fn jobs_spread_across_gpus() {
        // Two big jobs, two cards -> each card takes one, makespan = max duration.
        let request = ScheduleRequest {
            request_id: None,
            kind: None,
            gpus: vec![gpu("g0", 16_000), gpu("g1", 16_000)],
            jobs: vec![job("a", 16_000, 40), job("b", 16_000, 90)],
        };
        let response = schedule(request).unwrap();
        assert_eq!(response.scheduled, 2);
        assert_ne!(response.assignments[0].gpu, response.assignments[1].gpu);
        assert_eq!(response.makespan, 90);
    }

    #[test]
    fn faster_gpu_shortens_duration() {
        let request = ScheduleRequest {
            request_id: None,
            kind: None,
            gpus: vec![GpuInput {
                id: "h100".to_string(),
                memory_mib: 80_000,
                speed: Some(2.0),
                tags: Vec::new(),
            }],
            jobs: vec![job("train", 10_000, 100)],
        };
        let response = schedule(request).unwrap();
        // 100ms at 2x speed -> 50ms.
        assert_eq!(response.makespan, 50);
    }

    #[test]
    fn required_tags_route_to_matching_gpu() {
        let request = ScheduleRequest {
            request_id: None,
            kind: None,
            gpus: vec![
                gpu("cpu-ish", 16_000),
                GpuInput {
                    id: "fp8".to_string(),
                    memory_mib: 80_000,
                    speed: None,
                    tags: vec!["fp8".to_string(), "h100".to_string()],
                },
            ],
            jobs: vec![JobInput {
                id: "quantized".to_string(),
                vram_mib: 8_000,
                duration_ms: 30,
                priority: None,
                gpu: None,
                requires_tags: vec!["fp8".to_string()],
                release_ms: None,
            }],
        };
        let response = schedule(request).unwrap();
        assert_eq!(response.assignments[0].gpu, "fp8");
    }

    #[test]
    fn job_too_large_for_any_gpu_is_rejected() {
        let request = ScheduleRequest {
            request_id: None,
            kind: None,
            gpus: vec![gpu("g0", 16_000)],
            jobs: vec![job("huge", 40_000, 10)],
        };
        let response = schedule(request).unwrap();
        assert!(!response.feasible);
        assert_eq!(response.rejected, 1);
        assert_eq!(response.rejections[0].id, "huge");
    }

    #[test]
    fn pinned_job_stays_on_its_gpu() {
        let request = ScheduleRequest {
            request_id: None,
            kind: None,
            gpus: vec![gpu("g0", 24_000), gpu("g1", 24_000)],
            jobs: vec![JobInput {
                id: "pinned".to_string(),
                vram_mib: 8_000,
                duration_ms: 25,
                priority: None,
                gpu: Some("g1".to_string()),
                requires_tags: Vec::new(),
                release_ms: None,
            }],
        };
        let response = schedule(request).unwrap();
        assert_eq!(response.assignments[0].gpu, "g1");
    }

    #[test]
    fn rejects_empty_gpus_and_jobs() {
        assert!(schedule(ScheduleRequest {
            request_id: None,
            kind: None,
            gpus: vec![],
            jobs: vec![job("a", 1, 1)],
        })
        .is_err());
        assert!(schedule(ScheduleRequest {
            request_id: None,
            kind: None,
            gpus: vec![gpu("g0", 16_000)],
            jobs: vec![],
        })
        .is_err());
    }

    #[test]
    fn empty_card_starts_at_release() {
        assert_eq!(earliest_feasible_start(&[], 0, 50, 8_000, 24_000), 0);
        assert_eq!(earliest_feasible_start(&[], 30, 50, 8_000, 24_000), 30);
    }

    #[test]
    fn fits_under_threshold_starts_immediately() {
        // One 3 GiB resident job, 6 GiB request, 10 GiB card: 3+6 <= 10, so the
        // new job starts at est without waiting.
        let intervals = [(0u64, 100u64, 3_000u64)];
        assert_eq!(earliest_feasible_start(&intervals, 0, 10, 6_000, 10_000), 0);
    }

    #[test]
    fn waits_past_merged_overlapping_blockers() {
        // Two overlapping 6 GiB jobs on a 10 GiB card form one blocked region
        // [0, 200) for a 6 GiB request (6+6 > 10). The sweep must merge them and
        // start at 200, not slot between them.
        let intervals = [(0u64, 100u64, 6_000u64), (50u64, 200u64, 6_000u64)];
        assert_eq!(earliest_feasible_start(&intervals, 0, 10, 6_000, 10_000), 200);
    }

    #[test]
    fn slots_into_gap_between_blockers() {
        // Blockers at [0,100) and [300,400). A 6 GiB / 50ms request fits in the
        // clear gap starting at 100.
        let intervals = [(0u64, 100u64, 8_000u64), (300u64, 400u64, 8_000u64)];
        assert_eq!(earliest_feasible_start(&intervals, 0, 50, 6_000, 10_000), 100);
    }

    #[test]
    fn full_card_request_waits_for_total_drain() {
        // A request needing the whole card (threshold 0) must wait until every
        // resident byte is freed.
        let intervals = [(0u64, 80u64, 1_000u64), (0u64, 120u64, 1_000u64)];
        assert_eq!(earliest_feasible_start(&intervals, 0, 10, 24_000, 24_000), 120);
    }

    #[test]
    fn oversize_job_is_never_started_by_the_sweep() {
        // Defensive: vram > capacity can never fit; the sweep must not invent a
        // placement (callers filter these out, but guard anyway).
        let intervals = [(0u64, 50u64, 1_000u64)];
        assert_eq!(earliest_feasible_start(&intervals, 5, 10, 30_000, 24_000), 5);
    }

    #[test]
    fn rejects_tag_floods() {
        let mut gpus = vec![gpu("g0", 16_000)];
        gpus[0].tags = (0..MAX_TAGS + 1).map(|i| format!("t{i}")).collect();
        assert!(schedule(ScheduleRequest {
            request_id: None,
            kind: None,
            gpus,
            jobs: vec![job("a", 1_000, 1)],
        })
        .is_err());
    }
}
