//! dd-evolution-optimizer — a distributed island-model genetic algorithm.
//!
//! Roles (selected with EVOLUTION_NODE_ROLE):
//!   master  — exposes HTTP `/optimize`, splits the population across N islands,
//!             runs `epochs` epochs, migrates elites around a ring between epochs,
//!             and aggregates the global best.
//!   island  — a JetStream worker that evolves one subpopulation per epoch and
//!             publishes the result back to the master.
//!
//! The master/slave wiring mirrors the in-house MIP solver node: jobs go out on a
//! JetStream subject consumed through a shared queue group, results come back on a
//! plain subject the master correlates by solveId. With no NATS_URL the master
//! evolves every island locally so it still works as a single pod.

mod ga;

use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::{
    extract::{DefaultBodyLimit, State},
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use dd_nats_subject_defs::{
    DD_REMOTE_EVOLUTION_STREAM_NAME, DD_REMOTE_EVOLUTION_STREAM_SUBJECTS, EVOLUTION_EVENTS_SUBJECT,
    EVOLUTION_ISLANDS_QUEUE_GROUP, EVOLUTION_JOBS_SUBJECT, EVOLUTION_RESULTS_SUBJECT,
};

use ga::{GaParams, IslandJob, IslandResult, ProblemSpec};

const SERVICE_NAME: &str = "dd-evolution-optimizer";
const MAX_HTTP_BODY_BYTES: usize = 4 * 1024 * 1024;
const MAX_NATS_PAYLOAD_BYTES: usize = 8 * 1024 * 1024;
const MAX_ISLANDS: usize = 64;
const MAX_EPOCHS: usize = 500;

#[derive(Clone, Copy, PartialEq, Eq)]
enum NodeRole {
    Master,
    Island,
}

impl NodeRole {
    fn as_str(self) -> &'static str {
        match self {
            NodeRole::Master => "master",
            NodeRole::Island => "island",
        }
    }
}

#[derive(Default)]
struct Metrics {
    http_requests_total: AtomicU64,
    optimize_requests_total: AtomicU64,
    jobs_published_total: AtomicU64,
    results_collected_total: AtomicU64,
    island_jobs_processed_total: AtomicU64,
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
}

// ---------------------------------------------------------------------------
// HTTP request / response shapes
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OptimizeRequest {
    problem: ProblemInput,
    #[serde(default)]
    islands: Option<usize>,
    #[serde(default)]
    population_per_island: Option<usize>,
    #[serde(default)]
    generations_per_epoch: Option<usize>,
    #[serde(default)]
    epochs: Option<usize>,
    #[serde(default)]
    migration_size: Option<usize>,
    #[serde(default)]
    mutation_rate: Option<f64>,
    #[serde(default)]
    mutation_scale: Option<f64>,
    #[serde(default)]
    crossover_rate: Option<f64>,
    #[serde(default)]
    elite_count: Option<usize>,
    #[serde(default)]
    tournament_size: Option<usize>,
    #[serde(default)]
    seed: Option<u64>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProblemInput {
    function: String,
    dimension: usize,
    #[serde(default)]
    lower_bound: Option<f64>,
    #[serde(default)]
    upper_bound: Option<f64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EpochSummary {
    epoch: usize,
    best_fitness: f64,
    islands_reported: usize,
    timed_out: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OptimizeResponse {
    ok: bool,
    schema_version: &'static str,
    solve_id: String,
    request_id: String,
    function: String,
    dimension: usize,
    islands: usize,
    epochs_run: usize,
    distributed: bool,
    best_fitness: f64,
    best_genome: Vec<f64>,
    total_evaluations: u64,
    elapsed_ms: f64,
    epoch_history: Vec<EpochSummary>,
    warnings: Vec<String>,
}

/// Normalized, clamped configuration derived from an `OptimizeRequest`.
struct SolveConfig {
    problem: ProblemSpec,
    islands: usize,
    population_per_island: usize,
    epochs: usize,
    migration_size: usize,
    params: GaParams,
    seed: u64,
    timeout: Duration,
}

fn normalize(request: OptimizeRequest) -> Result<SolveConfig, String> {
    if request.problem.function.trim().is_empty() {
        return Err("problem.function is required".into());
    }
    if !ga::is_known_function(request.problem.function.trim()) {
        return Err(format!(
            "unknown function `{}` (supported: {})",
            request.problem.function,
            ga::known_functions().join(", ")
        ));
    }
    let dimension = request.problem.dimension.clamp(1, ga::MAX_DIMENSION);
    let lower = request.problem.lower_bound.unwrap_or(-5.12);
    let upper = request.problem.upper_bound.unwrap_or(5.12);
    if !(lower < upper) {
        return Err("problem.lowerBound must be < problem.upperBound".into());
    }

    let islands = request.islands.unwrap_or(4).clamp(1, MAX_ISLANDS);
    let population_per_island = request
        .population_per_island
        .unwrap_or(60)
        .clamp(4, ga::MAX_POPULATION);
    let generations = request
        .generations_per_epoch
        .unwrap_or(40)
        .clamp(1, ga::MAX_GENERATIONS);
    let epochs = request.epochs.unwrap_or(8).clamp(1, MAX_EPOCHS);
    let migration_size = request
        .migration_size
        .unwrap_or((population_per_island / 10).max(1))
        .min(population_per_island / 2);
    let elite_count = request
        .elite_count
        .unwrap_or((population_per_island / 20).max(1))
        .min(population_per_island - 1);

    Ok(SolveConfig {
        problem: ProblemSpec {
            function: request.problem.function.trim().to_string(),
            dimension,
            lower_bound: lower,
            upper_bound: upper,
        },
        islands,
        population_per_island,
        epochs,
        migration_size,
        params: GaParams {
            population_size: population_per_island,
            generations,
            mutation_rate: request.mutation_rate.unwrap_or(0.15).clamp(0.0, 1.0),
            mutation_scale: request.mutation_scale.unwrap_or(0.1).clamp(0.0001, 1.0),
            crossover_rate: request.crossover_rate.unwrap_or(0.9).clamp(0.0, 1.0),
            elite_count,
            tournament_size: request.tournament_size.unwrap_or(3).clamp(1, 16),
        },
        seed: request.seed.unwrap_or_else(|| now_ms() as u64),
        timeout: Duration::from_millis(request.timeout_ms.unwrap_or(120_000).clamp(1_000, 600_000)),
    })
}

// ---------------------------------------------------------------------------
// Master orchestration
// ---------------------------------------------------------------------------

async fn run_optimization(state: &AppState, config: SolveConfig) -> OptimizeResponse {
    let started = Instant::now();
    let solve_id = format!("evo-{}", Uuid::new_v4());
    let request_id = solve_id.clone();
    let mut warnings = Vec::new();

    // Per-island carried population; epoch 0 starts empty so islands seed randomly.
    let mut populations: Vec<Vec<Vec<f64>>> = vec![Vec::new(); config.islands];
    let mut best_fitness = f64::INFINITY;
    let mut best_genome: Vec<f64> = Vec::new();
    let mut total_evaluations = 0u64;
    let mut epoch_history = Vec::new();
    let distributed = state.nats.is_some() && config.islands > 0;
    let mut epochs_run = 0usize;

    publish_event(
        state,
        "solve-started",
        json!({
            "solveId": &solve_id,
            "function": &config.problem.function,
            "islands": config.islands,
            "epochs": config.epochs,
            "distributed": distributed,
        }),
    )
    .await;

    for epoch in 0..config.epochs {
        let jobs: Vec<IslandJob> = (0..config.islands)
            .map(|island_id| IslandJob {
                solve_id: solve_id.clone(),
                request_id: request_id.clone(),
                epoch,
                island_id,
                problem: config.problem.clone(),
                params: config.params,
                // Distinct, deterministic per (epoch, island) stream.
                seed: config
                    .seed
                    .wrapping_mul(0x100_0001)
                    .wrapping_add((epoch as u64) << 8)
                    .wrapping_add(island_id as u64),
                population: std::mem::take(&mut populations[island_id]),
            })
            .collect();

        let (results, timed_out) = if distributed {
            dispatch_epoch(state, &solve_id, epoch, &jobs, config.timeout).await
        } else {
            run_epoch_locally(state, jobs).await
        };
        if timed_out {
            warnings.push(format!("epoch {epoch} timed out waiting for all islands"));
        }

        // Reassemble per-island populations and track the global best.
        let mut reported = 0usize;
        let mut epoch_best = f64::INFINITY;
        for result in &results {
            if result.island_id >= config.islands {
                continue;
            }
            reported += 1;
            total_evaluations += result.evaluations;
            populations[result.island_id] = result.population.clone();
            if result.best_fitness < epoch_best {
                epoch_best = result.best_fitness;
            }
            if result.best_fitness < best_fitness {
                best_fitness = result.best_fitness;
                best_genome = result.best_genome.clone();
            }
        }
        state
            .metrics
            .results_collected_total
            .fetch_add(reported as u64, Ordering::Relaxed);

        // Ring migration of elites into the next epoch's starting populations.
        if epoch + 1 < config.epochs {
            migrate_ring(&mut populations, config.migration_size);
        }

        epoch_history.push(EpochSummary {
            epoch,
            best_fitness: epoch_best,
            islands_reported: reported,
            timed_out,
        });
        epochs_run += 1;
        publish_event(
            state,
            "epoch-complete",
            json!({
                "solveId": &solve_id,
                "epoch": epoch,
                "bestFitness": best_fitness,
                "islandsReported": reported,
            }),
        )
        .await;
    }

    publish_event(
        state,
        "solve-finished",
        json!({"solveId": &solve_id, "bestFitness": best_fitness, "epochsRun": epochs_run}),
    )
    .await;

    OptimizeResponse {
        ok: true,
        schema_version: "evolution.optimize.v1",
        solve_id,
        request_id,
        function: config.problem.function,
        dimension: config.problem.dimension,
        islands: config.islands,
        epochs_run,
        distributed,
        best_fitness,
        best_genome,
        total_evaluations,
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        epoch_history,
        warnings,
    }
}

/// Publish one job per island and collect island results for this epoch, keyed by
/// solveId + epoch, until every island reports or the deadline passes.
async fn dispatch_epoch(
    state: &AppState,
    solve_id: &str,
    epoch: usize,
    jobs: &[IslandJob],
    timeout: Duration,
) -> (Vec<IslandResult>, bool) {
    let Some(nats) = state.nats.clone() else {
        return (Vec::new(), true);
    };
    let mut subscription = match nats.subscribe(state.results_subject.clone()).await {
        Ok(subscription) => subscription,
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            eprintln!("{SERVICE_NAME} epoch result subscribe failed: {error}");
            return (Vec::new(), true);
        }
    };

    for job in jobs {
        match serde_json::to_vec(job) {
            Ok(payload) => {
                if let Err(error) = nats
                    .publish(state.jobs_subject.clone(), payload.into())
                    .await
                {
                    state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                    eprintln!("{SERVICE_NAME} job publish failed: {error}");
                } else {
                    state
                        .metrics
                        .jobs_published_total
                        .fetch_add(1, Ordering::Relaxed);
                }
            }
            Err(error) => eprintln!("{SERVICE_NAME} job serialize failed: {error}"),
        }
    }
    let _ = nats.flush().await;

    let deadline = Instant::now() + timeout;
    let mut results = Vec::new();
    let mut timed_out = false;
    while results.len() < jobs.len() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            timed_out = true;
            break;
        }
        match tokio::time::timeout(remaining, subscription.next()).await {
            Ok(Some(message)) => {
                if let Ok(result) = serde_json::from_slice::<IslandResult>(&message.payload) {
                    if result.solve_id == solve_id && result.epoch == epoch {
                        results.push(result);
                    }
                }
            }
            Ok(None) => {
                timed_out = true;
                break;
            }
            Err(_) => {
                timed_out = true;
                break;
            }
        }
    }
    (results, timed_out)
}

/// Local-fallback path: evolve every island on the runtime's blocking pool.
async fn run_epoch_locally(state: &AppState, jobs: Vec<IslandJob>) -> (Vec<IslandResult>, bool) {
    let node_id = state.node_id.clone();
    let mut results = Vec::with_capacity(jobs.len());
    for job in jobs {
        let node = node_id.clone();
        match tokio::task::spawn_blocking(move || job.run(&node)).await {
            Ok(result) => results.push(result),
            Err(error) => {
                state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                eprintln!("{SERVICE_NAME} local island task failed: {error}");
            }
        }
    }
    (results, false)
}

/// Move the best `count` individuals of each island into the next island on a ring
/// (island i donates to i+1), overwriting the receiver's worst members. Inputs are
/// fitness-sorted ascending, so the best are at the front and the worst at the back.
fn migrate_ring(populations: &mut [Vec<Vec<f64>>], count: usize) {
    let n = populations.len();
    if n < 2 || count == 0 {
        return;
    }
    let migrants: Vec<Vec<Vec<f64>>> = populations
        .iter()
        .map(|pop| pop.iter().take(count).cloned().collect())
        .collect();
    for receiver in 0..n {
        let donor = (receiver + n - 1) % n;
        let incoming = &migrants[donor];
        let pop = &mut populations[receiver];
        let len = pop.len();
        for (offset, individual) in incoming.iter().enumerate() {
            if offset < len {
                pop[len - 1 - offset] = individual.clone();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Island worker (JetStream consumer)
// ---------------------------------------------------------------------------

async fn run_island(state: AppState) -> Result<(), Box<dyn Error + Send + Sync>> {
    let Some(nats) = state.nats.clone() else {
        eprintln!("{SERVICE_NAME} island role requires NATS_URL");
        return Ok(());
    };
    let jetstream = async_nats::jetstream::new(nats.clone());
    let stream = jetstream
        .get_or_create_stream(async_nats::jetstream::stream::Config {
            name: DD_REMOTE_EVOLUTION_STREAM_NAME.to_string(),
            subjects: DD_REMOTE_EVOLUTION_STREAM_SUBJECTS
                .iter()
                .map(|subject| subject.to_string())
                .collect(),
            retention: async_nats::jetstream::stream::RetentionPolicy::Limits,
            max_age: Duration::from_secs(60 * 60 * 24),
            max_message_size: MAX_NATS_PAYLOAD_BYTES as i32,
            ..Default::default()
        })
        .await?;
    let consumer_name = env_value("EVOLUTION_NATS_CONSUMER", EVOLUTION_ISLANDS_QUEUE_GROUP);
    let consumer = stream
        .get_or_create_consumer::<async_nats::jetstream::consumer::pull::Config>(
            &consumer_name,
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some(consumer_name.clone()),
                filter_subject: state.jobs_subject.clone(),
                ack_wait: Duration::from_secs(env_u64("EVOLUTION_ACK_WAIT_SECONDS", 600)),
                max_ack_pending: env_u64("EVOLUTION_MAX_ACK_PENDING", 16) as i64,
                max_deliver: 4,
                ..Default::default()
            },
        )
        .await?;

    println!("{SERVICE_NAME} island worker ready: consumer={consumer_name} jobs={}", state.jobs_subject);
    publish_event(&state, "island-started", json!({"consumer": consumer_name})).await;

    let mut messages = consumer.messages().await?;
    while let Some(message) = messages.next().await {
        let message = match message {
            Ok(message) => message,
            Err(error) => {
                eprintln!("{SERVICE_NAME} island message fetch failed: {error}");
                continue;
            }
        };
        let job = match serde_json::from_slice::<IslandJob>(&message.payload) {
            Ok(job) => job,
            Err(error) => {
                eprintln!("{SERVICE_NAME} invalid island job: {error}");
                let _ = message.ack().await;
                continue;
            }
        };
        let node = state.node_id.clone();
        let result = match tokio::task::spawn_blocking(move || job.run(&node)).await {
            Ok(result) => result,
            Err(error) => {
                state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                eprintln!("{SERVICE_NAME} island evolve task failed: {error}");
                let _ = message
                    .ack_with(async_nats::jetstream::AckKind::Nak(Some(Duration::from_secs(5))))
                    .await;
                continue;
            }
        };
        match serde_json::to_vec(&result) {
            Ok(payload) => {
                if let Err(error) = nats
                    .publish(state.results_subject.clone(), payload.into())
                    .await
                {
                    eprintln!("{SERVICE_NAME} island result publish failed: {error}");
                }
            }
            Err(error) => eprintln!("{SERVICE_NAME} island result serialize failed: {error}"),
        }
        state
            .metrics
            .island_jobs_processed_total
            .fetch_add(1, Ordering::Relaxed);
        if let Err(error) = message.ack().await {
            eprintln!("{SERVICE_NAME} island ack failed: {error}");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// HTTP handlers
// ---------------------------------------------------------------------------

async fn optimize_http(
    State(state): State<AppState>,
    Json(request): Json<OptimizeRequest>,
) -> Response {
    state.metrics.http_requests_total.fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .optimize_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if state.role != NodeRole::Master {
        return error_response(
            axum::http::StatusCode::BAD_REQUEST,
            "this node runs the island role; submit /optimize to the master",
        );
    }
    let config = match normalize(request) {
        Ok(config) => config,
        Err(message) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            return error_response(axum::http::StatusCode::BAD_REQUEST, &message);
        }
    };
    let response = run_optimization(&state, config).await;
    Json(response).into_response()
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

async fn info(State(state): State<AppState>) -> impl IntoResponse {
    state.metrics.http_requests_total.fetch_add(1, Ordering::Relaxed);
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "role": state.role.as_str(),
        "functions": ga::known_functions(),
        "limits": {
            "maxIslands": MAX_ISLANDS,
            "maxEpochs": MAX_EPOCHS,
            "maxDimension": ga::MAX_DIMENSION,
            "maxPopulation": ga::MAX_POPULATION,
            "maxGenerations": ga::MAX_GENERATIONS,
        },
        "post": {
            "/optimize": "submit { problem: { function, dimension, lowerBound?, upperBound? }, islands?, populationPerIsland?, generationsPerEpoch?, epochs?, migrationSize?, mutationRate?, crossoverRate?, seed?, timeoutMs? }"
        }
    }))
}

async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    let m = &state.metrics;
    let body = format!(
        concat!(
            "# HELP dd_evolution_http_requests_total HTTP requests served.\n",
            "# TYPE dd_evolution_http_requests_total counter\n",
            "dd_evolution_http_requests_total {}\n",
            "# HELP dd_evolution_optimize_requests_total Optimize requests received.\n",
            "# TYPE dd_evolution_optimize_requests_total counter\n",
            "dd_evolution_optimize_requests_total {}\n",
            "# HELP dd_evolution_jobs_published_total Island jobs published to NATS.\n",
            "# TYPE dd_evolution_jobs_published_total counter\n",
            "dd_evolution_jobs_published_total {}\n",
            "# HELP dd_evolution_results_collected_total Island results aggregated by the master.\n",
            "# TYPE dd_evolution_results_collected_total counter\n",
            "dd_evolution_results_collected_total {}\n",
            "# HELP dd_evolution_island_jobs_processed_total Island jobs evolved by this worker.\n",
            "# TYPE dd_evolution_island_jobs_processed_total counter\n",
            "dd_evolution_island_jobs_processed_total {}\n",
            "# HELP dd_evolution_errors_total Errors across HTTP and NATS paths.\n",
            "# TYPE dd_evolution_errors_total counter\n",
            "dd_evolution_errors_total {}\n",
        ),
        m.http_requests_total.load(Ordering::Relaxed),
        m.optimize_requests_total.load(Ordering::Relaxed),
        m.jobs_published_total.load(Ordering::Relaxed),
        m.results_collected_total.load(Ordering::Relaxed),
        m.island_jobs_processed_total.load(Ordering::Relaxed),
        m.errors_total.load(Ordering::Relaxed),
    );
    (
        [(axum::http::header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        body,
    )
}

type Response = axum::response::Response;

fn error_response(status: axum::http::StatusCode, message: &str) -> Response {
    (status, Json(json!({"ok": false, "error": message}))).into_response()
}

async fn publish_event(state: &AppState, event_name: &str, payload: Value) {
    let Some(nats) = &state.nats else {
        return;
    };
    let event = json!({
        "schema": "dd.evolution.event.v1",
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let host = env_value("HOST", "0.0.0.0");
    let port = env_value("PORT", "8131").parse::<u16>()?;
    let role = match env_value("EVOLUTION_NODE_ROLE", "master").to_ascii_lowercase().as_str() {
        "island" | "slave" | "worker" => NodeRole::Island,
        _ => NodeRole::Master,
    };
    let node_id = env::var("HOSTNAME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("evo-{}", Uuid::new_v4()));

    let nats = match env::var("NATS_URL").ok().filter(|v| !v.trim().is_empty()) {
        Some(url) => match async_nats::connect(&url).await {
            Ok(client) => {
                println!("{SERVICE_NAME} connected to NATS at {url}");
                Some(client)
            }
            Err(error) => {
                eprintln!("{SERVICE_NAME} NATS connect failed ({error}); continuing without it");
                None
            }
        },
        None => None,
    };

    let state = AppState {
        role,
        node_id,
        nats,
        jobs_subject: env_value("EVOLUTION_JOBS_SUBJECT", EVOLUTION_JOBS_SUBJECT),
        results_subject: env_value("EVOLUTION_RESULTS_SUBJECT", EVOLUTION_RESULTS_SUBJECT),
        events_subject: env_value("EVOLUTION_EVENTS_SUBJECT", EVOLUTION_EVENTS_SUBJECT),
        metrics: Arc::new(Metrics::default()),
    };

    if role == NodeRole::Island {
        let island_state = state.clone();
        tokio::spawn(async move {
            if let Err(error) = run_island(island_state).await {
                eprintln!("{SERVICE_NAME} island loop exited: {error}");
            }
        });
    }

    let app = Router::new()
        .route("/", get(info))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
        .route("/optimize", post(optimize_http))
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .with_state(state);

    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    println!("{SERVICE_NAME} ({}) listening on http://{addr}", role.as_str());
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
