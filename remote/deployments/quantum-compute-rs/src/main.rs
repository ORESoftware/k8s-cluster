// dd-quantum-compute-rs
//
// A pure-Rust state-vector quantum simulator over HTTP and NATS. There is no
// quantum hardware here: it classically simulates a register of qubits and runs
// circuits and a few textbook quantum algorithms end-to-end, returning the
// measurement distribution and each mode's answer:
//
//   * circuit — apply an arbitrary gate list (H/X/Y/Z/S/T/RX/RY/RZ/phase/
//               CX/CZ/SWAP/CCX/U …), then sample measurements.
//   * grover  — amplitude amplification: find marked basis states in ~√N steps.
//   * qaoa    — Quantum Approximate Optimization for weighted MaxCut, with a
//               classical gradient-free outer optimiser over the (γ, β) angles.
//   * vqe     — Variational Quantum Eigensolver: optimise a hardware-efficient
//               ansatz to estimate the ground-state energy of a Pauli-sum
//               Hamiltonian (with an exact reference solve on small registers).
//
// Sits beside the other fun compute servers (monte-carlo, evolution, sat-smt,
// func-approx). Pure-Rust math with a seeded PRNG, so every run is reproducible
// from a seed.

mod algorithms;
mod complex;
mod gates;
mod rng;
mod run;
mod state;

#[cfg(test)]
mod tests;

use std::{
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
    QUANTUM_SOLVE_REQUESTS_QUEUE_GROUP, QUANTUM_SOLVE_REQUESTS_SUBJECT,
    QUANTUM_SOLVE_RESULTS_SUBJECT, RUNTIME_EVENTS_SUBJECT,
};
use futures_util::StreamExt;
use serde_json::json;

use run::{solve, SolveRequest, SolveResponse};

const MAX_HTTP_BODY_BYTES: usize = 8 * 1024 * 1024;
const MAX_NATS_PAYLOAD_BYTES: usize = 4 * 1024 * 1024;
const DEFAULT_MAX_INFLIGHT: usize = 4;
/// Skip publishing a result larger than this (NATS default max_payload ~1 MiB).
const MAX_PUBLISH_BYTES: usize = 900_000;

#[derive(Clone)]
struct AppState {
    nats: Option<async_nats::Client>,
    result_subject: String,
    event_subject: String,
    metrics: Arc<Metrics>,
    /// Bounds concurrent simulations so a request/NATS flood cannot spawn
    /// unbounded CPU- and memory-heavy state-vector work.
    inflight: Arc<tokio::sync::Semaphore>,
    /// Optional shared secret; when set, HTTP solve requests must present it.
    auth_secret: Option<String>,
}

#[derive(Default)]
struct Metrics {
    requests_total: AtomicU64,
    solves_total: AtomicU64,
    errors_total: AtomicU64,
    rejected_busy_total: AtomicU64,
    auth_failures_total: AtomicU64,
    nats_messages_total: AtomicU64,
}

fn env_value(key: &str, fallback: &str) -> String {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn env_usize(key: &str, fallback: usize) -> usize {
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|&value| value > 0)
        .unwrap_or(fallback)
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
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

async fn solve_in_background(request: SolveRequest) -> Result<SolveResponse, String> {
    tokio::task::spawn_blocking(move || solve(request))
        .await
        .map_err(|error| format!("solve task join failed: {error}"))?
}

async fn publish_result(state: &AppState, response: &SolveResponse) {
    let Some(nats) = &state.nats else {
        return;
    };
    let payload = match serde_json::to_vec(&json!({
        "messageKind": "quantum.solve.result",
        "schemaVersion": "quantum.solve.v1",
        "source": "dd-quantum-compute-rs",
        "result": response,
    })) {
        Ok(payload) => payload,
        Err(error) => {
            tracing::error!("failed to encode quantum result: {error}");
            return;
        }
    };
    if payload.len() > MAX_PUBLISH_BYTES {
        tracing::error!(
            "quantum result too large to publish: bytes={} max={MAX_PUBLISH_BYTES}",
            payload.len()
        );
        state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
        return;
    }
    if let Err(error) = nats
        .publish(state.result_subject.clone(), payload.into())
        .await
    {
        tracing::error!("failed to publish quantum result: {error}");
        return;
    }
    let _ = nats
        .publish(
            state.event_subject.clone(),
            json!({
                "type": "quantum.solve.result",
                "source": "dd-quantum-compute-rs",
                "requestId": response.request_id,
                "mode": response.mode,
                "qubits": response.qubits,
                "objective": response.objective,
                "objectiveKind": response.objective_kind,
                "successProbability": response.success_probability,
                "durationMs": response.duration_ms,
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
        "service": "dd-quantum-compute-rs",
        "mode": "quantum-simulator",
        "modes": ["circuit", "grover", "qaoa", "vqe"],
        "atMs": now_ms(),
    }))
}

async fn metrics(State(state): State<AppState>) -> Response {
    let m = &state.metrics;
    let body = format!(
        "# HELP dd_quantum_requests_total HTTP solve requests.\n\
         # TYPE dd_quantum_requests_total counter\n\
         dd_quantum_requests_total {}\n\
         # HELP dd_quantum_solves_total Simulations completed.\n\
         # TYPE dd_quantum_solves_total counter\n\
         dd_quantum_solves_total {}\n\
         # HELP dd_quantum_errors_total Solve or message errors.\n\
         # TYPE dd_quantum_errors_total counter\n\
         dd_quantum_errors_total {}\n\
         # HELP dd_quantum_rejected_busy_total Requests shed because the inflight cap was full.\n\
         # TYPE dd_quantum_rejected_busy_total counter\n\
         dd_quantum_rejected_busy_total {}\n\
         # HELP dd_quantum_auth_failures_total Rejected unauthenticated/invalid-secret requests.\n\
         # TYPE dd_quantum_auth_failures_total counter\n\
         dd_quantum_auth_failures_total {}\n\
         # HELP dd_quantum_nats_messages_total NATS solve requests received.\n\
         # TYPE dd_quantum_nats_messages_total counter\n\
         dd_quantum_nats_messages_total {}\n",
        m.requests_total.load(Ordering::Relaxed),
        m.solves_total.load(Ordering::Relaxed),
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

async fn solve_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SolveRequest>,
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
    match solve_in_background(request).await {
        Ok(response) => {
            state.metrics.solves_total.fetch_add(1, Ordering::Relaxed);
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
        tracing::info!("quantum nats loop disabled: NATS_URL is not configured");
        return;
    };
    tracing::info!(
        "quantum nats loop starting: subject={subject} queue_group={queue_group} resultSubject={}",
        state.result_subject
    );
    loop {
        let mut subscription = match nats.queue_subscribe(subject.clone(), queue_group.clone()).await
        {
            Ok(subscription) => subscription,
            Err(error) => {
                tracing::error!("quantum subscribe failed: {error}; retrying in 5s");
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
                    "quantum rejected oversize nats request: bytes={} max={MAX_NATS_PAYLOAD_BYTES}",
                    payload.len()
                );
                continue;
            }
            // Backpressure: wait for an inflight slot before taking on more work
            // so a NATS flood can't spawn unbounded simulations. NATS redelivers.
            let Ok(permit) = state.inflight.clone().acquire_owned().await else {
                continue;
            };
            let task_state = state.clone();
            tokio::spawn(async move {
                let _permit = permit;
                match serde_json::from_slice::<SolveRequest>(&payload) {
                    Ok(request) => match solve_in_background(request).await {
                        Ok(response) => {
                            task_state.metrics.solves_total.fetch_add(1, Ordering::Relaxed);
                            publish_result(&task_state, &response).await;
                        }
                        Err(error) => {
                            task_state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                            tracing::error!("quantum failed nats solve: {error}");
                        }
                    },
                    Err(error) => {
                        task_state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                        tracing::error!("quantum invalid nats request: {error}");
                    }
                }
            });
        }
        tracing::error!("quantum subscription ended; re-subscribing in 5s");
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let _otel = dd_telemetry::init("dd-quantum-compute-rs");

    let host = env_value("HOST", "0.0.0.0");
    let port = env_value("PORT", "8140").parse::<u16>()?;
    let nats = match env::var("NATS_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        Some(url) => match async_nats::connect(&url).await {
            Ok(client) => Some(client),
            Err(error) => {
                tracing::error!("quantum-compute-rs NATS connect failed ({url}): {error}");
                None
            }
        },
        None => None,
    };
    let max_inflight = env_usize("QUANTUM_MAX_INFLIGHT", DEFAULT_MAX_INFLIGHT);
    let state = AppState {
        nats,
        result_subject: env_value("QUANTUM_RESULT_SUBJECT", QUANTUM_SOLVE_RESULTS_SUBJECT),
        event_subject: env_value("QUANTUM_EVENT_SUBJECT", RUNTIME_EVENTS_SUBJECT),
        metrics: Arc::new(Metrics::default()),
        inflight: Arc::new(tokio::sync::Semaphore::new(max_inflight)),
        auth_secret: optional_auth_secret("QUANTUM_AUTH_SECRET"),
    };
    let subject = env_value("QUANTUM_SOLVE_SUBJECT", QUANTUM_SOLVE_REQUESTS_SUBJECT);
    let queue_group = env_value("QUANTUM_QUEUE_GROUP", QUANTUM_SOLVE_REQUESTS_QUEUE_GROUP);
    tokio::spawn(run_nats_loop(state.clone(), subject, queue_group));

    let app = Router::new()
        .route("/", get(healthz))
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics))
        .route("/solve", post(solve_http))
        .route("/simulate", post(solve_http))
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .with_state(state)
        .merge(dd_runtime_config_client::router());

    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    tracing::info!("dd-quantum-compute-rs listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app.layer(dd_telemetry::http_trace_layer()))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    tokio::time::sleep(Duration::from_millis(10)).await;
    Ok(())
}
