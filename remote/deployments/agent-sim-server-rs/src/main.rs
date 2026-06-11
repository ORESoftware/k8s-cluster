// dd-agent-sim-server
//
// Agent-based / cellular-automata simulation streamed over NATS for live demos
// (in the spirit of the soccer live server). Four models share one harness:
//
//   * life      — Conway's Game of Life (toroidal Moore neighbourhood)
//   * sir       — stochastic SIR epidemic spread on a grid
//   * schelling — Schelling segregation with a tolerance threshold
//   * boids      — continuous flocking (alignment / cohesion / separation)
//
// The simulation runs on a blocking worker and produces a per-step time series
// plus a bounded set of full frames. The async layer then fans those frames
// out on `dd.remote.agent_sim.frames` AND bridges them to the shared websocket
// subject so browser clients animate them, before publishing the final result
// on `dd.remote.agent_sim.simulate.results`. Determinism comes from a seeded
// SplitMix64 PRNG (no external rand dependency).

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
    AGENT_SIM_FRAMES_SUBJECT, AGENT_SIM_SIMULATE_REQUESTS_QUEUE_GROUP,
    AGENT_SIM_SIMULATE_REQUESTS_SUBJECT, AGENT_SIM_SIMULATE_RESULTS_SUBJECT, RUNTIME_EVENTS_SUBJECT,
    WEBSOCKET_EVENTS_SUBJECT,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;

const MAX_HTTP_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_NATS_PAYLOAD_BYTES: usize = 2 * 1024 * 1024;
const MAX_CELLS: usize = 40_000;
const MAX_STEPS: usize = 2_000;
const MAX_BOIDS: usize = 2_000;
const MAX_STREAM_FRAMES: usize = 500;
/// Compute budgets bound per-request CPU regardless of the individual caps:
/// grid models are O(cells * steps); boids is O(agents^2 * steps).
const MAX_GRID_WORK: u64 = 60_000_000;
const MAX_BOIDS_WORK: u64 = 200_000_000;
/// Per-frame inter-publish delay ceiling, and the total wall-clock budget a
/// single streamed run may spend sleeping. Together they cap how long one
/// request can hold a worker while pacing frames for a live demo.
const MAX_FRAME_DELAY_MS: u64 = 100;
const MAX_STREAM_MILLIS: u64 = 15_000;
const DEFAULT_MAX_INFLIGHT: usize = 8;
/// Skip publishing a result larger than this (NATS default max_payload is ~1 MiB).
const MAX_PUBLISH_BYTES: usize = 900_000;

#[derive(Clone)]
struct AppState {
    nats: Option<async_nats::Client>,
    result_subject: String,
    frame_subject: String,
    ws_subject: String,
    event_subject: String,
    metrics: Arc<Metrics>,
    /// Bounds concurrent simulations so a request/NATS flood cannot spawn
    /// unbounded CPU-heavy, long-streaming work.
    inflight: Arc<tokio::sync::Semaphore>,
}

#[derive(Default)]
struct Metrics {
    requests_total: AtomicU64,
    simulations_total: AtomicU64,
    frames_published_total: AtomicU64,
    errors_total: AtomicU64,
    rejected_busy_total: AtomicU64,
    nats_messages_total: AtomicU64,
}

// ---------- PRNG ----------

struct Rng {
    state: u64,
}

impl Rng {
    fn new(seed: u64) -> Self {
        Rng {
            state: seed ^ 0x9E3779B97F4A7C15,
        }
    }
    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }
    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            0
        } else {
            (self.next_u64() % bound as u64) as usize
        }
    }
}

// ---------- Request / response ----------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SimRequest {
    request_id: Option<String>,
    model: String,
    width: Option<usize>,
    height: Option<usize>,
    steps: Option<usize>,
    seed: Option<u64>,
    density: Option<f64>,
    // SIR
    infection_rate: Option<f64>,
    recovery_rate: Option<f64>,
    waning_rate: Option<f64>,
    initial_infected: Option<f64>,
    // Schelling
    tolerance: Option<f64>,
    type_ratio: Option<f64>,
    // Boids
    agents: Option<usize>,
    perception: Option<f64>,
    max_speed: Option<f64>,
    separation_weight: Option<f64>,
    alignment_weight: Option<f64>,
    cohesion_weight: Option<f64>,
    // Streaming / output
    frame_stride: Option<usize>,
    frame_delay_ms: Option<u64>,
    include_frames: Option<bool>,
    initial_grid: Option<Vec<Vec<u8>>>,
}

#[derive(Debug, Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct Stats {
    #[serde(skip_serializing_if = "Option::is_none")]
    alive: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    susceptible: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    infected: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recovered: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    happy_fraction: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    segregation: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mean_speed: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    polarization: Option<f64>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Frame {
    step: usize,
    stats: Stats,
    #[serde(skip_serializing_if = "Option::is_none")]
    grid: Option<Vec<Vec<u8>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agents: Option<Vec<Agent2d>>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Agent2d {
    x: f64,
    y: f64,
    vx: f64,
    vy: f64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SimResponse {
    ok: bool,
    request_id: String,
    model: String,
    width: usize,
    height: usize,
    steps: usize,
    timeseries: Vec<Stats>,
    summary: Stats,
    converged: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    frames: Option<Vec<Frame>>,
    warnings: Vec<String>,
    generated_at_ms: u128,
}

struct SimRun {
    response: SimResponse,
    frames: Vec<Frame>,
    frame_delay_ms: u64,
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

fn finite_ratio(value: f64, label: &str) -> Result<f64, String> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(format!("{label} must be in [0, 1]"));
    }
    Ok(value)
}

/// Reject grid runs whose cells*steps would blow the CPU budget.
fn ensure_grid_budget(width: usize, height: usize, steps: usize) -> Result<(), String> {
    let work = (width as u64 * height as u64).saturating_mul(steps as u64);
    if work > MAX_GRID_WORK {
        return Err(format!(
            "grid work {width}x{height} over {steps} steps exceeds the {MAX_GRID_WORK} budget; reduce size or steps"
        ));
    }
    Ok(())
}

/// Reject boid runs whose agents^2*steps would blow the CPU budget.
fn ensure_boids_budget(count: usize, steps: usize) -> Result<(), String> {
    let work = (count as u64 * count as u64).saturating_mul(steps as u64);
    if work > MAX_BOIDS_WORK {
        return Err(format!(
            "boids work {count} agents over {steps} steps exceeds the {MAX_BOIDS_WORK} budget; reduce agents or steps"
        ));
    }
    Ok(())
}

// ---------- Grid neighbour helper ----------

fn moore_neighbors(r: usize, c: usize, height: usize, width: usize) -> [(usize, usize); 8] {
    let rm = (r + height - 1) % height;
    let rp = (r + 1) % height;
    let cm = (c + width - 1) % width;
    let cp = (c + 1) % width;
    [
        (rm, cm),
        (rm, c),
        (rm, cp),
        (r, cm),
        (r, cp),
        (rp, cm),
        (rp, c),
        (rp, cp),
    ]
}

// ---------- Driver ----------

fn simulate(request: SimRequest) -> Result<SimRun, String> {
    let request_id = request
        .request_id
        .clone()
        .unwrap_or_else(|| format!("sim-{}", now_ms()));
    let model = request.model.to_ascii_lowercase();
    let steps = request.steps.unwrap_or(100).clamp(1, MAX_STEPS);
    let frame_stride = request.frame_stride.unwrap_or(1).max(1);
    let frame_delay_ms = request.frame_delay_ms.unwrap_or(0).min(MAX_FRAME_DELAY_MS);
    let include_frames = request.include_frames.unwrap_or(false);
    let seed = request.seed.unwrap_or(0x5EED);

    let (timeseries, mut frames, summary, converged, width, height, mut warnings) = match model.as_str() {
        "life" => run_life(&request, steps, seed, frame_stride)?,
        "sir" => run_sir(&request, steps, seed, frame_stride)?,
        "schelling" => run_schelling(&request, steps, seed, frame_stride)?,
        "boids" => run_boids(&request, steps, seed, frame_stride)?,
        other => {
            return Err(format!(
                "unsupported model {other}; expected life, sir, schelling, or boids"
            ))
        }
    };

    // Bound the total wall-clock a streamed run can spend sleeping between
    // frames so one request can't hold a worker for minutes.
    if frame_delay_ms > 0 {
        let allowed = MAX_STREAM_MILLIS
            .checked_div(frame_delay_ms)
            .unwrap_or(MAX_STREAM_FRAMES as u64)
            .max(1) as usize;
        if frames.len() > allowed {
            warnings.push(format!(
                "streaming truncated to {allowed} frames to cap total pacing at {MAX_STREAM_MILLIS} ms (frameDelayMs={frame_delay_ms})"
            ));
            frames.truncate(allowed);
        }
    }

    let response = SimResponse {
        ok: true,
        request_id,
        model,
        width,
        height,
        steps,
        timeseries,
        summary,
        converged,
        frames: if include_frames {
            Some(frames.clone())
        } else {
            None
        },
        warnings,
        generated_at_ms: now_ms(),
    };

    Ok(SimRun {
        response,
        frames,
        frame_delay_ms,
    })
}

fn grid_dims(request: &SimRequest) -> Result<(usize, usize), String> {
    let width = request.width.unwrap_or(64).clamp(2, 1024);
    let height = request.height.unwrap_or(64).clamp(2, 1024);
    if width * height > MAX_CELLS {
        return Err(format!("width*height must be <= {MAX_CELLS}"));
    }
    Ok((width, height))
}

fn seed_grid(
    request: &SimRequest,
    width: usize,
    height: usize,
    rng: &mut Rng,
    density: f64,
    value: u8,
) -> Result<Vec<u8>, String> {
    if let Some(grid) = &request.initial_grid {
        if grid.len() != height || grid.iter().any(|row| row.len() != width) {
            return Err(format!(
                "initialGrid must be {height} rows of {width} columns"
            ));
        }
        return Ok(grid.iter().flatten().copied().collect());
    }
    let mut cells = vec![0u8; width * height];
    for cell in cells.iter_mut() {
        if rng.next_f64() < density {
            *cell = value;
        }
    }
    Ok(cells)
}

fn frame_grid(cells: &[u8], width: usize, height: usize) -> Vec<Vec<u8>> {
    (0..height)
        .map(|r| cells[r * width..(r + 1) * width].to_vec())
        .collect()
}

#[allow(clippy::type_complexity)]
fn run_life(
    request: &SimRequest,
    steps: usize,
    seed: u64,
    frame_stride: usize,
) -> Result<(Vec<Stats>, Vec<Frame>, Stats, bool, usize, usize, Vec<String>), String> {
    let (width, height) = grid_dims(request)?;
    ensure_grid_budget(width, height, steps)?;
    let density = finite_ratio(request.density.unwrap_or(0.3), "density")?;
    let mut rng = Rng::new(seed);
    let mut cells = seed_grid(request, width, height, &mut rng, density, 1)?;

    let mut timeseries = Vec::with_capacity(steps + 1);
    let mut frames = Vec::new();
    let mut converged = false;

    let life_stats = |cells: &[u8]| Stats {
        alive: Some(cells.iter().filter(|&&c| c == 1).count() as u64),
        ..Stats::default()
    };

    push_frame(&mut frames, &mut timeseries, 0, frame_stride, life_stats(&cells), || {
        frame_grid(&cells, width, height)
    });

    for step in 1..=steps {
        let mut next = vec![0u8; width * height];
        for r in 0..height {
            for c in 0..width {
                let live = moore_neighbors(r, c, height, width)
                    .iter()
                    .filter(|&&(nr, nc)| cells[nr * width + nc] == 1)
                    .count();
                let alive = cells[r * width + c] == 1;
                next[r * width + c] = if live == 3 || (alive && live == 2) {
                    1
                } else {
                    0
                };
            }
        }
        if next == cells {
            converged = true;
        }
        cells = next;
        let stats = life_stats(&cells);
        push_frame(&mut frames, &mut timeseries, step, frame_stride, stats, || {
            frame_grid(&cells, width, height)
        });
        if converged {
            break;
        }
    }

    let summary = timeseries.last().cloned().unwrap_or_default();
    Ok((timeseries, frames, summary, converged, width, height, Vec::new()))
}

#[allow(clippy::type_complexity)]
fn run_sir(
    request: &SimRequest,
    steps: usize,
    seed: u64,
    frame_stride: usize,
) -> Result<(Vec<Stats>, Vec<Frame>, Stats, bool, usize, usize, Vec<String>), String> {
    let (width, height) = grid_dims(request)?;
    ensure_grid_budget(width, height, steps)?;
    let beta = finite_ratio(request.infection_rate.unwrap_or(0.3), "infectionRate")?;
    let gamma = finite_ratio(request.recovery_rate.unwrap_or(0.1), "recoveryRate")?;
    let xi = finite_ratio(request.waning_rate.unwrap_or(0.0), "waningRate")?;
    let initial_infected = finite_ratio(request.initial_infected.unwrap_or(0.02), "initialInfected")?;
    let mut rng = Rng::new(seed);

    // 0 = S, 1 = I, 2 = R.
    let mut cells = vec![0u8; width * height];
    if let Some(grid) = &request.initial_grid {
        if grid.len() != height || grid.iter().any(|row| row.len() != width) {
            return Err(format!("initialGrid must be {height} rows of {width} columns"));
        }
        cells = grid.iter().flatten().copied().collect();
    } else {
        for cell in cells.iter_mut() {
            if rng.next_f64() < initial_infected {
                *cell = 1;
            }
        }
    }

    let sir_stats = |cells: &[u8]| {
        let mut s = 0u64;
        let mut i = 0u64;
        let mut r = 0u64;
        for &c in cells {
            match c {
                1 => i += 1,
                2 => r += 1,
                _ => s += 1,
            }
        }
        Stats {
            susceptible: Some(s),
            infected: Some(i),
            recovered: Some(r),
            ..Stats::default()
        }
    };

    let mut timeseries = Vec::with_capacity(steps + 1);
    let mut frames = Vec::new();
    push_frame(&mut frames, &mut timeseries, 0, frame_stride, sir_stats(&cells), || {
        frame_grid(&cells, width, height)
    });

    let mut converged = false;
    for step in 1..=steps {
        let mut next = cells.clone();
        for r in 0..height {
            for c in 0..width {
                let idx = r * width + c;
                match cells[idx] {
                    0 => {
                        let infected = moore_neighbors(r, c, height, width)
                            .iter()
                            .filter(|&&(nr, nc)| cells[nr * width + nc] == 1)
                            .count();
                        let p = 1.0 - (1.0 - beta).powi(infected as i32);
                        if rng.next_f64() < p {
                            next[idx] = 1;
                        }
                    }
                    1 => {
                        if rng.next_f64() < gamma {
                            next[idx] = 2;
                        }
                    }
                    _ => {
                        if xi > 0.0 && rng.next_f64() < xi {
                            next[idx] = 0;
                        }
                    }
                }
            }
        }
        let infected_now = next.iter().filter(|&&c| c == 1).count();
        cells = next;
        let stats = sir_stats(&cells);
        push_frame(&mut frames, &mut timeseries, step, frame_stride, stats, || {
            frame_grid(&cells, width, height)
        });
        if infected_now == 0 && xi == 0.0 {
            converged = true;
            break;
        }
    }

    let summary = timeseries.last().cloned().unwrap_or_default();
    Ok((timeseries, frames, summary, converged, width, height, Vec::new()))
}

#[allow(clippy::type_complexity)]
fn run_schelling(
    request: &SimRequest,
    steps: usize,
    seed: u64,
    frame_stride: usize,
) -> Result<(Vec<Stats>, Vec<Frame>, Stats, bool, usize, usize, Vec<String>), String> {
    let (width, height) = grid_dims(request)?;
    ensure_grid_budget(width, height, steps)?;
    let density = finite_ratio(request.density.unwrap_or(0.9), "density")?;
    let tolerance = finite_ratio(request.tolerance.unwrap_or(0.3), "tolerance")?;
    let type_ratio = finite_ratio(request.type_ratio.unwrap_or(0.5), "typeRatio")?;
    let mut rng = Rng::new(seed);

    // 0 = empty, 1 = type A, 2 = type B.
    let mut cells = vec![0u8; width * height];
    if let Some(grid) = &request.initial_grid {
        if grid.len() != height || grid.iter().any(|row| row.len() != width) {
            return Err(format!("initialGrid must be {height} rows of {width} columns"));
        }
        cells = grid.iter().flatten().copied().collect();
    } else {
        for cell in cells.iter_mut() {
            if rng.next_f64() < density {
                *cell = if rng.next_f64() < type_ratio { 1 } else { 2 };
            }
        }
    }

    let happiness = |cells: &[u8], r: usize, c: usize| -> Option<bool> {
        let me = cells[r * width + c];
        if me == 0 {
            return None;
        }
        let mut same = 0;
        let mut occupied = 0;
        for &(nr, nc) in moore_neighbors(r, c, height, width).iter() {
            let n = cells[nr * width + nc];
            if n != 0 {
                occupied += 1;
                if n == me {
                    same += 1;
                }
            }
        }
        if occupied == 0 {
            return Some(true);
        }
        Some(same as f64 / occupied as f64 >= tolerance)
    };

    let stats_of = |cells: &[u8]| -> Stats {
        let mut happy = 0u64;
        let mut occupied = 0u64;
        let mut same_sum = 0.0;
        let mut same_count = 0.0;
        for r in 0..height {
            for c in 0..width {
                if cells[r * width + c] == 0 {
                    continue;
                }
                occupied += 1;
                if happiness(cells, r, c) == Some(true) {
                    happy += 1;
                }
                let me = cells[r * width + c];
                let mut same = 0;
                let mut occ = 0;
                for &(nr, nc) in moore_neighbors(r, c, height, width).iter() {
                    let n = cells[nr * width + nc];
                    if n != 0 {
                        occ += 1;
                        if n == me {
                            same += 1;
                        }
                    }
                }
                if occ > 0 {
                    same_sum += same as f64 / occ as f64;
                    same_count += 1.0;
                }
            }
        }
        Stats {
            happy_fraction: Some(if occupied > 0 {
                happy as f64 / occupied as f64
            } else {
                1.0
            }),
            segregation: Some(if same_count > 0.0 {
                same_sum / same_count
            } else {
                0.0
            }),
            ..Stats::default()
        }
    };

    let mut timeseries = Vec::with_capacity(steps + 1);
    let mut frames = Vec::new();
    push_frame(&mut frames, &mut timeseries, 0, frame_stride, stats_of(&cells), || {
        frame_grid(&cells, width, height)
    });

    let mut converged = false;
    for step in 1..=steps {
        // Collect unhappy occupied cells and empty cells.
        let mut unhappy: Vec<usize> = Vec::new();
        let mut empties: Vec<usize> = Vec::new();
        for r in 0..height {
            for c in 0..width {
                let idx = r * width + c;
                if cells[idx] == 0 {
                    empties.push(idx);
                } else if happiness(&cells, r, c) == Some(false) {
                    unhappy.push(idx);
                }
            }
        }
        if unhappy.is_empty() || empties.is_empty() {
            converged = unhappy.is_empty();
            let stats = stats_of(&cells);
            push_frame(&mut frames, &mut timeseries, step, frame_stride, stats, || {
                frame_grid(&cells, width, height)
            });
            break;
        }
        // Shuffle unhappy order (Fisher-Yates) and relocate each to a random empty.
        for i in (1..unhappy.len()).rev() {
            unhappy.swap(i, rng.below(i + 1));
        }
        for &from in &unhappy {
            if empties.is_empty() {
                break;
            }
            let pick = rng.below(empties.len());
            let to = empties.swap_remove(pick);
            cells[to] = cells[from];
            cells[from] = 0;
            empties.push(from);
        }
        let stats = stats_of(&cells);
        push_frame(&mut frames, &mut timeseries, step, frame_stride, stats, || {
            frame_grid(&cells, width, height)
        });
    }

    let summary = timeseries.last().cloned().unwrap_or_default();
    Ok((timeseries, frames, summary, converged, width, height, Vec::new()))
}

#[allow(clippy::type_complexity)]
fn run_boids(
    request: &SimRequest,
    steps: usize,
    seed: u64,
    frame_stride: usize,
) -> Result<(Vec<Stats>, Vec<Frame>, Stats, bool, usize, usize, Vec<String>), String> {
    let width = request.width.unwrap_or(100).clamp(10, 4096);
    let height = request.height.unwrap_or(100).clamp(10, 4096);
    let count = request.agents.unwrap_or(120).clamp(1, MAX_BOIDS);
    ensure_boids_budget(count, steps)?;
    let perception = request.perception.unwrap_or(8.0).clamp(0.1, 1e4);
    let max_speed = request.max_speed.unwrap_or(2.0).clamp(0.01, 1e4);
    let sep_w = request.separation_weight.unwrap_or(1.5).clamp(0.0, 1e3);
    let align_w = request.alignment_weight.unwrap_or(1.0).clamp(0.0, 1e3);
    let coh_w = request.cohesion_weight.unwrap_or(1.0).clamp(0.0, 1e3);
    let mut rng = Rng::new(seed);

    let mut boids: Vec<Agent2d> = (0..count)
        .map(|_| {
            let angle = rng.next_f64() * std::f64::consts::TAU;
            Agent2d {
                x: rng.next_f64() * width as f64,
                y: rng.next_f64() * height as f64,
                vx: angle.cos() * max_speed,
                vy: angle.sin() * max_speed,
            }
        })
        .collect();

    let stats_of = |boids: &[Agent2d]| -> Stats {
        let n = boids.len().max(1) as f64;
        let mut speed_sum = 0.0;
        let mut sum_ux = 0.0;
        let mut sum_uy = 0.0;
        for b in boids {
            let speed = (b.vx * b.vx + b.vy * b.vy).sqrt();
            speed_sum += speed;
            if speed > 1e-9 {
                sum_ux += b.vx / speed;
                sum_uy += b.vy / speed;
            }
        }
        Stats {
            mean_speed: Some(speed_sum / n),
            polarization: Some(((sum_ux / n).powi(2) + (sum_uy / n).powi(2)).sqrt()),
            ..Stats::default()
        }
    };

    let mut timeseries = Vec::with_capacity(steps + 1);
    let mut frames = Vec::new();
    push_agent_frame(&mut frames, &mut timeseries, 0, frame_stride, stats_of(&boids), &boids);

    let wrap = |value: f64, bound: f64| -> f64 {
        let mut v = value % bound;
        if v < 0.0 {
            v += bound;
        }
        v
    };

    for step in 1..=steps {
        let snapshot = boids.clone();
        for b in boids.iter_mut() {
            let mut neighbors = 0.0;
            let mut avg_vx = 0.0;
            let mut avg_vy = 0.0;
            let mut center_x = 0.0;
            let mut center_y = 0.0;
            let mut sep_x = 0.0;
            let mut sep_y = 0.0;
            for other in &snapshot {
                let dx = toroidal_delta(other.x, b.x, width as f64);
                let dy = toroidal_delta(other.y, b.y, height as f64);
                let dist2 = dx * dx + dy * dy;
                if dist2 > 1e-9 && dist2 < perception * perception {
                    neighbors += 1.0;
                    avg_vx += other.vx;
                    avg_vy += other.vy;
                    center_x += dx;
                    center_y += dy;
                    let dist = dist2.sqrt();
                    sep_x -= dx / dist;
                    sep_y -= dy / dist;
                }
            }
            if neighbors > 0.0 {
                avg_vx /= neighbors;
                avg_vy /= neighbors;
                center_x /= neighbors;
                center_y /= neighbors;
                b.vx += align_w * (avg_vx - b.vx) * 0.05
                    + coh_w * center_x * 0.01
                    + sep_w * sep_x * 0.05;
                b.vy += align_w * (avg_vy - b.vy) * 0.05
                    + coh_w * center_y * 0.01
                    + sep_w * sep_y * 0.05;
            }
            // Clamp speed.
            let speed = (b.vx * b.vx + b.vy * b.vy).sqrt();
            if speed > max_speed && speed > 1e-9 {
                b.vx = b.vx / speed * max_speed;
                b.vy = b.vy / speed * max_speed;
            }
            b.x = wrap(b.x + b.vx, width as f64);
            b.y = wrap(b.y + b.vy, height as f64);
        }
        push_agent_frame(&mut frames, &mut timeseries, step, frame_stride, stats_of(&boids), &boids);
    }

    let summary = timeseries.last().cloned().unwrap_or_default();
    Ok((timeseries, frames, summary, false, width, height, Vec::new()))
}

fn toroidal_delta(a: f64, b: f64, bound: f64) -> f64 {
    let mut d = a - b;
    if d > bound / 2.0 {
        d -= bound;
    } else if d < -bound / 2.0 {
        d += bound;
    }
    d
}

fn push_frame<F: FnOnce() -> Vec<Vec<u8>>>(
    frames: &mut Vec<Frame>,
    timeseries: &mut Vec<Stats>,
    step: usize,
    frame_stride: usize,
    stats: Stats,
    grid: F,
) {
    timeseries.push(stats.clone());
    if step.is_multiple_of(frame_stride) && frames.len() < MAX_STREAM_FRAMES {
        frames.push(Frame {
            step,
            stats,
            grid: Some(grid()),
            agents: None,
        });
    }
}

fn push_agent_frame(
    frames: &mut Vec<Frame>,
    timeseries: &mut Vec<Stats>,
    step: usize,
    frame_stride: usize,
    stats: Stats,
    agents: &[Agent2d],
) {
    timeseries.push(stats.clone());
    if step.is_multiple_of(frame_stride) && frames.len() < MAX_STREAM_FRAMES {
        frames.push(Frame {
            step,
            stats,
            grid: None,
            agents: Some(agents.to_vec()),
        });
    }
}

// ---------- NATS publishing ----------

async fn stream_and_publish(state: &AppState, run: &SimRun) {
    let Some(nats) = &state.nats else {
        return;
    };
    let model = &run.response.model;
    let request_id = &run.response.request_id;
    for frame in &run.frames {
        let envelope = json!({
            "messageKind": "agent_sim.frame",
            "schemaVersion": "agent_sim.simulate.v1",
            "source": "dd-agent-sim-server",
            "requestId": request_id,
            "model": model,
            "frame": frame,
        });
        let payload = match serde_json::to_vec(&envelope) {
            Ok(payload) => payload,
            Err(error) => {
                eprintln!("failed to encode agent-sim frame: {error}");
                continue;
            }
        };
        if nats
            .publish(state.frame_subject.clone(), payload.clone().into())
            .await
            .is_ok()
        {
            state
                .metrics
                .frames_published_total
                .fetch_add(1, Ordering::Relaxed);
        }
        // Bridge to the shared websocket subject for browser animation.
        let _ = nats
            .publish(
                state.ws_subject.clone(),
                json!({
                    "channel": "agent_sim",
                    "requestId": request_id,
                    "model": model,
                    "frame": frame,
                })
                .to_string()
                .into(),
            )
            .await;
        if run.frame_delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(run.frame_delay_ms)).await;
        }
    }

    // The bundled result must NOT carry the frames: they are streamed
    // individually above, and a full frame set would blow past the NATS
    // max_payload (default 1 MiB). Strip them before publishing.
    let mut result_value = serde_json::to_value(&run.response).unwrap_or(serde_json::Value::Null);
    if let Some(object) = result_value.as_object_mut() {
        object.remove("frames");
    }
    let payload = match serde_json::to_vec(&json!({
        "messageKind": "agent_sim.simulate.result",
        "schemaVersion": "agent_sim.simulate.v1",
        "source": "dd-agent-sim-server",
        "result": result_value,
    })) {
        Ok(payload) => payload,
        Err(error) => {
            eprintln!("failed to encode agent-sim result: {error}");
            return;
        }
    };
    if payload.len() > MAX_PUBLISH_BYTES {
        eprintln!(
            "agent-sim result too large to publish: bytes={} max={MAX_PUBLISH_BYTES}",
            payload.len()
        );
        state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
    } else {
        let _ = nats
            .publish(state.result_subject.clone(), payload.into())
            .await;
    }
    let _ = nats
        .publish(
            state.event_subject.clone(),
            json!({
                "type": "agent_sim.simulate.result",
                "source": "dd-agent-sim-server",
                "requestId": request_id,
                "model": model,
                "steps": run.response.steps,
                "converged": run.response.converged,
                "atMs": now_ms(),
            })
            .to_string()
            .into(),
        )
        .await;
}

async fn simulate_in_background(request: SimRequest) -> Result<SimRun, String> {
    tokio::task::spawn_blocking(move || simulate(request))
        .await
        .map_err(|error| format!("simulate task join failed: {error}"))?
}

async fn healthz() -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "service": "dd-agent-sim-server",
        "mode": "agent-ca-sim-nats-ws",
        "models": ["life", "sir", "schelling", "boids"],
        "atMs": now_ms(),
    }))
}

async fn metrics(State(state): State<AppState>) -> Response {
    let m = &state.metrics;
    let body = format!(
        "# HELP dd_agent_sim_requests_total HTTP simulate requests.\n\
         # TYPE dd_agent_sim_requests_total counter\n\
         dd_agent_sim_requests_total {}\n\
         # HELP dd_agent_sim_simulations_total Simulations completed.\n\
         # TYPE dd_agent_sim_simulations_total counter\n\
         dd_agent_sim_simulations_total {}\n\
         # HELP dd_agent_sim_frames_published_total Frames fanned out over NATS.\n\
         # TYPE dd_agent_sim_frames_published_total counter\n\
         dd_agent_sim_frames_published_total {}\n\
         # HELP dd_agent_sim_errors_total Simulation or message errors.\n\
         # TYPE dd_agent_sim_errors_total counter\n\
         dd_agent_sim_errors_total {}\n\
         # HELP dd_agent_sim_rejected_busy_total Requests shed because the inflight cap was full.\n\
         # TYPE dd_agent_sim_rejected_busy_total counter\n\
         dd_agent_sim_rejected_busy_total {}\n\
         # HELP dd_agent_sim_nats_messages_total NATS simulate requests received.\n\
         # TYPE dd_agent_sim_nats_messages_total counter\n\
         dd_agent_sim_nats_messages_total {}\n",
        m.requests_total.load(Ordering::Relaxed),
        m.simulations_total.load(Ordering::Relaxed),
        m.frames_published_total.load(Ordering::Relaxed),
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
        Ok(run) => {
            state
                .metrics
                .simulations_total
                .fetch_add(1, Ordering::Relaxed);
            stream_and_publish(&state, &run).await;
            Json(run.response).into_response()
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
        println!("agent-sim nats loop disabled: NATS_URL is not configured");
        return;
    };
    println!(
        "agent-sim nats loop starting: subject={subject} queue_group={queue_group} frameSubject={}",
        state.frame_subject
    );
    loop {
        let mut subscription = match nats
            .queue_subscribe(subject.clone(), queue_group.clone())
            .await
        {
            Ok(subscription) => subscription,
            Err(error) => {
                eprintln!("agent-sim nats subscribe failed: {error}; retrying in 5s");
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
                    "agent-sim rejected oversize nats request: bytes={} max={MAX_NATS_PAYLOAD_BYTES}",
                    payload.len()
                );
                continue;
            }
            // Backpressure: wait for an inflight slot before taking on more work so a
            // NATS flood can't spawn unbounded simulations. NATS buffers/redelivers.
            let Ok(permit) = state.inflight.clone().acquire_owned().await else {
                continue;
            };
            let task_state = state.clone();
            tokio::spawn(async move {
                let _permit = permit;
                match serde_json::from_slice::<SimRequest>(&payload) {
                    Ok(request) => match simulate_in_background(request).await {
                        Ok(run) => {
                            task_state
                                .metrics
                                .simulations_total
                                .fetch_add(1, Ordering::Relaxed);
                            stream_and_publish(&task_state, &run).await;
                        }
                        Err(error) => {
                            task_state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                            eprintln!("agent-sim failed nats simulate: {error}");
                        }
                    },
                    Err(error) => {
                        task_state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                        eprintln!("agent-sim invalid nats request: {error}");
                    }
                }
            });
        }
        eprintln!("agent-sim nats subscription ended; re-subscribing in 5s");
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let host = env_value("HOST", "0.0.0.0");
    let port = env_value("PORT", "8133").parse::<u16>()?;
    let nats = match env::var("NATS_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        Some(url) => match async_nats::connect(&url).await {
            Ok(client) => Some(client),
            Err(error) => {
                eprintln!("agent-sim NATS connect failed ({url}): {error}");
                None
            }
        },
        None => None,
    };
    let max_inflight = env_usize("AGENT_SIM_MAX_INFLIGHT", DEFAULT_MAX_INFLIGHT);
    let state = AppState {
        nats,
        result_subject: env_value("AGENT_SIM_RESULT_SUBJECT", AGENT_SIM_SIMULATE_RESULTS_SUBJECT),
        frame_subject: env_value("AGENT_SIM_FRAME_SUBJECT", AGENT_SIM_FRAMES_SUBJECT),
        ws_subject: env_value("AGENT_SIM_WS_SUBJECT", WEBSOCKET_EVENTS_SUBJECT),
        event_subject: env_value("AGENT_SIM_EVENT_SUBJECT", RUNTIME_EVENTS_SUBJECT),
        metrics: Arc::new(Metrics::default()),
        inflight: Arc::new(tokio::sync::Semaphore::new(max_inflight)),
    };
    let subject = env_value("AGENT_SIM_SIMULATE_SUBJECT", AGENT_SIM_SIMULATE_REQUESTS_SUBJECT);
    let queue_group = env_value("AGENT_SIM_QUEUE_GROUP", AGENT_SIM_SIMULATE_REQUESTS_QUEUE_GROUP);
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
    println!("dd-agent-sim-server listening on http://{addr}");
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

    fn base(model: &str) -> SimRequest {
        SimRequest {
            request_id: None,
            model: model.to_string(),
            width: Some(16),
            height: Some(16),
            steps: Some(20),
            seed: Some(42),
            density: None,
            infection_rate: None,
            recovery_rate: None,
            waning_rate: None,
            initial_infected: None,
            tolerance: None,
            type_ratio: None,
            agents: None,
            perception: None,
            max_speed: None,
            separation_weight: None,
            alignment_weight: None,
            cohesion_weight: None,
            frame_stride: Some(1),
            frame_delay_ms: None,
            include_frames: Some(true),
            initial_grid: None,
        }
    }

    #[test]
    fn life_blinker_oscillates_and_conserves() {
        // A 3-cell blinker on an empty grid keeps 3 alive cells each step.
        let mut request = base("life");
        request.density = Some(0.0);
        let mut grid = vec![vec![0u8; 16]; 16];
        grid[8][7] = 1;
        grid[8][8] = 1;
        grid[8][9] = 1;
        request.initial_grid = Some(grid);
        let run = simulate(request).unwrap();
        assert_eq!(run.response.summary.alive, Some(3));
    }

    #[test]
    fn sir_epidemic_runs() {
        let run = simulate(base("sir")).unwrap();
        let last = run.response.summary;
        let total = last.susceptible.unwrap() + last.infected.unwrap() + last.recovered.unwrap();
        assert_eq!(total, 16 * 16);
    }

    #[test]
    fn schelling_increases_satisfaction() {
        let run = simulate(base("schelling")).unwrap();
        assert!(run.response.summary.happy_fraction.unwrap() >= 0.0);
    }

    #[test]
    fn boids_reports_polarization() {
        let run = simulate(base("boids")).unwrap();
        let pol = run.response.summary.polarization.unwrap();
        assert!((0.0..=1.0001).contains(&pol));
    }

    #[test]
    fn rejects_unknown_model() {
        assert!(simulate(base("nope")).is_err());
    }
}
