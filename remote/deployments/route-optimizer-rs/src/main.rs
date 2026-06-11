// dd-route-optimizer
//
// TSP and capacitated VRP-with-time-windows over HTTP and NATS.
//
//   * TSP: nearest-neighbour construction from the depot followed by 2-opt
//     local search on the closed tour.
//   * VRP: sequential greedy insertion — each vehicle repeatedly takes the
//     nearest time-window-and-capacity-feasible customer (waiting until the
//     customer's ready time when it arrives early), then returns to the depot.
//
// Distances are Euclidean over (x, y) by default; an explicit distance matrix
// may be supplied instead. Travel time equals distance unless an explicit
// matrix sets it; service times add to arrival. Complements mdp-optimizer with
// a concrete combinatorial-routing demo that renders nicely.

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
    ROUTE_OPTIMIZE_REQUESTS_QUEUE_GROUP, ROUTE_OPTIMIZE_REQUESTS_SUBJECT,
    ROUTE_OPTIMIZE_RESULTS_SUBJECT, RUNTIME_EVENTS_SUBJECT,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;

const MAX_HTTP_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_NATS_PAYLOAD_BYTES: usize = 2 * 1024 * 1024;
const MAX_STOPS: usize = 1_000;
const MAX_TWO_OPT_PASSES: usize = 60;
/// 2-opt is O(n^2) per pass; above this many stops it is skipped (the
/// nearest-neighbour tour is still returned) to bound per-request CPU.
const MAX_TWO_OPT_STOPS: usize = 600;
const DEFAULT_MAX_INFLIGHT: usize = 16;
/// Skip publishing a result larger than this (NATS default max_payload is ~1 MiB).
const MAX_PUBLISH_BYTES: usize = 900_000;

#[derive(Clone)]
struct AppState {
    nats: Option<async_nats::Client>,
    result_subject: String,
    event_subject: String,
    metrics: Arc<Metrics>,
    /// Bounds concurrent optimizations so a request/NATS flood cannot spawn
    /// unbounded CPU-heavy work.
    inflight: Arc<tokio::sync::Semaphore>,
}

#[derive(Default)]
struct Metrics {
    requests_total: AtomicU64,
    optimizations_total: AtomicU64,
    feasible_total: AtomicU64,
    errors_total: AtomicU64,
    rejected_busy_total: AtomicU64,
    nats_messages_total: AtomicU64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RouteRequest {
    request_id: Option<String>,
    /// "tsp" (single tour over all stops) or "vrp" (multi-vehicle).
    mode: Option<String>,
    depot: Option<Point>,
    stops: Vec<StopInput>,
    vehicles: Option<u32>,
    vehicle_capacity: Option<f64>,
    /// Optional explicit cost matrix over [depot, stops...] (size n+1).
    distance_matrix: Option<Vec<Vec<f64>>>,
    two_opt: Option<bool>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Point {
    x: f64,
    y: f64,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct StopInput {
    id: String,
    x: Option<f64>,
    y: Option<f64>,
    demand: Option<f64>,
    ready: Option<f64>,
    due: Option<f64>,
    service: Option<f64>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RouteResponse {
    ok: bool,
    request_id: String,
    mode: String,
    feasible: bool,
    total_distance: f64,
    vehicles_used: usize,
    served: usize,
    unserved: Vec<String>,
    routes: Vec<VehicleRoute>,
    warnings: Vec<String>,
    generated_at_ms: u128,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct VehicleRoute {
    vehicle: usize,
    stops: Vec<RouteStop>,
    distance: f64,
    load: f64,
    finish_time: f64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RouteStop {
    id: String,
    arrival: f64,
    departure: f64,
    wait: f64,
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

struct Problem {
    /// node 0 is the depot; nodes 1..=n are stops.
    ids: Vec<String>,
    demand: Vec<f64>,
    ready: Vec<f64>,
    due: Vec<f64>,
    service: Vec<f64>,
    dist: Vec<Vec<f64>>,
}

fn build_problem(request: &RouteRequest) -> Result<Problem, String> {
    if request.stops.is_empty() {
        return Err("stops must not be empty".to_string());
    }
    if request.stops.len() > MAX_STOPS {
        return Err(format!("too many stops; max {MAX_STOPS}"));
    }
    let n = request.stops.len();

    let mut ids = Vec::with_capacity(n + 1);
    let mut demand = Vec::with_capacity(n + 1);
    let mut ready = Vec::with_capacity(n + 1);
    let mut due = Vec::with_capacity(n + 1);
    let mut service = Vec::with_capacity(n + 1);

    ids.push("depot".to_string());
    demand.push(0.0);
    ready.push(0.0);
    due.push(f64::INFINITY);
    service.push(0.0);

    let mut seen = HashMap::new();
    for stop in &request.stops {
        if seen.insert(stop.id.clone(), ()).is_some() {
            return Err(format!("duplicate stop id {}", stop.id));
        }
        let d = stop.demand.unwrap_or(0.0);
        if d < 0.0 || !d.is_finite() {
            return Err(format!("stop {} demand must be finite and non-negative", stop.id));
        }
        let r = stop.ready.unwrap_or(0.0);
        let u = stop.due.unwrap_or(f64::INFINITY);
        if u < r {
            return Err(format!("stop {} due must be >= ready", stop.id));
        }
        ids.push(stop.id.clone());
        demand.push(d);
        ready.push(r);
        due.push(u);
        service.push(stop.service.unwrap_or(0.0).max(0.0));
    }

    let dist = build_distance(request, n)?;
    Ok(Problem {
        ids,
        demand,
        ready,
        due,
        service,
        dist,
    })
}

fn build_distance(request: &RouteRequest, n: usize) -> Result<Vec<Vec<f64>>, String> {
    if let Some(matrix) = &request.distance_matrix {
        if matrix.len() != n + 1 || matrix.iter().any(|row| row.len() != n + 1) {
            return Err(format!(
                "distanceMatrix must be square of size {} (depot + stops)",
                n + 1
            ));
        }
        if matrix.iter().flatten().any(|v| !v.is_finite() || *v < 0.0 || *v > 1e12) {
            return Err("distanceMatrix entries must be finite and within [0, 1e12]".to_string());
        }
        return Ok(matrix.clone());
    }

    // Euclidean over coordinates; depot defaults to origin if omitted.
    let depot = request.depot.clone().unwrap_or(Point { x: 0.0, y: 0.0 });
    let mut coords = Vec::with_capacity(n + 1);
    coords.push((depot.x, depot.y));
    for stop in &request.stops {
        match (stop.x, stop.y) {
            (Some(x), Some(y)) => {
                if !x.is_finite() || !y.is_finite() || x.abs() > 1e9 || y.abs() > 1e9 {
                    return Err(format!("stop {} coordinates must be finite and within ±1e9", stop.id));
                }
                coords.push((x, y));
            }
            _ => {
                return Err(format!(
                    "stop {} needs x and y when no distanceMatrix is provided",
                    stop.id
                ))
            }
        }
    }
    let size = n + 1;
    let mut dist = vec![vec![0.0; size]; size];
    for i in 0..size {
        for j in 0..size {
            let dx = coords[i].0 - coords[j].0;
            let dy = coords[i].1 - coords[j].1;
            dist[i][j] = (dx * dx + dy * dy).sqrt();
        }
    }
    Ok(dist)
}

fn optimize(request: RouteRequest) -> Result<RouteResponse, String> {
    let request_id = request
        .request_id
        .clone()
        .unwrap_or_else(|| format!("route-{}", now_ms()));
    let mode = request
        .mode
        .clone()
        .unwrap_or_else(|| "vrp".to_string())
        .to_ascii_lowercase();
    let problem = build_problem(&request)?;

    match mode.as_str() {
        "tsp" => solve_tsp(&problem, &request, request_id),
        "vrp" => solve_vrp(&problem, &request, request_id),
        other => Err(format!("unsupported mode {other}; expected tsp or vrp")),
    }
}

fn solve_tsp(problem: &Problem, request: &RouteRequest, request_id: String) -> Result<RouteResponse, String> {
    let n = problem.ids.len() - 1;
    // Nearest-neighbour tour starting/ending at the depot (node 0).
    let mut visited = vec![false; n + 1];
    let mut tour = vec![0usize];
    visited[0] = true;
    let mut current = 0;
    for _ in 0..n {
        let mut best = None;
        let mut best_dist = f64::INFINITY;
        #[allow(clippy::needless_range_loop)]
        for candidate in 1..=n {
            if !visited[candidate] && problem.dist[current][candidate] < best_dist {
                best_dist = problem.dist[current][candidate];
                best = Some(candidate);
            }
        }
        let next = best.expect("unvisited node exists");
        visited[next] = true;
        tour.push(next);
        current = next;
    }

    let two_opt_requested = request.two_opt.unwrap_or(true);
    let two_opt_applied = two_opt_requested && n <= MAX_TWO_OPT_STOPS;
    if two_opt_applied {
        two_opt(&mut tour, &problem.dist);
    }

    // Build the route output; TSP ignores time windows but still reports arrivals.
    let mut stops = Vec::new();
    let mut distance = 0.0;
    let mut time = 0.0;
    let mut warnings = Vec::new();
    if two_opt_requested && !two_opt_applied {
        warnings.push(format!(
            "2-opt skipped: {n} stops exceeds the {MAX_TWO_OPT_STOPS} cap; returning the nearest-neighbour tour"
        ));
    }
    for window in tour.windows(2) {
        let (from, to) = (window[0], window[1]);
        distance += problem.dist[from][to];
        time += problem.dist[from][to];
        let arrival = time;
        let wait = (problem.ready[to] - arrival).max(0.0);
        time = arrival + wait + problem.service[to];
        if problem.due[to].is_finite() && arrival > problem.due[to] {
            warnings.push(format!(
                "stop {} reached at {:.2} after its due {:.2} (TSP mode ignores time windows)",
                problem.ids[to], arrival, problem.due[to]
            ));
        }
        stops.push(RouteStop {
            id: problem.ids[to].clone(),
            arrival,
            departure: time,
            wait,
        });
    }
    // Close the tour back to the depot.
    let last = *tour.last().unwrap();
    distance += problem.dist[last][0];
    let finish_time = time + problem.dist[last][0];

    let route = VehicleRoute {
        vehicle: 0,
        stops,
        distance,
        load: problem.demand.iter().sum(),
        finish_time,
    };

    Ok(RouteResponse {
        ok: true,
        request_id,
        mode: "tsp".to_string(),
        feasible: true,
        total_distance: distance,
        vehicles_used: 1,
        served: n,
        unserved: Vec::new(),
        routes: vec![route],
        warnings,
        generated_at_ms: now_ms(),
    })
}

fn two_opt(tour: &mut [usize], dist: &[Vec<f64>]) {
    // Closed tour: tour[0] is depot, returns to depot implicitly. Improve edges
    // by reversing segments while any swap shortens the closed length.
    let n = tour.len();
    if n < 4 {
        return;
    }
    for _ in 0..MAX_TWO_OPT_PASSES {
        let mut improved = false;
        for i in 1..n - 1 {
            for k in i + 1..n {
                let a = tour[i - 1];
                let b = tour[i];
                let c = tour[k];
                let d = if k + 1 < n { tour[k + 1] } else { tour[0] };
                let before = dist[a][b] + dist[c][d];
                let after = dist[a][c] + dist[b][d];
                if after + 1e-9 < before {
                    tour[i..=k].reverse();
                    improved = true;
                }
            }
        }
        if !improved {
            break;
        }
    }
}

fn solve_vrp(problem: &Problem, request: &RouteRequest, request_id: String) -> Result<RouteResponse, String> {
    let n = problem.ids.len() - 1;
    let vehicles = request.vehicles.unwrap_or(1).max(1) as usize;
    let capacity = request.vehicle_capacity.unwrap_or(f64::INFINITY);
    if capacity <= 0.0 {
        return Err("vehicleCapacity must be positive when provided".to_string());
    }

    let mut served = vec![false; n + 1];
    served[0] = true;
    let mut routes = Vec::new();
    let mut total_distance = 0.0;
    let mut warnings = Vec::new();

    for vehicle in 0..vehicles {
        if served.iter().skip(1).all(|&s| s) {
            break;
        }
        let mut current = 0usize;
        let mut time = 0.0;
        let mut load = 0.0;
        let mut distance = 0.0;
        let mut stops = Vec::new();

        loop {
            // Pick the nearest feasible unserved customer.
            let mut best = None;
            let mut best_dist = f64::INFINITY;
            #[allow(clippy::needless_range_loop)]
            for candidate in 1..=n {
                if served[candidate] {
                    continue;
                }
                if load + problem.demand[candidate] > capacity {
                    continue;
                }
                let travel = problem.dist[current][candidate];
                let arrival = time + travel;
                if problem.due[candidate].is_finite() && arrival > problem.due[candidate] {
                    continue;
                }
                if travel < best_dist {
                    best_dist = travel;
                    best = Some(candidate);
                }
            }

            let Some(next) = best else {
                break;
            };
            let travel = problem.dist[current][next];
            let arrival = time + travel;
            let wait = (problem.ready[next] - arrival).max(0.0);
            let departure = arrival + wait + problem.service[next];
            distance += travel;
            load += problem.demand[next];
            time = departure;
            served[next] = true;
            stops.push(RouteStop {
                id: problem.ids[next].clone(),
                arrival,
                departure,
                wait,
            });
            current = next;
        }

        if stops.is_empty() {
            continue;
        }
        // Return to depot.
        distance += problem.dist[current][0];
        let finish_time = time + problem.dist[current][0];
        total_distance += distance;
        routes.push(VehicleRoute {
            vehicle,
            stops,
            distance,
            load,
            finish_time,
        });
    }

    let unserved: Vec<String> = (1..=n)
        .filter(|&i| !served[i])
        .map(|i| problem.ids[i].clone())
        .collect();
    let feasible = unserved.is_empty();
    if !feasible {
        warnings.push(format!(
            "{} stop(s) unserved: increase vehicles, capacity, or widen time windows",
            unserved.len()
        ));
    }

    Ok(RouteResponse {
        ok: true,
        request_id,
        mode: "vrp".to_string(),
        feasible,
        total_distance,
        vehicles_used: routes.len(),
        served: n - unserved.len(),
        unserved,
        routes,
        warnings,
        generated_at_ms: now_ms(),
    })
}

async fn optimize_in_background(request: RouteRequest) -> Result<RouteResponse, String> {
    tokio::task::spawn_blocking(move || optimize(request))
        .await
        .map_err(|error| format!("optimize task join failed: {error}"))?
}

async fn publish_result(state: &AppState, response: &RouteResponse) {
    let Some(nats) = &state.nats else {
        return;
    };
    let payload = match serde_json::to_vec(&json!({
        "messageKind": "route.optimize.result",
        "schemaVersion": "route.optimize.v1",
        "source": "dd-route-optimizer",
        "result": response,
    })) {
        Ok(payload) => payload,
        Err(error) => {
            eprintln!("failed to encode route result: {error}");
            return;
        }
    };
    if payload.len() > MAX_PUBLISH_BYTES {
        eprintln!(
            "route result too large to publish: bytes={} max={MAX_PUBLISH_BYTES}",
            payload.len()
        );
        state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
        return;
    }
    if let Err(error) = nats
        .publish(state.result_subject.clone(), payload.into())
        .await
    {
        eprintln!("failed to publish route result: {error}");
        return;
    }
    let _ = nats
        .publish(
            state.event_subject.clone(),
            json!({
                "type": "route.optimize.result",
                "source": "dd-route-optimizer",
                "requestId": response.request_id,
                "mode": response.mode,
                "totalDistance": response.total_distance,
                "feasible": response.feasible,
                "vehiclesUsed": response.vehicles_used,
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
        "service": "dd-route-optimizer",
        "mode": "tsp-vrp-tw-nats",
        "atMs": now_ms(),
    }))
}

async fn metrics(State(state): State<AppState>) -> Response {
    let m = &state.metrics;
    let body = format!(
        "# HELP dd_route_optimizer_requests_total HTTP optimize requests.\n\
         # TYPE dd_route_optimizer_requests_total counter\n\
         dd_route_optimizer_requests_total {}\n\
         # HELP dd_route_optimizer_optimizations_total Optimizations completed.\n\
         # TYPE dd_route_optimizer_optimizations_total counter\n\
         dd_route_optimizer_optimizations_total {}\n\
         # HELP dd_route_optimizer_feasible_total Feasible (all stops served) outcomes.\n\
         # TYPE dd_route_optimizer_feasible_total counter\n\
         dd_route_optimizer_feasible_total {}\n\
         # HELP dd_route_optimizer_errors_total Optimize or message errors.\n\
         # TYPE dd_route_optimizer_errors_total counter\n\
         dd_route_optimizer_errors_total {}\n\
         # HELP dd_route_optimizer_rejected_busy_total Requests shed because the inflight cap was full.\n\
         # TYPE dd_route_optimizer_rejected_busy_total counter\n\
         dd_route_optimizer_rejected_busy_total {}\n\
         # HELP dd_route_optimizer_nats_messages_total NATS optimize requests received.\n\
         # TYPE dd_route_optimizer_nats_messages_total counter\n\
         dd_route_optimizer_nats_messages_total {}\n",
        m.requests_total.load(Ordering::Relaxed),
        m.optimizations_total.load(Ordering::Relaxed),
        m.feasible_total.load(Ordering::Relaxed),
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

async fn optimize_http(State(state): State<AppState>, Json(request): Json<RouteRequest>) -> Response {
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
    match optimize_in_background(request).await {
        Ok(response) => {
            state.metrics.optimizations_total.fetch_add(1, Ordering::Relaxed);
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
        println!("route-optimizer nats loop disabled: NATS_URL is not configured");
        return;
    };
    println!(
        "route-optimizer nats loop starting: subject={subject} queue_group={queue_group} resultSubject={}",
        state.result_subject
    );
    let mut subscription = match nats.queue_subscribe(subject, queue_group).await {
        Ok(subscription) => subscription,
        Err(error) => {
            eprintln!("route-optimizer nats subscribe failed: {error}");
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
                "route-optimizer rejected oversize nats request: bytes={} max={MAX_NATS_PAYLOAD_BYTES}",
                payload.len()
            );
            continue;
        }
        // Backpressure: wait for an inflight slot before taking on more work so a
        // NATS flood can't spawn unbounded optimizations. NATS buffers/redelivers.
        let Ok(permit) = state.inflight.clone().acquire_owned().await else {
            continue;
        };
        let task_state = state.clone();
        tokio::spawn(async move {
            let _permit = permit;
            match serde_json::from_slice::<RouteRequest>(&payload) {
                Ok(request) => match optimize_in_background(request).await {
                    Ok(response) => {
                        task_state.metrics.optimizations_total.fetch_add(1, Ordering::Relaxed);
                        if response.feasible {
                            task_state.metrics.feasible_total.fetch_add(1, Ordering::Relaxed);
                        }
                        publish_result(&task_state, &response).await;
                    }
                    Err(error) => {
                        task_state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                        eprintln!("route-optimizer failed nats optimize: {error}");
                    }
                },
                Err(error) => {
                    task_state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                    eprintln!("route-optimizer invalid nats request: {error}");
                }
            }
        });
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let host = env_value("HOST", "0.0.0.0");
    let port = env_value("PORT", "8132").parse::<u16>()?;
    let nats = match env::var("NATS_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        Some(url) => Some(async_nats::connect(url).await?),
        None => None,
    };
    let max_inflight = env_usize("ROUTE_MAX_INFLIGHT", DEFAULT_MAX_INFLIGHT);
    let state = AppState {
        nats,
        result_subject: env_value("ROUTE_RESULT_SUBJECT", ROUTE_OPTIMIZE_RESULTS_SUBJECT),
        event_subject: env_value("ROUTE_EVENT_SUBJECT", RUNTIME_EVENTS_SUBJECT),
        metrics: Arc::new(Metrics::default()),
        inflight: Arc::new(tokio::sync::Semaphore::new(max_inflight)),
    };
    let subject = env_value("ROUTE_OPTIMIZE_SUBJECT", ROUTE_OPTIMIZE_REQUESTS_SUBJECT);
    let queue_group = env_value("ROUTE_QUEUE_GROUP", ROUTE_OPTIMIZE_REQUESTS_QUEUE_GROUP);
    tokio::spawn(run_nats_loop(state.clone(), subject, queue_group));

    let app = Router::new()
        .route("/", get(healthz))
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics))
        .route("/optimize", post(optimize_http))
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .with_state(state)
        .merge(dd_runtime_config_client::router());

    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    println!("dd-route-optimizer listening on http://{addr}");
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

    fn stop(id: &str, x: f64, y: f64) -> StopInput {
        StopInput {
            id: id.to_string(),
            x: Some(x),
            y: Some(y),
            demand: None,
            ready: None,
            due: None,
            service: None,
        }
    }

    #[test]
    fn tsp_solves_square() {
        // Unit square: optimal closed tour length is 4.
        let request = RouteRequest {
            request_id: None,
            mode: Some("tsp".to_string()),
            depot: Some(Point { x: 0.0, y: 0.0 }),
            stops: vec![stop("a", 0.0, 1.0), stop("b", 1.0, 1.0), stop("c", 1.0, 0.0)],
            vehicles: None,
            vehicle_capacity: None,
            distance_matrix: None,
            two_opt: Some(true),
        };
        let response = optimize(request).unwrap();
        assert!(response.feasible);
        assert!((response.total_distance - 4.0).abs() < 1e-6, "got {}", response.total_distance);
    }

    #[test]
    fn vrp_respects_capacity() {
        let mut a = stop("a", 1.0, 0.0);
        a.demand = Some(6.0);
        let mut b = stop("b", 2.0, 0.0);
        b.demand = Some(6.0);
        let request = RouteRequest {
            request_id: None,
            mode: Some("vrp".to_string()),
            depot: Some(Point { x: 0.0, y: 0.0 }),
            stops: vec![a, b],
            vehicles: Some(2),
            vehicle_capacity: Some(10.0),
            distance_matrix: None,
            two_opt: None,
        };
        let response = optimize(request).unwrap();
        assert!(response.feasible);
        // Each vehicle can carry only one stop (6+6 > 10).
        assert_eq!(response.vehicles_used, 2);
    }

    #[test]
    fn vrp_reports_unserved_when_time_window_infeasible() {
        let mut a = stop("a", 100.0, 0.0);
        a.due = Some(1.0); // unreachable by time 1 from depot at distance 100
        let request = RouteRequest {
            request_id: None,
            mode: Some("vrp".to_string()),
            depot: Some(Point { x: 0.0, y: 0.0 }),
            stops: vec![a],
            vehicles: Some(1),
            vehicle_capacity: None,
            distance_matrix: None,
            two_opt: None,
        };
        let response = optimize(request).unwrap();
        assert!(!response.feasible);
        assert_eq!(response.unserved, vec!["a".to_string()]);
    }
}
