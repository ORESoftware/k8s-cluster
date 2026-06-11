// dd-monte-carlo-server
//
// A generic Monte Carlo estimation engine over HTTP and NATS. Every experiment
// returns a point estimate with its standard error and a 95% confidence
// interval, so results are statistically honest rather than a single number.
//
// Experiments:
//   * pi        — estimate π by sampling the unit square / quarter circle
//   * option    — European call/put price under geometric Brownian motion,
//                 with the Black-Scholes closed form as an analytic reference
//   * queue     — M/M/1 queue simulation (mean wait, L, Lq, utilisation) vs the
//                 analytic steady-state formulas
//   * integrate — Monte Carlo integral of a built-in function over [a, b]
//
// Slots beside the economics / trading servers. Determinism via a seeded
// SplitMix64 PRNG with Box-Muller normals (no external rand dependency).

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
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use dd_nats_subject_defs::{
    MONTE_CARLO_SIMULATE_REQUESTS_QUEUE_GROUP, MONTE_CARLO_SIMULATE_REQUESTS_SUBJECT,
    MONTE_CARLO_SIMULATE_RESULTS_SUBJECT, RUNTIME_EVENTS_SUBJECT,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;

const MAX_HTTP_BODY_BYTES: usize = 256 * 1024;
const MAX_NATS_PAYLOAD_BYTES: usize = 256 * 1024;
const DEFAULT_SAMPLES: u64 = 100_000;
const MAX_SAMPLES: u64 = 20_000_000;
const DEFAULT_MAX_INFLIGHT: usize = 16;
/// Skip publishing a result larger than this (NATS default max_payload is ~1 MiB).
const MAX_PUBLISH_BYTES: usize = 900_000;

#[derive(Clone)]
struct AppState {
    nats: Option<async_nats::Client>,
    result_subject: String,
    event_subject: String,
    metrics: Arc<Metrics>,
    /// Bounds concurrent simulations so a request/NATS flood cannot spawn
    /// unbounded CPU-heavy sampling.
    inflight: Arc<tokio::sync::Semaphore>,
}

#[derive(Default)]
struct Metrics {
    requests_total: AtomicU64,
    simulations_total: AtomicU64,
    samples_total: AtomicU64,
    errors_total: AtomicU64,
    rejected_busy_total: AtomicU64,
    nats_messages_total: AtomicU64,
}

struct Rng {
    state: u64,
    spare: Option<f64>,
}

impl Rng {
    fn new(seed: u64) -> Self {
        Rng {
            state: seed ^ 0x9E3779B97F4A7C15,
            spare: None,
        }
    }
    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }
    fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    /// Box-Muller standard normal with one cached spare.
    fn normal(&mut self) -> f64 {
        if let Some(spare) = self.spare.take() {
            return spare;
        }
        let mut u1 = self.uniform();
        if u1 < 1e-12 {
            u1 = 1e-12;
        }
        let u2 = self.uniform();
        let mag = (-2.0 * u1.ln()).sqrt();
        self.spare = Some(mag * (std::f64::consts::TAU * u2).sin());
        mag * (std::f64::consts::TAU * u2).cos()
    }
    /// Exponential(rate) via inverse transform.
    fn exponential(&mut self, rate: f64) -> f64 {
        let mut u = self.uniform();
        if u < 1e-12 {
            u = 1e-12;
        }
        -u.ln() / rate
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SimRequest {
    request_id: Option<String>,
    experiment: String,
    samples: Option<u64>,
    seed: Option<u64>,
    // option
    spot: Option<f64>,
    strike: Option<f64>,
    rate: Option<f64>,
    volatility: Option<f64>,
    maturity: Option<f64>,
    option_type: Option<String>,
    // queue
    arrival_rate: Option<f64>,
    service_rate: Option<f64>,
    // integrate
    function: Option<String>,
    lower: Option<f64>,
    upper: Option<f64>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SimResponse {
    ok: bool,
    request_id: String,
    experiment: String,
    samples: u64,
    estimate: f64,
    standard_error: f64,
    confidence_interval: [f64; 2],
    #[serde(skip_serializing_if = "Option::is_none")]
    analytic_reference: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    relative_error: Option<f64>,
    #[serde(skip_serializing_if = "serde_json::Map::is_empty")]
    extra: serde_json::Map<String, serde_json::Value>,
    warnings: Vec<String>,
    generated_at_ms: u128,
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

fn require_positive(value: Option<f64>, label: &str) -> Result<f64, String> {
    let value = value.ok_or_else(|| format!("{label} is required"))?;
    if !value.is_finite() || value <= 0.0 {
        return Err(format!("{label} must be finite and positive"));
    }
    Ok(value)
}

fn require_bounded(value: Option<f64>, label: &str, max: f64) -> Result<f64, String> {
    let value = require_positive(value, label)?;
    if value > max {
        return Err(format!("{label} must be <= {max}"));
    }
    Ok(value)
}

/// Online mean/variance accumulator (Welford) for an honest standard error.
#[derive(Default)]
struct Accumulator {
    count: u64,
    mean: f64,
    m2: f64,
}

impl Accumulator {
    fn push(&mut self, value: f64) {
        self.count += 1;
        let delta = value - self.mean;
        self.mean += delta / self.count as f64;
        self.m2 += delta * (value - self.mean);
    }
    fn variance(&self) -> f64 {
        if self.count < 2 {
            0.0
        } else {
            self.m2 / (self.count - 1) as f64
        }
    }
    fn standard_error(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            (self.variance() / self.count as f64).sqrt()
        }
    }
}

fn finish(
    request_id: String,
    experiment: &str,
    samples: u64,
    acc: &Accumulator,
    analytic: Option<f64>,
    extra: serde_json::Map<String, serde_json::Value>,
    warnings: Vec<String>,
) -> SimResponse {
    let estimate = acc.mean;
    let se = acc.standard_error();
    let half = 1.96 * se;
    let relative_error = analytic.and_then(|reference| {
        if reference.abs() > 1e-12 {
            Some((estimate - reference).abs() / reference.abs())
        } else {
            None
        }
    });
    // serde_json serialises non-finite floats as `null`, silently breaking the
    // numeric contract. Guard the headline numbers and flag it instead.
    let mut warnings = warnings;
    if !estimate.is_finite() || !se.is_finite() {
        warnings.push(
            "estimate or standard error was non-finite (numerical overflow); inputs may be too extreme".to_string(),
        );
    }
    SimResponse {
        ok: true,
        request_id,
        experiment: experiment.to_string(),
        samples,
        estimate: finite_or(estimate, 0.0),
        standard_error: finite_or(se, 0.0),
        confidence_interval: [finite_or(estimate - half, 0.0), finite_or(estimate + half, 0.0)],
        analytic_reference: analytic.map(|value| finite_or(value, 0.0)),
        relative_error: relative_error.map(|value| finite_or(value, 0.0)),
        extra,
        warnings,
        generated_at_ms: now_ms(),
    }
}

/// Replace a non-finite float with a fallback so JSON output stays numeric.
fn finite_or(value: f64, fallback: f64) -> f64 {
    if value.is_finite() {
        value
    } else {
        fallback
    }
}

fn simulate(request: SimRequest) -> Result<SimResponse, String> {
    let request_id = request
        .request_id
        .clone()
        .unwrap_or_else(|| format!("mc-{}", now_ms()));
    let experiment = request.experiment.to_ascii_lowercase();
    let samples = request.samples.unwrap_or(DEFAULT_SAMPLES).clamp(1, MAX_SAMPLES);
    let mut rng = Rng::new(request.seed.unwrap_or(0xC0FFEE));

    match experiment.as_str() {
        "pi" => Ok(run_pi(request_id, samples, &mut rng)),
        "option" => run_option(request_id, samples, &mut rng, &request),
        "queue" | "mm1" => run_queue(request_id, samples, &mut rng, &request),
        "integrate" => run_integrate(request_id, samples, &mut rng, &request),
        other => Err(format!(
            "unsupported experiment {other}; expected pi, option, queue, or integrate"
        )),
    }
}

fn run_pi(request_id: String, samples: u64, rng: &mut Rng) -> SimResponse {
    // Each sample contributes 4 if inside the quarter circle, else 0; the mean
    // estimates π and Welford gives the standard error directly.
    let mut acc = Accumulator::default();
    for _ in 0..samples {
        let x = rng.uniform();
        let y = rng.uniform();
        acc.push(if x * x + y * y <= 1.0 { 4.0 } else { 0.0 });
    }
    finish(
        request_id,
        "pi",
        samples,
        &acc,
        Some(std::f64::consts::PI),
        serde_json::Map::new(),
        Vec::new(),
    )
}

fn standard_normal_cdf(x: f64) -> f64 {
    // Abramowitz & Stegun 7.1.26 erf approximation.
    let t = x / std::f64::consts::SQRT_2;
    let sign = if t < 0.0 { -1.0 } else { 1.0 };
    let z = t.abs();
    let a1 = 0.254829592;
    let a2 = -0.284496736;
    let a3 = 1.421413741;
    let a4 = -1.453152027;
    let a5 = 1.061405429;
    let p = 0.3275911;
    let k = 1.0 / (1.0 + p * z);
    let y = 1.0 - (((((a5 * k + a4) * k) + a3) * k + a2) * k + a1) * k * (-z * z).exp();
    0.5 * (1.0 + sign * y)
}

fn black_scholes(spot: f64, strike: f64, rate: f64, vol: f64, maturity: f64, is_call: bool) -> f64 {
    if maturity <= 0.0 || vol <= 0.0 {
        let intrinsic = if is_call {
            (spot - strike).max(0.0)
        } else {
            (strike - spot).max(0.0)
        };
        return intrinsic;
    }
    let d1 = ((spot / strike).ln() + (rate + 0.5 * vol * vol) * maturity) / (vol * maturity.sqrt());
    let d2 = d1 - vol * maturity.sqrt();
    if is_call {
        spot * standard_normal_cdf(d1) - strike * (-rate * maturity).exp() * standard_normal_cdf(d2)
    } else {
        strike * (-rate * maturity).exp() * standard_normal_cdf(-d2) - spot * standard_normal_cdf(-d1)
    }
}

fn run_option(
    request_id: String,
    samples: u64,
    rng: &mut Rng,
    request: &SimRequest,
) -> Result<SimResponse, String> {
    let spot = require_bounded(request.spot, "spot", 1e12)?;
    let strike = require_bounded(request.strike, "strike", 1e12)?;
    // Keep the GBM exponent in a range where exp() stays finite for tail draws,
    // so the estimate can't silently become Inf (which serialises as null).
    let vol = require_bounded(request.volatility, "volatility", 5.0)?;
    let maturity = require_bounded(request.maturity, "maturity", 100.0)?;
    let rate = request.rate.unwrap_or(0.0);
    if !rate.is_finite() || rate.abs() > 1.0 {
        return Err("rate must be finite and within [-1, 1]".to_string());
    }
    let is_call = match request.option_type.as_deref().unwrap_or("call").to_ascii_lowercase().as_str() {
        "call" => true,
        "put" => false,
        other => return Err(format!("optionType must be call or put, got {other}")),
    };

    let drift = (rate - 0.5 * vol * vol) * maturity;
    let diffusion = vol * maturity.sqrt();
    let discount = (-rate * maturity).exp();

    let mut acc = Accumulator::default();
    for _ in 0..samples {
        let z = rng.normal();
        let terminal = spot * (drift + diffusion * z).exp();
        let payoff = if is_call {
            (terminal - strike).max(0.0)
        } else {
            (strike - terminal).max(0.0)
        };
        acc.push(discount * payoff);
    }

    let analytic = black_scholes(spot, strike, rate, vol, maturity, is_call);
    let mut extra = serde_json::Map::new();
    extra.insert("optionType".to_string(), json!(if is_call { "call" } else { "put" }));
    extra.insert("spot".to_string(), json!(spot));
    extra.insert("strike".to_string(), json!(strike));
    extra.insert("blackScholes".to_string(), json!(analytic));

    Ok(finish(
        request_id,
        "option",
        samples,
        &acc,
        Some(analytic),
        extra,
        Vec::new(),
    ))
}

fn run_queue(
    request_id: String,
    samples: u64,
    rng: &mut Rng,
    request: &SimRequest,
) -> Result<SimResponse, String> {
    let lambda = require_positive(request.arrival_rate, "arrivalRate")?;
    let mu = require_positive(request.service_rate, "serviceRate")?;
    let mut warnings = Vec::new();
    let rho = lambda / mu;
    if rho >= 1.0 {
        warnings.push(format!(
            "utilisation rho={rho:.3} >= 1; the M/M/1 queue is unstable and waits grow without bound"
        ));
    }

    // Simulate `samples` customers through a single-server FIFO queue using the
    // Lindley recursion for waiting time.
    let customers = samples;
    let mut clock = 0.0;
    let mut server_free_at = 0.0;
    let mut wait_acc = Accumulator::default();
    let mut total_service = 0.0;
    for _ in 0..customers {
        let interarrival = rng.exponential(lambda);
        clock += interarrival;
        let service_start = clock.max(server_free_at);
        let wait = service_start - clock;
        let service = rng.exponential(mu);
        server_free_at = service_start + service;
        total_service += service;
        wait_acc.push(wait);
    }

    let mean_wait = wait_acc.mean; // Wq, time waiting in queue
    let mean_w = mean_wait + 1.0 / mu; // W, time in system = Wq + E[service]
    // Little's law (exact in steady state): L = lambda*W, Lq = lambda*Wq.
    let l_estimate = lambda * mean_w;
    let lq_estimate = lambda * mean_wait;
    let analytic_wq = if rho < 1.0 {
        rho / (mu - lambda)
    } else {
        f64::INFINITY
    };
    let analytic_lq = if rho < 1.0 {
        rho * rho / (1.0 - rho)
    } else {
        f64::INFINITY
    };

    let mut extra = serde_json::Map::new();
    extra.insert("utilization".to_string(), json!(rho));
    extra.insert("meanWaitInQueue".to_string(), json!(mean_wait));
    extra.insert("meanTimeInSystem".to_string(), json!(mean_w));
    extra.insert("meanNumberInSystemEstimate".to_string(), json!(l_estimate));
    extra.insert("meanNumberInQueueEstimate".to_string(), json!(lq_estimate));
    extra.insert("serverBusyFraction".to_string(), json!(total_service / clock.max(1e-9)));
    extra.insert("analyticMeanWaitInQueue".to_string(), json!(analytic_wq));
    extra.insert("analyticMeanInQueue".to_string(), json!(analytic_lq));

    // The headline estimate is the mean wait in queue, with the analytic Wq as
    // reference when the queue is stable.
    Ok(finish(
        request_id,
        "queue",
        customers,
        &wait_acc,
        if rho < 1.0 { Some(analytic_wq) } else { None },
        extra,
        warnings,
    ))
}

fn run_integrate(
    request_id: String,
    samples: u64,
    rng: &mut Rng,
    request: &SimRequest,
) -> Result<SimResponse, String> {
    let lower = request.lower.ok_or("lower bound is required")?;
    let upper = request.upper.ok_or("upper bound is required")?;
    if !lower.is_finite() || !upper.is_finite() || upper <= lower {
        return Err("require finite bounds with upper > lower".to_string());
    }
    let name = request.function.as_deref().unwrap_or("gaussian").to_ascii_lowercase();
    let (func, analytic): (fn(f64) -> f64, Option<f64>) = match name.as_str() {
        "gaussian" => (
            |x: f64| (-x * x).exp(),
            // ∫ e^{-x^2} dx has no elementary closed form; leave reference None
            // unless bounds are the classic full line, handled below.
            None,
        ),
        "sin" => (|x: f64| x.sin(), Some(lower.cos() - upper.cos())),
        "x2" | "quadratic" => (|x: f64| x * x, Some((upper.powi(3) - lower.powi(3)) / 3.0)),
        "identity" | "x" => (|x: f64| x, Some((upper * upper - lower * lower) / 2.0)),
        other => {
            return Err(format!(
                "unsupported function {other}; expected gaussian, sin, x2, or identity"
            ))
        }
    };

    let span = upper - lower;
    let mut acc = Accumulator::default();
    for _ in 0..samples {
        let x = lower + rng.uniform() * span;
        acc.push(span * func(x));
    }

    let mut extra = serde_json::Map::new();
    extra.insert("function".to_string(), json!(name));
    extra.insert("lower".to_string(), json!(lower));
    extra.insert("upper".to_string(), json!(upper));

    Ok(finish(request_id, "integrate", samples, &acc, analytic, extra, Vec::new()))
}

async fn simulate_in_background(request: SimRequest) -> Result<SimResponse, String> {
    tokio::task::spawn_blocking(move || simulate(request))
        .await
        .map_err(|error| format!("simulate task join failed: {error}"))?
}

async fn publish_result(state: &AppState, response: &SimResponse) {
    let Some(nats) = &state.nats else {
        return;
    };
    let payload = match serde_json::to_vec(&json!({
        "messageKind": "montecarlo.simulate.result",
        "schemaVersion": "montecarlo.simulate.v1",
        "source": "dd-monte-carlo-server",
        "result": response,
    })) {
        Ok(payload) => payload,
        Err(error) => {
            eprintln!("failed to encode monte-carlo result: {error}");
            return;
        }
    };
    if payload.len() > MAX_PUBLISH_BYTES {
        eprintln!(
            "monte-carlo result too large to publish: bytes={} max={MAX_PUBLISH_BYTES}",
            payload.len()
        );
        state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
        return;
    }
    if let Err(error) = nats
        .publish(state.result_subject.clone(), payload.into())
        .await
    {
        eprintln!("failed to publish monte-carlo result: {error}");
        return;
    }
    let _ = nats
        .publish(
            state.event_subject.clone(),
            json!({
                "type": "montecarlo.simulate.result",
                "source": "dd-monte-carlo-server",
                "requestId": response.request_id,
                "experiment": response.experiment,
                "estimate": response.estimate,
                "standardError": response.standard_error,
                "samples": response.samples,
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
        "service": "dd-monte-carlo-server",
        "mode": "monte-carlo-nats",
        "experiments": ["pi", "option", "queue", "integrate"],
        "atMs": now_ms(),
    }))
}

async fn metrics(State(state): State<AppState>) -> Response {
    let m = &state.metrics;
    let body = format!(
        "# HELP dd_monte_carlo_requests_total HTTP simulate requests.\n\
         # TYPE dd_monte_carlo_requests_total counter\n\
         dd_monte_carlo_requests_total {}\n\
         # HELP dd_monte_carlo_simulations_total Simulations completed.\n\
         # TYPE dd_monte_carlo_simulations_total counter\n\
         dd_monte_carlo_simulations_total {}\n\
         # HELP dd_monte_carlo_samples_total Total samples drawn.\n\
         # TYPE dd_monte_carlo_samples_total counter\n\
         dd_monte_carlo_samples_total {}\n\
         # HELP dd_monte_carlo_errors_total Simulation or message errors.\n\
         # TYPE dd_monte_carlo_errors_total counter\n\
         dd_monte_carlo_errors_total {}\n\
         # HELP dd_monte_carlo_rejected_busy_total Requests shed because the inflight cap was full.\n\
         # TYPE dd_monte_carlo_rejected_busy_total counter\n\
         dd_monte_carlo_rejected_busy_total {}\n\
         # HELP dd_monte_carlo_nats_messages_total NATS simulate requests received.\n\
         # TYPE dd_monte_carlo_nats_messages_total counter\n\
         dd_monte_carlo_nats_messages_total {}\n",
        m.requests_total.load(Ordering::Relaxed),
        m.simulations_total.load(Ordering::Relaxed),
        m.samples_total.load(Ordering::Relaxed),
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

async fn simulate_http(State(state): State<AppState>, Json(request): Json<SimRequest>) -> Response {
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
    match simulate_in_background(request).await {
        Ok(response) => {
            state.metrics.simulations_total.fetch_add(1, Ordering::Relaxed);
            state
                .metrics
                .samples_total
                .fetch_add(response.samples, Ordering::Relaxed);
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
        println!("monte-carlo nats loop disabled: NATS_URL is not configured");
        return;
    };
    println!(
        "monte-carlo nats loop starting: subject={subject} queue_group={queue_group} resultSubject={}",
        state.result_subject
    );
    loop {
        let mut subscription = match nats.queue_subscribe(subject.clone(), queue_group.clone()).await {
            Ok(subscription) => subscription,
            Err(error) => {
                eprintln!("monte-carlo subscribe failed: {error}; retrying in 5s");
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
                eprintln!(
                    "monte-carlo rejected oversize nats request: bytes={} max={MAX_NATS_PAYLOAD_BYTES}",
                    payload.len()
                );
                continue;
            }
            // Backpressure: wait for an inflight slot before taking on more work so a
            // NATS flood can't spawn unbounded sampling. NATS buffers/redelivers.
            let Ok(permit) = state.inflight.clone().acquire_owned().await else {
                continue;
            };
            let task_state = state.clone();
            tokio::spawn(async move {
                let _permit = permit;
                match serde_json::from_slice::<SimRequest>(&payload) {
                    Ok(request) => match simulate_in_background(request).await {
                        Ok(response) => {
                            task_state.metrics.simulations_total.fetch_add(1, Ordering::Relaxed);
                            task_state
                                .metrics
                                .samples_total
                                .fetch_add(response.samples, Ordering::Relaxed);
                            publish_result(&task_state, &response).await;
                        }
                        Err(error) => {
                            task_state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                            eprintln!("monte-carlo failed nats simulate: {error}");
                        }
                    },
                    Err(error) => {
                        task_state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                        eprintln!("monte-carlo invalid nats request: {error}");
                    }
                }
            });
        }
        eprintln!("monte-carlo subscription ended; re-subscribing in 5s");
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let host = env_value("HOST", "0.0.0.0");
    let port = env_value("PORT", "8134").parse::<u16>()?;
    let nats = match env::var("NATS_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        Some(url) => match async_nats::connect(&url).await {
            Ok(client) => Some(client),
            Err(error) => {
                eprintln!("monte-carlo-server NATS connect failed ({url}): {error}");
                None
            }
        },
        None => None,
    };
    let max_inflight = env_usize("MONTE_CARLO_MAX_INFLIGHT", DEFAULT_MAX_INFLIGHT);
    let state = AppState {
        nats,
        result_subject: env_value("MONTE_CARLO_RESULT_SUBJECT", MONTE_CARLO_SIMULATE_RESULTS_SUBJECT),
        event_subject: env_value("MONTE_CARLO_EVENT_SUBJECT", RUNTIME_EVENTS_SUBJECT),
        metrics: Arc::new(Metrics::default()),
        inflight: Arc::new(tokio::sync::Semaphore::new(max_inflight)),
    };
    let subject = env_value("MONTE_CARLO_SIMULATE_SUBJECT", MONTE_CARLO_SIMULATE_REQUESTS_SUBJECT);
    let queue_group = env_value("MONTE_CARLO_QUEUE_GROUP", MONTE_CARLO_SIMULATE_REQUESTS_QUEUE_GROUP);
    tokio::spawn(run_nats_loop(state.clone(), subject, queue_group));

    let app = Router::new()
        .route("/", get(healthz))
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics))
        .route("/simulate", post(simulate_http))
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .with_state(state)
        .merge(dd_runtime_config_client::router());

    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    println!("dd-monte-carlo-server listening on http://{addr}");
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

    fn base(experiment: &str) -> SimRequest {
        SimRequest {
            request_id: None,
            experiment: experiment.to_string(),
            samples: Some(200_000),
            seed: Some(7),
            spot: None,
            strike: None,
            rate: None,
            volatility: None,
            maturity: None,
            option_type: None,
            arrival_rate: None,
            service_rate: None,
            function: None,
            lower: None,
            upper: None,
        }
    }

    #[test]
    fn pi_is_close() {
        let response = simulate(base("pi")).unwrap();
        assert!((response.estimate - std::f64::consts::PI).abs() < 0.05, "got {}", response.estimate);
    }

    #[test]
    fn option_tracks_black_scholes() {
        let mut request = base("option");
        request.spot = Some(100.0);
        request.strike = Some(100.0);
        request.rate = Some(0.05);
        request.volatility = Some(0.2);
        request.maturity = Some(1.0);
        request.option_type = Some("call".to_string());
        let response = simulate(request).unwrap();
        let bs = response.analytic_reference.unwrap();
        assert!((response.estimate - bs).abs() < 0.3, "mc {} vs bs {}", response.estimate, bs);
    }

    #[test]
    fn integrate_x2_matches_closed_form() {
        let mut request = base("integrate");
        request.function = Some("x2".to_string());
        request.lower = Some(0.0);
        request.upper = Some(3.0);
        let response = simulate(request).unwrap();
        // ∫_0^3 x^2 dx = 9.
        assert!((response.estimate - 9.0).abs() < 0.2, "got {}", response.estimate);
    }

    #[test]
    fn queue_satisfies_littles_law() {
        let mut request = base("queue");
        request.arrival_rate = Some(0.5);
        request.service_rate = Some(1.0);
        request.samples = Some(200_000);
        let response = simulate(request).unwrap();
        let l = response.extra["meanNumberInSystemEstimate"].as_f64().unwrap();
        let w = response.extra["meanTimeInSystem"].as_f64().unwrap();
        // L = lambda * W must hold exactly by construction.
        assert!((l - 0.5 * w).abs() < 1e-9, "L={l} lambda*W={}", 0.5 * w);
    }

    #[test]
    fn rejects_extreme_volatility() {
        let mut request = base("option");
        request.spot = Some(100.0);
        request.strike = Some(100.0);
        request.rate = Some(0.05);
        request.volatility = Some(50.0); // beyond the 5.0 cap that keeps exp() finite
        request.maturity = Some(1.0);
        assert!(simulate(request).is_err());
    }

    #[test]
    fn queue_is_stable_for_low_load() {
        let mut request = base("queue");
        request.arrival_rate = Some(0.5);
        request.service_rate = Some(1.0);
        request.samples = Some(50_000);
        let response = simulate(request).unwrap();
        assert!(response.estimate.is_finite());
        assert!(response.estimate >= 0.0);
    }
}
