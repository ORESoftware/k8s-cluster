// dd-func-approx-rs
//
// An Eureqa-style function approximator over HTTP and NATS. Given a dataset it
// discovers a non-linear regression model several ways and returns the result
// in as analytic a form as the method allows:
//
//   * symbolic  — genetic-programming symbolic regression. Returns human-
//                 readable equations along an accuracy/complexity Pareto front,
//                 plus the symbolic derivatives of the chosen equation. Fuses an
//                 analytic least-squares solve (linear scaling) into evolution.
//   * neural    — a small MLP trained by backpropagation (analytic gradients).
//   * evolution — the same MLP, but its weights evolved by a self-adaptive
//                 Evolution Strategy (gradient-free neuroevolution).
//   * hybrid    — neuroevolution followed by a short gradient-descent polish.
//   * linear    — closed-form ridge polynomial least squares (exact, analytic).
//   * auto      — run several and keep whichever generalises best, preferring
//                 the simpler analytic answer on a near-tie.
//
// Sits beside the other compute servers (monte-carlo, evolution, economics).
// Pure-Rust math with a seeded PRNG, so every fit is reproducible from a seed.

mod data;
mod evo;
mod fit;
mod gp;
mod linalg;
mod nn;
mod rng;

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
    FUNC_APPROX_FIT_REQUESTS_QUEUE_GROUP, FUNC_APPROX_FIT_REQUESTS_SUBJECT,
    FUNC_APPROX_FIT_RESULTS_SUBJECT, RUNTIME_EVENTS_SUBJECT,
};
use futures_util::StreamExt;
use serde_json::json;

use fit::{fit, FitRequest, FitResponse};

const MAX_HTTP_BODY_BYTES: usize = 8 * 1024 * 1024;
const MAX_NATS_PAYLOAD_BYTES: usize = 4 * 1024 * 1024;
const DEFAULT_MAX_INFLIGHT: usize = 8;
/// Skip publishing a result larger than this (NATS default max_payload ~1 MiB).
const MAX_PUBLISH_BYTES: usize = 900_000;

#[derive(Clone)]
struct AppState {
    nats: Option<async_nats::Client>,
    result_subject: String,
    event_subject: String,
    metrics: Arc<Metrics>,
    /// Bounds concurrent fits so a request/NATS flood cannot spawn unbounded
    /// CPU-heavy evolution.
    inflight: Arc<tokio::sync::Semaphore>,
    /// Optional shared secret; when set, HTTP compute requests must present it.
    auth_secret: Option<String>,
}

#[derive(Default)]
struct Metrics {
    requests_total: AtomicU64,
    fits_total: AtomicU64,
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

async fn fit_in_background(request: FitRequest) -> Result<FitResponse, String> {
    tokio::task::spawn_blocking(move || fit(request))
        .await
        .map_err(|error| format!("fit task join failed: {error}"))?
}

async fn publish_result(state: &AppState, response: &FitResponse) {
    let Some(nats) = &state.nats else {
        return;
    };
    let payload = match serde_json::to_vec(&json!({
        "messageKind": "funcapprox.fit.result",
        "schemaVersion": "funcapprox.fit.v1",
        "source": "dd-func-approx-rs",
        "result": response,
    })) {
        Ok(payload) => payload,
        Err(error) => {
            tracing::error!("failed to encode func-approx result: {error}");
            return;
        }
    };
    if payload.len() > MAX_PUBLISH_BYTES {
        tracing::error!(
            "func-approx result too large to publish: bytes={} max={MAX_PUBLISH_BYTES}",
            payload.len()
        );
        state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
        return;
    }
    if let Err(error) = nats
        .publish(state.result_subject.clone(), payload.into())
        .await
    {
        tracing::error!("failed to publish func-approx result: {error}");
        return;
    }
    let _ = nats
        .publish(
            state.event_subject.clone(),
            json!({
                "type": "funcapprox.fit.result",
                "source": "dd-func-approx-rs",
                "requestId": response.request_id,
                "method": response.selected.clone().unwrap_or_else(|| response.method.clone()),
                "expression": response.expression,
                "valRmse": response.validation.rmse,
                "valR2": response.validation.r2,
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
        "service": "dd-func-approx-rs",
        "mode": "function-approximator",
        "methods": ["symbolic", "neural", "evolution", "hybrid", "linear", "auto"],
        "atMs": now_ms(),
    }))
}

async fn metrics(State(state): State<AppState>) -> Response {
    let m = &state.metrics;
    let body = format!(
        "# HELP dd_func_approx_requests_total HTTP fit requests.\n\
         # TYPE dd_func_approx_requests_total counter\n\
         dd_func_approx_requests_total {}\n\
         # HELP dd_func_approx_fits_total Fits completed.\n\
         # TYPE dd_func_approx_fits_total counter\n\
         dd_func_approx_fits_total {}\n\
         # HELP dd_func_approx_errors_total Fit or message errors.\n\
         # TYPE dd_func_approx_errors_total counter\n\
         dd_func_approx_errors_total {}\n\
         # HELP dd_func_approx_rejected_busy_total Requests shed because the inflight cap was full.\n\
         # TYPE dd_func_approx_rejected_busy_total counter\n\
         dd_func_approx_rejected_busy_total {}\n\
         # HELP dd_func_approx_auth_failures_total Rejected unauthenticated/invalid-secret requests.\n\
         # TYPE dd_func_approx_auth_failures_total counter\n\
         dd_func_approx_auth_failures_total {}\n\
         # HELP dd_func_approx_nats_messages_total NATS fit requests received.\n\
         # TYPE dd_func_approx_nats_messages_total counter\n\
         dd_func_approx_nats_messages_total {}\n",
        m.requests_total.load(Ordering::Relaxed),
        m.fits_total.load(Ordering::Relaxed),
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

async fn approximate_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<FitRequest>,
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
    match fit_in_background(request).await {
        Ok(response) => {
            state.metrics.fits_total.fetch_add(1, Ordering::Relaxed);
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
        tracing::info!("func-approx nats loop disabled: NATS_URL is not configured");
        return;
    };
    tracing::info!(
        "func-approx nats loop starting: subject={subject} queue_group={queue_group} resultSubject={}",
        state.result_subject
    );
    loop {
        let mut subscription = match nats.queue_subscribe(subject.clone(), queue_group.clone()).await {
            Ok(subscription) => subscription,
            Err(error) => {
                tracing::error!("func-approx subscribe failed: {error}; retrying in 5s");
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
                    "func-approx rejected oversize nats request: bytes={} max={MAX_NATS_PAYLOAD_BYTES}",
                    payload.len()
                );
                continue;
            }
            // Backpressure: wait for an inflight slot before taking on more work
            // so a NATS flood can't spawn unbounded evolution. NATS redelivers.
            let Ok(permit) = state.inflight.clone().acquire_owned().await else {
                continue;
            };
            let task_state = state.clone();
            tokio::spawn(async move {
                let _permit = permit;
                match serde_json::from_slice::<FitRequest>(&payload) {
                    Ok(request) => match fit_in_background(request).await {
                        Ok(response) => {
                            task_state.metrics.fits_total.fetch_add(1, Ordering::Relaxed);
                            publish_result(&task_state, &response).await;
                        }
                        Err(error) => {
                            task_state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                            tracing::error!("func-approx failed nats fit: {error}");
                        }
                    },
                    Err(error) => {
                        task_state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                        tracing::error!("func-approx invalid nats request: {error}");
                    }
                }
            });
        }
        tracing::error!("func-approx subscription ended; re-subscribing in 5s");
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let _otel = dd_telemetry::init("dd-func-approx-rs");

    let host = env_value("HOST", "0.0.0.0");
    let port = env_value("PORT", "8139").parse::<u16>()?;
    let nats = match env::var("NATS_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        Some(url) => match async_nats::connect(&url).await {
            Ok(client) => Some(client),
            Err(error) => {
                tracing::error!("func-approx-rs NATS connect failed ({url}): {error}");
                None
            }
        },
        None => None,
    };
    let max_inflight = env_usize("FUNC_APPROX_MAX_INFLIGHT", DEFAULT_MAX_INFLIGHT);
    let state = AppState {
        nats,
        result_subject: env_value("FUNC_APPROX_RESULT_SUBJECT", FUNC_APPROX_FIT_RESULTS_SUBJECT),
        event_subject: env_value("FUNC_APPROX_EVENT_SUBJECT", RUNTIME_EVENTS_SUBJECT),
        metrics: Arc::new(Metrics::default()),
        inflight: Arc::new(tokio::sync::Semaphore::new(max_inflight)),
        auth_secret: optional_auth_secret("FUNC_APPROX_AUTH_SECRET"),
    };
    let subject = env_value("FUNC_APPROX_FIT_SUBJECT", FUNC_APPROX_FIT_REQUESTS_SUBJECT);
    let queue_group = env_value("FUNC_APPROX_QUEUE_GROUP", FUNC_APPROX_FIT_REQUESTS_QUEUE_GROUP);
    tokio::spawn(run_nats_loop(state.clone(), subject, queue_group));

    let app = Router::new()
        .route("/", get(healthz))
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics))
        .route("/approximate", post(approximate_http))
        .route("/fit", post(approximate_http))
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .with_state(state)
        .merge(dd_runtime_config_client::router());

    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    tracing::info!("dd-func-approx-rs listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app.layer(dd_telemetry::http_trace_layer()))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    tokio::time::sleep(Duration::from_millis(10)).await;
    Ok(())
}
