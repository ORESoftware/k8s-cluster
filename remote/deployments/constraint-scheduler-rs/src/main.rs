// dd-constraint-scheduler
//
// Constraint scheduling (job-shop, nurse-rostering, timetabling) over HTTP and
// NATS. The core is a priority-rule serial schedule generation scheme (SGS):
// tasks are ordered by a priority rule (critical-path tail by default) and
// placed one at a time at the earliest start that satisfies (a) precedence,
// (b) release times, and (c) machine/resource capacity (each machine runs up
// to `capacity` tasks concurrently, default 1 = a disjunctive machine).
//
// This complements the MIP/LP solvers with a dedicated CP-style scheduler:
// fast, feasibility-first, and good for makespan minimisation on the kinds of
// resource-and-precedence problems those solvers are overkill for.

use std::{
    collections::{HashMap, HashSet, VecDeque},
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
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use dd_nats_subject_defs::{
    RUNTIME_EVENTS_SUBJECT, SCHEDULER_SCHEDULE_REQUESTS_QUEUE_GROUP,
    SCHEDULER_SCHEDULE_REQUESTS_SUBJECT, SCHEDULER_SCHEDULE_RESULTS_SUBJECT,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;

const MAX_HTTP_BODY_BYTES: usize = 1024 * 1024;
const MAX_NATS_PAYLOAD_BYTES: usize = 1024 * 1024;
const MAX_TASKS: usize = 1_000;
/// Per-task duration / release ceiling. Bounds the timeline so `start + duration`
/// can never overflow u64 and keeps utilisation denominators finite.
const MAX_TIME_UNIT: u64 = 1_000_000_000;
const DEFAULT_MAX_INFLIGHT: usize = 16;
/// Skip publishing a result larger than this (NATS default max_payload is ~1 MiB).
const MAX_PUBLISH_BYTES: usize = 900_000;

#[derive(Clone)]
struct AppState {
    nats: Option<async_nats::Client>,
    result_subject: String,
    event_subject: String,
    metrics: Arc<Metrics>,
    /// Bounds concurrent schedules so a request/NATS flood cannot spawn
    /// unbounded CPU-heavy work.
    inflight: Arc<tokio::sync::Semaphore>,
    /// Optional shared secret; when set, HTTP compute requests must present it.
    auth_secret: Option<String>,
}

#[derive(Default)]
struct Metrics {
    requests_total: AtomicU64,
    schedules_total: AtomicU64,
    feasible_total: AtomicU64,
    errors_total: AtomicU64,
    rejected_busy_total: AtomicU64,
    auth_failures_total: AtomicU64,
    nats_messages_total: AtomicU64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScheduleRequest {
    request_id: Option<String>,
    kind: Option<String>,
    priority_rule: Option<String>,
    #[serde(default)]
    machines: Vec<MachineInput>,
    tasks: Vec<TaskInput>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MachineInput {
    id: String,
    capacity: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskInput {
    id: String,
    duration: u64,
    machine: Option<String>,
    #[serde(default)]
    predecessors: Vec<String>,
    release: Option<u64>,
    due: Option<u64>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ScheduleResponse {
    ok: bool,
    request_id: String,
    kind: String,
    priority_rule: String,
    feasible: bool,
    makespan: u64,
    total_tardiness: u64,
    max_tardiness: u64,
    assignments: Vec<TaskAssignment>,
    machine_utilization: Vec<MachineUtilization>,
    warnings: Vec<String>,
    generated_at_ms: u128,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct TaskAssignment {
    id: String,
    machine: String,
    start: u64,
    finish: u64,
    duration: u64,
    tardiness: u64,
    critical: bool,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct MachineUtilization {
    machine: String,
    capacity: u32,
    busy_time: u64,
    span: u64,
    utilization: f64,
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

/// Resolve the optional auth secret from the service-specific key, falling back
/// to the shared `SERVER_AUTH_SECRET`. Empty values are treated as unset.
fn optional_auth_secret(primary: &str) -> Option<String> {
    [primary, "SERVER_AUTH_SECRET"]
        .iter()
        .filter_map(|key| env::var(key).ok())
        .map(|value| value.trim().to_string())
        .find(|value| !value.is_empty())
}

/// Timing-safe comparison so auth checks don't leak the secret via response time.
fn constant_time_equals(candidate: &str, expected: &str) -> bool {
    let candidate = candidate.as_bytes();
    let expected = expected.as_bytes();
    if candidate.len() != expected.len() {
        return false;
    }
    let mut diff = 0u8;
    for (left, right) in candidate.iter().zip(expected.iter()) {
        diff |= left ^ right;
    }
    diff == 0
}

/// Optional shared-secret gate. Open when no secret is configured (matching the
/// sibling compute services); when set, the compute endpoint requires a matching
/// `x-server-auth` (or `auth`) header.
fn check_auth(state: &AppState, headers: &HeaderMap) -> Option<Response> {
    let secret = state.auth_secret.as_deref()?;
    let authorized = ["x-server-auth", "auth"]
        .iter()
        .filter_map(|name| headers.get(*name))
        .filter_map(|value| value.to_str().ok())
        .any(|value| constant_time_equals(value, secret));
    if authorized {
        None
    } else {
        state
            .metrics
            .auth_failures_total
            .fetch_add(1, Ordering::Relaxed);
        Some(
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "ok": false, "error": "unauthorized" })),
            )
                .into_response(),
        )
    }
}

struct Task {
    id: String,
    duration: u64,
    machine: String,
    predecessors: Vec<usize>,
    release: u64,
    due: Option<u64>,
}

fn schedule(request: ScheduleRequest) -> Result<ScheduleResponse, String> {
    let request_id = request
        .request_id
        .clone()
        .unwrap_or_else(|| format!("schedule-{}", now_ms()));
    let priority_rule = request
        .priority_rule
        .clone()
        .unwrap_or_else(|| "critical-path".to_string())
        .to_ascii_lowercase();

    if request.tasks.is_empty() {
        return Err("tasks must not be empty".to_string());
    }
    if request.tasks.len() > MAX_TASKS {
        return Err(format!("too many tasks; max {MAX_TASKS}"));
    }

    // Index tasks and validate uniqueness.
    let mut index: HashMap<String, usize> = HashMap::with_capacity(request.tasks.len());
    for (i, task) in request.tasks.iter().enumerate() {
        if index.insert(task.id.clone(), i).is_some() {
            return Err(format!("duplicate task id {}", task.id));
        }
    }

    // Machine capacities. Default machine "default" with capacity 1; declared
    // machines may override. A task without an explicit machine uses "default".
    let mut capacities: HashMap<String, u32> = HashMap::new();
    capacities.insert("default".to_string(), 1);
    for machine in &request.machines {
        let capacity = machine.capacity.unwrap_or(1);
        if capacity == 0 {
            return Err(format!("machine {} capacity must be >= 1", machine.id));
        }
        // A capacity beyond the task count is meaningless and could overflow the
        // utilisation denominator; clamp the accepted range.
        if capacity as usize > MAX_TASKS {
            return Err(format!("machine {} capacity must be <= {MAX_TASKS}", machine.id));
        }
        capacities.insert(machine.id.clone(), capacity);
    }

    let mut tasks: Vec<Task> = Vec::with_capacity(request.tasks.len());
    for task in &request.tasks {
        if task.duration == 0 {
            return Err(format!("task {} duration must be >= 1", task.id));
        }
        if task.duration > MAX_TIME_UNIT {
            return Err(format!("task {} duration must be <= {MAX_TIME_UNIT}", task.id));
        }
        if task.release.unwrap_or(0) > MAX_TIME_UNIT {
            return Err(format!("task {} release must be <= {MAX_TIME_UNIT}", task.id));
        }
        let machine = task.machine.clone().unwrap_or_else(|| "default".to_string());
        if !capacities.contains_key(&machine) {
            // Implicit disjunctive machine for any unreferenced id.
            capacities.insert(machine.clone(), 1);
        }
        let mut predecessors = Vec::with_capacity(task.predecessors.len());
        for pred in &task.predecessors {
            let pred_index = *index
                .get(pred)
                .ok_or_else(|| format!("task {} references unknown predecessor {}", task.id, pred))?;
            predecessors.push(pred_index);
        }
        tasks.push(Task {
            id: task.id.clone(),
            duration: task.duration,
            machine,
            predecessors,
            release: task.release.unwrap_or(0),
            due: task.due,
        });
    }

    let order = topological_order(&tasks)?;
    let tail = critical_path_tail(&tasks, &order);
    let priority = priority_scores(&tasks, &tail, &priority_rule)?;

    // Schedule in priority order, but only release a task once all predecessors
    // are placed (priority is a tie-break atop precedence readiness).
    let mut ready_order = order.clone();
    ready_order.sort_by(|&a, &b| {
        priority[b]
            .total_cmp(&priority[a])
            .then_with(|| tasks[a].id.cmp(&tasks[b].id))
    });

    let mut start_at: Vec<Option<u64>> = vec![None; tasks.len()];
    let mut finish_at: Vec<u64> = vec![0; tasks.len()];
    let mut machine_intervals: HashMap<String, Vec<(u64, u64)>> = HashMap::new();
    let mut placed: HashSet<usize> = HashSet::new();

    // Repeatedly place the highest-priority task whose predecessors are placed.
    while placed.len() < tasks.len() {
        let next = ready_order.iter().copied().find(|&i| {
            !placed.contains(&i) && tasks[i].predecessors.iter().all(|p| placed.contains(p))
        });
        let Some(i) = next else {
            return Err("precedence graph is not schedulable (unexpected cycle)".to_string());
        };

        let mut est = tasks[i].release;
        for &p in &tasks[i].predecessors {
            est = est.max(finish_at[p]);
        }
        let machine = &tasks[i].machine;
        let capacity = *capacities.get(machine).unwrap_or(&1);
        let intervals = machine_intervals.entry(machine.clone()).or_default();
        let start = earliest_feasible_start(intervals, est, tasks[i].duration, capacity);
        let finish = start + tasks[i].duration;
        intervals.push((start, finish));
        start_at[i] = Some(start);
        finish_at[i] = finish;
        placed.insert(i);
    }

    let makespan = finish_at.iter().copied().max().unwrap_or(0);
    let critical = mark_critical(&tasks, &start_at, &finish_at, makespan);

    let mut total_tardiness = 0u64;
    let mut max_tardiness = 0u64;
    let mut assignments = Vec::with_capacity(tasks.len());
    for (i, task) in tasks.iter().enumerate() {
        let start = start_at[i].unwrap_or(0);
        let finish = finish_at[i];
        let tardiness = match task.due {
            Some(due) if finish > due => finish - due,
            _ => 0,
        };
        total_tardiness += tardiness;
        max_tardiness = max_tardiness.max(tardiness);
        assignments.push(TaskAssignment {
            id: task.id.clone(),
            machine: task.machine.clone(),
            start,
            finish,
            duration: task.duration,
            tardiness,
            critical: critical[i],
        });
    }
    assignments.sort_by(|a, b| a.start.cmp(&b.start).then_with(|| a.id.cmp(&b.id)));

    let mut machine_utilization: Vec<MachineUtilization> = machine_intervals
        .iter()
        .map(|(machine, intervals)| {
            let busy_time: u64 = intervals.iter().map(|(s, f)| f - s).sum();
            let span = intervals.iter().map(|(_, f)| *f).max().unwrap_or(0);
            let capacity = *capacities.get(machine).unwrap_or(&1);
            let denom = span.max(1) * capacity as u64;
            MachineUtilization {
                machine: machine.clone(),
                capacity,
                busy_time,
                span,
                utilization: busy_time as f64 / denom as f64,
            }
        })
        .collect();
    machine_utilization.sort_by(|a, b| a.machine.cmp(&b.machine));

    let mut warnings = Vec::new();
    if total_tardiness > 0 {
        warnings.push(format!(
            "{total_tardiness} total tardiness across due-dated tasks; consider more machine capacity or relaxed due dates"
        ));
    }

    Ok(ScheduleResponse {
        ok: true,
        request_id,
        kind: request
            .kind
            .clone()
            .unwrap_or_else(|| "scheduler.sgs".to_string()),
        priority_rule,
        feasible: true,
        makespan,
        total_tardiness,
        max_tardiness,
        assignments,
        machine_utilization,
        warnings,
        generated_at_ms: now_ms(),
    })
}

fn topological_order(tasks: &[Task]) -> Result<Vec<usize>, String> {
    let n = tasks.len();
    let mut indegree = vec![0usize; n];
    let mut successors: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, task) in tasks.iter().enumerate() {
        for &p in &task.predecessors {
            successors[p].push(i);
            indegree[i] += 1;
        }
    }
    let mut queue: VecDeque<usize> = (0..n).filter(|&i| indegree[i] == 0).collect();
    let mut order = Vec::with_capacity(n);
    while let Some(i) = queue.pop_front() {
        order.push(i);
        for &s in &successors[i] {
            indegree[s] -= 1;
            if indegree[s] == 0 {
                queue.push_back(s);
            }
        }
    }
    if order.len() != n {
        return Err("precedence constraints contain a cycle".to_string());
    }
    Ok(order)
}

/// Longest remaining chain (including the task itself) to any sink — the
/// critical-path "tail" used as the default priority.
fn critical_path_tail(tasks: &[Task], order: &[usize]) -> Vec<u64> {
    let n = tasks.len();
    let mut successors: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, task) in tasks.iter().enumerate() {
        for &p in &task.predecessors {
            successors[p].push(i);
        }
    }
    let mut tail = vec![0u64; n];
    for &i in order.iter().rev() {
        let mut best = 0;
        for &s in &successors[i] {
            best = best.max(tail[s]);
        }
        tail[i] = tasks[i].duration + best;
    }
    tail
}

fn priority_scores(tasks: &[Task], tail: &[u64], rule: &str) -> Result<Vec<f64>, String> {
    let scores = match rule {
        "critical-path" | "lst" | "mintail" | "cp" => tail.iter().map(|&t| t as f64).collect(),
        "lpt" => tasks.iter().map(|t| t.duration as f64).collect(),
        "spt" => tasks.iter().map(|t| -(t.duration as f64)).collect(),
        "edd" => tasks
            .iter()
            .map(|t| -(t.due.unwrap_or(u64::MAX) as f64))
            .collect(),
        "release" | "ready" => tasks.iter().map(|t| -(t.release as f64)).collect(),
        other => {
            return Err(format!(
                "unsupported priorityRule {other}; expected critical-path, lpt, spt, edd, or release"
            ))
        }
    };
    Ok(scores)
}

/// Earliest start >= est such that placing [start, start+duration) on the
/// machine keeps concurrency <= capacity at every instant.
fn earliest_feasible_start(intervals: &[(u64, u64)], est: u64, duration: u64, capacity: u32) -> u64 {
    if capacity == 0 {
        return est;
    }
    // Candidate starts: est plus every existing interval finish >= est.
    let mut candidates: Vec<u64> = vec![est];
    for &(_, finish) in intervals {
        if finish >= est {
            candidates.push(finish);
        }
    }
    candidates.sort_unstable();
    candidates.dedup();
    for &start in &candidates {
        if fits(intervals, start, duration, capacity) {
            return start;
        }
    }
    // Fallback: after the last finish (always fits on an empty timeline).
    intervals.iter().map(|(_, f)| *f).max().unwrap_or(est).max(est)
}

fn fits(intervals: &[(u64, u64)], start: u64, duration: u64, capacity: u32) -> bool {
    let end = start + duration;
    // Sample concurrency at `start` and at every existing interval start inside
    // the window — concurrency only rises at interval starts.
    let mut sample_points: Vec<u64> = vec![start];
    for &(s, _) in intervals {
        if s > start && s < end {
            sample_points.push(s);
        }
    }
    for point in sample_points {
        let concurrent = intervals
            .iter()
            .filter(|&&(s, f)| s <= point && point < f)
            .count() as u32;
        if concurrent + 1 > capacity {
            return false;
        }
    }
    true
}

/// A task is critical if it lies on a path realising the makespan: it finishes
/// at the makespan, or a critical successor starts exactly when it finishes.
fn mark_critical(tasks: &[Task], start_at: &[Option<u64>], finish_at: &[u64], makespan: u64) -> Vec<bool> {
    let n = tasks.len();
    let mut successors: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, task) in tasks.iter().enumerate() {
        for &p in &task.predecessors {
            successors[p].push(i);
        }
    }
    let mut critical = vec![false; n];
    // Process in decreasing finish time so successors are decided first.
    let mut by_finish: Vec<usize> = (0..n).collect();
    by_finish.sort_by(|&a, &b| finish_at[b].cmp(&finish_at[a]));
    for &i in &by_finish {
        if finish_at[i] == makespan {
            critical[i] = true;
            continue;
        }
        for &s in &successors[i] {
            if critical[s] && start_at[s] == Some(finish_at[i]) {
                critical[i] = true;
                break;
            }
        }
    }
    critical
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
        "messageKind": "scheduler.schedule.result",
        "schemaVersion": "scheduler.schedule.v1",
        "source": "dd-constraint-scheduler",
        "result": response,
    })) {
        Ok(payload) => payload,
        Err(error) => {
            tracing::error!("failed to encode schedule result: {error}");
            return;
        }
    };
    if payload.len() > MAX_PUBLISH_BYTES {
        tracing::error!(
            "schedule result too large to publish: bytes={} max={MAX_PUBLISH_BYTES}",
            payload.len()
        );
        state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
        return;
    }
    if let Err(error) = nats
        .publish(state.result_subject.clone(), payload.into())
        .await
    {
        tracing::error!("failed to publish schedule result: {error}");
        return;
    }
    let _ = nats
        .publish(
            state.event_subject.clone(),
            json!({
                "type": "scheduler.schedule.result",
                "source": "dd-constraint-scheduler",
                "requestId": response.request_id,
                "makespan": response.makespan,
                "totalTardiness": response.total_tardiness,
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
        "service": "dd-constraint-scheduler",
        "mode": "cp-sgs-nats",
        "atMs": now_ms(),
    }))
}

async fn metrics(State(state): State<AppState>) -> Response {
    let m = &state.metrics;
    let body = format!(
        "# HELP dd_constraint_scheduler_requests_total HTTP schedule requests.\n\
         # TYPE dd_constraint_scheduler_requests_total counter\n\
         dd_constraint_scheduler_requests_total {}\n\
         # HELP dd_constraint_scheduler_schedules_total Schedules computed.\n\
         # TYPE dd_constraint_scheduler_schedules_total counter\n\
         dd_constraint_scheduler_schedules_total {}\n\
         # HELP dd_constraint_scheduler_feasible_total Feasible schedules.\n\
         # TYPE dd_constraint_scheduler_feasible_total counter\n\
         dd_constraint_scheduler_feasible_total {}\n\
         # HELP dd_constraint_scheduler_errors_total Schedule or message errors.\n\
         # TYPE dd_constraint_scheduler_errors_total counter\n\
         dd_constraint_scheduler_errors_total {}\n\
         # HELP dd_constraint_scheduler_rejected_busy_total Requests shed because the inflight cap was full.\n\
         # TYPE dd_constraint_scheduler_rejected_busy_total counter\n\
         dd_constraint_scheduler_rejected_busy_total {}\n\
         # HELP dd_constraint_scheduler_auth_failures_total Rejected unauthenticated/invalid-secret requests.\n\
         # TYPE dd_constraint_scheduler_auth_failures_total counter\n\
         dd_constraint_scheduler_auth_failures_total {}\n\
         # HELP dd_constraint_scheduler_nats_messages_total NATS schedule requests received.\n\
         # TYPE dd_constraint_scheduler_nats_messages_total counter\n\
         dd_constraint_scheduler_nats_messages_total {}\n",
        m.requests_total.load(Ordering::Relaxed),
        m.schedules_total.load(Ordering::Relaxed),
        m.feasible_total.load(Ordering::Relaxed),
        m.errors_total.load(Ordering::Relaxed),
        m.rejected_busy_total.load(Ordering::Relaxed),
        m.auth_failures_total.load(Ordering::Relaxed),
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

async fn schedule_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ScheduleRequest>,
) -> Response {
    if let Some(response) = check_auth(&state, &headers) {
        return response;
    }
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
            state.metrics.schedules_total.fetch_add(1, Ordering::Relaxed);
            if response.feasible {
                state.metrics.feasible_total.fetch_add(1, Ordering::Relaxed);
            }
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
        tracing::info!("constraint-scheduler nats loop disabled: NATS_URL is not configured");
        return;
    };
    tracing::info!(
        "constraint-scheduler nats loop starting: subject={subject} queue_group={queue_group} resultSubject={}",
        state.result_subject
    );
    loop {
        let mut subscription = match nats.queue_subscribe(subject.clone(), queue_group.clone()).await {
            Ok(subscription) => subscription,
            Err(error) => {
                tracing::error!("constraint-scheduler subscribe failed: {error}; retrying in 5s");
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
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
                tracing::error!(
                    "constraint-scheduler rejected oversize nats request: bytes={} max={MAX_NATS_PAYLOAD_BYTES}",
                    payload.len()
                );
                continue;
            }
            // Backpressure: wait for an inflight slot before taking on more work so a
            // NATS flood can't spawn unbounded schedules. NATS buffers/redelivers.
            let Ok(permit) = state.inflight.clone().acquire_owned().await else {
                continue;
            };
            let task_state = state.clone();
            tokio::spawn(async move {
                let _permit = permit;
                match serde_json::from_slice::<ScheduleRequest>(&payload) {
                    Ok(request) => match schedule_in_background(request).await {
                        Ok(response) => {
                            task_state.metrics.schedules_total.fetch_add(1, Ordering::Relaxed);
                            if response.feasible {
                                task_state.metrics.feasible_total.fetch_add(1, Ordering::Relaxed);
                            }
                            publish_result(&task_state, &response).await;
                        }
                        Err(error) => {
                            task_state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                            tracing::error!("constraint-scheduler failed nats schedule: {error}");
                        }
                    },
                    Err(error) => {
                        task_state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                        tracing::error!("constraint-scheduler invalid nats request: {error}");
                    }
                }
            });
        }
        tracing::error!("constraint-scheduler subscription ended; re-subscribing in 5s");
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let _otel = dd_telemetry::init("dd-constraint-scheduler");

    let host = env_value("HOST", "0.0.0.0");
    let port = env_value("PORT", "8131").parse::<u16>()?;
    let nats = match env::var("NATS_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        Some(url) => match async_nats::connect(&url).await {
            Ok(client) => Some(client),
            Err(error) => {
                tracing::error!("constraint-scheduler NATS connect failed ({url}): {error}");
                None
            }
        },
        None => None,
    };
    let max_inflight = env_usize("SCHEDULER_MAX_INFLIGHT", DEFAULT_MAX_INFLIGHT);
    let state = AppState {
        nats,
        result_subject: env_value("SCHEDULER_RESULT_SUBJECT", SCHEDULER_SCHEDULE_RESULTS_SUBJECT),
        event_subject: env_value("SCHEDULER_EVENT_SUBJECT", RUNTIME_EVENTS_SUBJECT),
        metrics: Arc::new(Metrics::default()),
        inflight: Arc::new(tokio::sync::Semaphore::new(max_inflight)),
        auth_secret: optional_auth_secret("SCHEDULER_AUTH_SECRET"),
    };
    let subject = env_value("SCHEDULER_SCHEDULE_SUBJECT", SCHEDULER_SCHEDULE_REQUESTS_SUBJECT);
    let queue_group = env_value("SCHEDULER_QUEUE_GROUP", SCHEDULER_SCHEDULE_REQUESTS_QUEUE_GROUP);
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
    tracing::info!("dd-constraint-scheduler listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app.layer(dd_telemetry::http_trace_layer()))
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

    fn test_state(secret: Option<String>) -> AppState {
        AppState {
            nats: None,
            result_subject: String::new(),
            event_subject: String::new(),
            metrics: Arc::new(Metrics::default()),
            inflight: Arc::new(tokio::sync::Semaphore::new(1)),
            auth_secret: secret,
        }
    }

    #[test]
    fn auth_open_when_no_secret() {
        assert!(check_auth(&test_state(None), &HeaderMap::new()).is_none());
    }

    #[test]
    fn auth_enforced_when_secret_set() {
        let state = test_state(Some("s3cret".to_string()));
        assert!(check_auth(&state, &HeaderMap::new()).is_some());
        let mut good = HeaderMap::new();
        good.insert("x-server-auth", "s3cret".parse().unwrap());
        assert!(check_auth(&state, &good).is_none());
    }

    fn task(id: &str, duration: u64, machine: &str, preds: &[&str]) -> TaskInput {
        TaskInput {
            id: id.to_string(),
            duration,
            machine: Some(machine.to_string()),
            predecessors: preds.iter().map(|p| p.to_string()).collect(),
            release: None,
            due: None,
        }
    }

    #[test]
    fn serial_chain_sums_durations() {
        let request = ScheduleRequest {
            request_id: None,
            kind: None,
            priority_rule: None,
            machines: vec![MachineInput { id: "m1".to_string(), capacity: Some(1) }],
            tasks: vec![
                task("a", 3, "m1", &[]),
                task("b", 2, "m1", &["a"]),
                task("c", 4, "m1", &["b"]),
            ],
        };
        let response = schedule(request).unwrap();
        assert!(response.feasible);
        assert_eq!(response.makespan, 9);
    }

    #[test]
    fn disjunctive_machine_serialises_independent_tasks() {
        // Two independent tasks on a capacity-1 machine cannot overlap.
        let request = ScheduleRequest {
            request_id: None,
            kind: None,
            priority_rule: None,
            machines: vec![MachineInput { id: "m1".to_string(), capacity: Some(1) }],
            tasks: vec![task("a", 3, "m1", &[]), task("b", 5, "m1", &[])],
        };
        let response = schedule(request).unwrap();
        assert_eq!(response.makespan, 8);
    }

    #[test]
    fn parallel_capacity_allows_overlap() {
        // Capacity 2 lets both run at once -> makespan = max duration.
        let request = ScheduleRequest {
            request_id: None,
            kind: None,
            priority_rule: None,
            machines: vec![MachineInput { id: "pool".to_string(), capacity: Some(2) }],
            tasks: vec![task("a", 3, "pool", &[]), task("b", 5, "pool", &[])],
        };
        let response = schedule(request).unwrap();
        assert_eq!(response.makespan, 5);
    }

    #[test]
    fn rejects_cycle() {
        let request = ScheduleRequest {
            request_id: None,
            kind: None,
            priority_rule: None,
            machines: vec![],
            tasks: vec![task("a", 1, "default", &["b"]), task("b", 1, "default", &["a"])],
        };
        assert!(schedule(request).is_err());
    }
}
