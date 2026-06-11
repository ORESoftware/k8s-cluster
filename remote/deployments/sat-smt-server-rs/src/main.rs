// dd-sat-smt-server
//
// CNF SAT / lightweight SMT solving over HTTP and NATS. The core is an
// iterative DPLL with unit propagation, pure-literal elimination, and a
// conflict budget so pathological instances return `unknown` instead of
// hanging. On top of raw CNF the request accepts cardinality sugar
// (atMostOne / atLeastOne / exactlyOne) that is compiled down to clauses,
// which gives the service an SMT-lite feel for encoding scheduling,
// configuration, and graph-colouring style constraints.
//
// Pairs with the formal-methods servers: throw a constraint problem at it
// over `dd.remote.sat.solve.requests` and read the model off
// `dd.remote.sat.solve.results`.

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
    RUNTIME_EVENTS_SUBJECT, SAT_SOLVE_REQUESTS_QUEUE_GROUP, SAT_SOLVE_REQUESTS_SUBJECT,
    SAT_SOLVE_RESULTS_SUBJECT,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;

const MAX_HTTP_BODY_BYTES: usize = 1024 * 1024;
const MAX_NATS_PAYLOAD_BYTES: usize = 1024 * 1024;
const MAX_VARS: usize = 2_000;
const MAX_CLAUSES: usize = 100_000;
const DEFAULT_CONFLICT_BUDGET: u64 = 2_000_000;
const MAX_CONFLICT_BUDGET: u64 = 50_000_000;
/// Hard ceiling on total clause-visits per solve so a crafted instance cannot
/// pin a core indefinitely even without hitting conflicts; exceeding it yields
/// `unknown` rather than running unbounded.
const MAX_SOLVE_WORK: u64 = 800_000_000;
/// Dedicated solver-thread stack. DPLL recurses up to MAX_VARS deep, which can
/// exceed a default 2 MiB worker stack; run it on a thread with headroom.
const SOLVER_STACK_BYTES: usize = 64 * 1024 * 1024;
const DEFAULT_MAX_INFLIGHT: usize = 16;
/// Skip publishing a result larger than this (NATS default max_payload is ~1 MiB).
const MAX_PUBLISH_BYTES: usize = 900_000;

#[derive(Clone)]
struct AppState {
    nats: Option<async_nats::Client>,
    result_subject: String,
    event_subject: String,
    metrics: Arc<Metrics>,
    /// Bounds concurrent solves so a request/NATS flood cannot spawn unbounded
    /// CPU-heavy work.
    inflight: Arc<tokio::sync::Semaphore>,
}

#[derive(Default)]
struct Metrics {
    requests_total: AtomicU64,
    solves_total: AtomicU64,
    sat_total: AtomicU64,
    unsat_total: AtomicU64,
    unknown_total: AtomicU64,
    errors_total: AtomicU64,
    rejected_busy_total: AtomicU64,
    nats_messages_total: AtomicU64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SolveRequest {
    request_id: Option<String>,
    /// Optional symbolic variable names. Clause literals may reference either a
    /// declared name (via `var`) or a 1-based index (via `index`).
    variables: Option<Vec<String>>,
    /// CNF clauses. Each literal is `{ "var": "x", "negated": false }` or
    /// `{ "index": 1, "negated": true }`. An all-false clause is a conflict.
    #[serde(default)]
    clauses: Vec<Vec<LiteralInput>>,
    /// Cardinality sugar compiled to clauses before solving.
    #[serde(default)]
    at_most_one: Vec<Vec<LiteralInput>>,
    #[serde(default)]
    at_least_one: Vec<Vec<LiteralInput>>,
    #[serde(default)]
    exactly_one: Vec<Vec<LiteralInput>>,
    conflict_budget: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LiteralInput {
    var: Option<String>,
    index: Option<usize>,
    #[serde(default)]
    negated: bool,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SolveResponse {
    ok: bool,
    request_id: String,
    kind: String,
    status: String,
    satisfiable: Option<bool>,
    variables: usize,
    clauses: usize,
    assignment: Vec<VarAssignment>,
    stats: SolveStats,
    warnings: Vec<String>,
    generated_at_ms: u128,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct VarAssignment {
    var: String,
    value: bool,
}

#[derive(Debug, Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct SolveStats {
    decisions: u64,
    propagations: u64,
    conflicts: u64,
    pure_literals: u64,
    budget_exhausted: bool,
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

/// 1-based signed literal: +i means var i true, -i means var i false.
type Lit = i64;

struct Cnf {
    names: Vec<String>,
    clauses: Vec<Vec<Lit>>,
}

fn build_cnf(request: &SolveRequest) -> Result<Cnf, String> {
    let mut names: Vec<String> = request.variables.clone().unwrap_or_default();
    let mut name_index: HashMap<String, usize> = names
        .iter()
        .enumerate()
        .map(|(i, name)| (name.clone(), i + 1))
        .collect();
    if names.len() != name_index.len() {
        return Err("duplicate variable names are not allowed".to_string());
    }
    if names.len() > MAX_VARS {
        return Err(format!("too many declared variables; max {MAX_VARS}"));
    }

    let mut resolve = |literal: &LiteralInput| -> Result<Lit, String> {
        let index = match (&literal.var, literal.index) {
            (Some(name), _) => {
                if let Some(idx) = name_index.get(name) {
                    *idx
                } else {
                    if names.len() >= MAX_VARS {
                        return Err(format!("too many variables; max {MAX_VARS}"));
                    }
                    names.push(name.clone());
                    let idx = names.len();
                    name_index.insert(name.clone(), idx);
                    idx
                }
            }
            (None, Some(idx)) => {
                if idx == 0 {
                    return Err("literal index is 1-based and must be >= 1".to_string());
                }
                // Bound the index BEFORE materialising auto-variables, otherwise a
                // single huge index would allocate billions of names (OOM).
                if idx > MAX_VARS {
                    return Err(format!("literal index {idx} exceeds max {MAX_VARS} variables"));
                }
                while names.len() < idx {
                    let auto = format!("v{}", names.len() + 1);
                    name_index.insert(auto.clone(), names.len() + 1);
                    names.push(auto);
                }
                idx
            }
            (None, None) => return Err("literal must set either var or index".to_string()),
        };
        if names.len() > MAX_VARS {
            return Err(format!("too many variables; max {MAX_VARS}"));
        }
        Ok(if literal.negated {
            -(index as Lit)
        } else {
            index as Lit
        })
    };

    let mut clauses: Vec<Vec<Lit>> = Vec::new();

    for clause in &request.clauses {
        if clause.is_empty() {
            return Err("empty clause is trivially unsatisfiable; reject explicitly".to_string());
        }
        let mut lits = Vec::with_capacity(clause.len());
        for literal in clause {
            lits.push(resolve(literal)?);
        }
        clauses.push(lits);
    }

    // exactlyOne = atLeastOne AND atMostOne.
    for group in &request.exactly_one {
        let mut lits = Vec::with_capacity(group.len());
        for literal in group {
            lits.push(resolve(literal)?);
        }
        if lits.is_empty() {
            return Err("exactlyOne group must be non-empty".to_string());
        }
        clauses.push(lits.clone()); // at-least-one
        push_at_most_one(&mut clauses, &lits);
    }
    for group in &request.at_least_one {
        let mut lits = Vec::with_capacity(group.len());
        for literal in group {
            lits.push(resolve(literal)?);
        }
        if lits.is_empty() {
            return Err("atLeastOne group must be non-empty".to_string());
        }
        clauses.push(lits);
    }
    for group in &request.at_most_one {
        let mut lits = Vec::with_capacity(group.len());
        for literal in group {
            lits.push(resolve(literal)?);
        }
        push_at_most_one(&mut clauses, &lits);
    }

    if clauses.len() > MAX_CLAUSES {
        return Err(format!("too many clauses; max {MAX_CLAUSES}"));
    }
    if names.is_empty() {
        return Err("no variables referenced by any clause".to_string());
    }

    Ok(Cnf { names, clauses })
}

/// Pairwise (naive) at-most-one encoding: for every pair (a, b) add (¬a ∨ ¬b).
fn push_at_most_one(clauses: &mut Vec<Vec<Lit>>, lits: &[Lit]) {
    for i in 0..lits.len() {
        for j in (i + 1)..lits.len() {
            clauses.push(vec![-lits[i], -lits[j]]);
        }
    }
}

#[derive(Default)]
struct DpllStats {
    decisions: u64,
    propagations: u64,
    conflicts: u64,
    pure_literals: u64,
    /// Total clause-visits; checked against MAX_SOLVE_WORK to bound CPU.
    work: u64,
    budget_exhausted: bool,
}

enum SolveOutcome {
    Sat(Vec<bool>),
    Unsat,
    Unknown,
}

fn solve_cnf(cnf: &Cnf, conflict_budget: u64) -> (SolveOutcome, DpllStats) {
    let num_vars = cnf.names.len();
    // assignment[i] for 1-based var i: 0 unassigned, 1 true, -1 false.
    let mut assignment = vec![0i8; num_vars + 1];
    let mut stats = DpllStats::default();
    let outcome = dpll(&cnf.clauses, &mut assignment, &mut stats, conflict_budget);
    let outcome = match outcome {
        Some(true) => {
            let model = (1..=num_vars).map(|i| assignment[i] == 1).collect();
            SolveOutcome::Sat(model)
        }
        Some(false) => SolveOutcome::Unsat,
        None => {
            stats.budget_exhausted = true;
            SolveOutcome::Unknown
        }
    };
    (outcome, stats)
}

fn lit_value(assignment: &[i8], lit: Lit) -> i8 {
    let var = lit.unsigned_abs() as usize;
    let raw = assignment[var];
    if raw == 0 {
        0
    } else if lit > 0 {
        raw
    } else {
        -raw
    }
}

/// Returns Some(true)=sat, Some(false)=unsat, None=budget exhausted.
fn dpll(clauses: &[Vec<Lit>], assignment: &mut [i8], stats: &mut DpllStats, budget: u64) -> Option<bool> {
    if stats.work > MAX_SOLVE_WORK {
        return None;
    }
    // Unit propagation to a fixpoint.
    loop {
        if stats.work > MAX_SOLVE_WORK {
            return None;
        }
        let mut progressed = false;
        for clause in clauses {
            stats.work += 1;
            let mut unassigned: Option<Lit> = None;
            let mut satisfied = false;
            let mut unassigned_count = 0;
            for &lit in clause {
                match lit_value(assignment, lit) {
                    1 => {
                        satisfied = true;
                        break;
                    }
                    0 => {
                        unassigned = Some(lit);
                        unassigned_count += 1;
                        if unassigned_count > 1 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if satisfied {
                continue;
            }
            if unassigned_count == 0 {
                stats.conflicts += 1;
                return Some(false);
            }
            if unassigned_count == 1 {
                let lit = unassigned.expect("unit literal present");
                assign(assignment, lit);
                stats.propagations += 1;
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }

    // Pure-literal elimination and the all-satisfied scan both walk every
    // clause; charge that to the work budget so they can't spin for free.
    stats.work = stats.work.saturating_add(2 * clauses.len() as u64);
    if stats.work > MAX_SOLVE_WORK {
        return None;
    }

    // Pure-literal elimination.
    if let Some(pure) = find_pure_literal(clauses, assignment) {
        assign(assignment, pure);
        stats.pure_literals += 1;
        let saved = snapshot(assignment);
        match dpll(clauses, assignment, stats, budget) {
            Some(true) => return Some(true),
            Some(false) => {
                restore(assignment, &saved);
                return Some(false);
            }
            None => return None,
        }
    }

    // All clauses satisfied?
    if all_satisfied(clauses, assignment) {
        return Some(true);
    }

    if stats.conflicts >= budget {
        return None;
    }

    let var = match pick_branch_var(assignment) {
        Some(var) => var,
        None => {
            // No unassigned var but not all satisfied: unsat on this path.
            return Some(false);
        }
    };

    for &value in &[1i8, -1i8] {
        let saved = snapshot(assignment);
        assignment[var] = value;
        stats.decisions += 1;
        match dpll(clauses, assignment, stats, budget) {
            Some(true) => return Some(true),
            Some(false) => restore(assignment, &saved),
            None => return None,
        }
    }
    Some(false)
}

fn assign(assignment: &mut [i8], lit: Lit) {
    let var = lit.unsigned_abs() as usize;
    assignment[var] = if lit > 0 { 1 } else { -1 };
}

fn snapshot(assignment: &[i8]) -> Vec<i8> {
    assignment.to_vec()
}

fn restore(assignment: &mut [i8], saved: &[i8]) {
    assignment.copy_from_slice(saved);
}

fn find_pure_literal(clauses: &[Vec<Lit>], assignment: &[i8]) -> Option<Lit> {
    let mut seen_pos: HashMap<usize, ()> = HashMap::new();
    let mut seen_neg: HashMap<usize, ()> = HashMap::new();
    for clause in clauses {
        // Skip already-satisfied clauses.
        if clause.iter().any(|&lit| lit_value(assignment, lit) == 1) {
            continue;
        }
        for &lit in clause {
            if lit_value(assignment, lit) != 0 {
                continue;
            }
            let var = lit.unsigned_abs() as usize;
            if lit > 0 {
                seen_pos.insert(var, ());
            } else {
                seen_neg.insert(var, ());
            }
        }
    }
    for (&var, _) in seen_pos.iter() {
        if !seen_neg.contains_key(&var) {
            return Some(var as Lit);
        }
    }
    for (&var, _) in seen_neg.iter() {
        if !seen_pos.contains_key(&var) {
            return Some(-(var as Lit));
        }
    }
    None
}

fn all_satisfied(clauses: &[Vec<Lit>], assignment: &[i8]) -> bool {
    clauses
        .iter()
        .all(|clause| clause.iter().any(|&lit| lit_value(assignment, lit) == 1))
}

fn pick_branch_var(assignment: &[i8]) -> Option<usize> {
    (1..assignment.len()).find(|&var| assignment[var] == 0)
}

fn solve(request: SolveRequest) -> Result<SolveResponse, String> {
    let request_id = request
        .request_id
        .clone()
        .unwrap_or_else(|| format!("sat-{}", now_ms()));
    let budget = request
        .conflict_budget
        .unwrap_or(DEFAULT_CONFLICT_BUDGET)
        .clamp(1, MAX_CONFLICT_BUDGET);

    let cnf = build_cnf(&request)?;
    let num_vars = cnf.names.len();
    let num_clauses = cnf.clauses.len();

    let (outcome, stats) = solve_cnf(&cnf, budget);

    let mut warnings = Vec::new();
    let (status, satisfiable, assignment) = match outcome {
        SolveOutcome::Sat(model) => {
            let assignment = cnf
                .names
                .iter()
                .enumerate()
                .map(|(i, name)| VarAssignment {
                    var: name.clone(),
                    value: model[i],
                })
                .collect();
            ("sat".to_string(), Some(true), assignment)
        }
        SolveOutcome::Unsat => ("unsat".to_string(), Some(false), Vec::new()),
        SolveOutcome::Unknown => {
            warnings.push(format!(
                "search budget exhausted (conflict budget {budget} or work ceiling {MAX_SOLVE_WORK}); increase conflictBudget or simplify the instance to keep searching"
            ));
            ("unknown".to_string(), None, Vec::new())
        }
    };

    Ok(SolveResponse {
        ok: true,
        request_id,
        kind: "sat.dpll".to_string(),
        status,
        satisfiable,
        variables: num_vars,
        clauses: num_clauses,
        assignment,
        stats: SolveStats {
            decisions: stats.decisions,
            propagations: stats.propagations,
            conflicts: stats.conflicts,
            pure_literals: stats.pure_literals,
            budget_exhausted: stats.budget_exhausted,
        },
        warnings,
        generated_at_ms: now_ms(),
    })
}

async fn solve_in_background(request: SolveRequest) -> Result<SolveResponse, String> {
    // Run on a dedicated thread with a large stack: DPLL recursion depth scales
    // with the variable count and can overflow a standard worker stack.
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("sat-solve".to_string())
        .stack_size(SOLVER_STACK_BYTES)
        .spawn(move || {
            let _ = tx.send(solve(request));
        })
        .map_err(|error| format!("failed to spawn solver thread: {error}"))?;
    rx.await
        .map_err(|error| format!("solver thread canceled: {error}"))?
}

async fn publish_result(state: &AppState, response: &SolveResponse) {
    let Some(nats) = &state.nats else {
        return;
    };
    let payload = match serde_json::to_vec(&json!({
        "messageKind": "sat.solve.result",
        "schemaVersion": "sat.solve.v1",
        "source": "dd-sat-smt-server",
        "result": response,
    })) {
        Ok(payload) => payload,
        Err(error) => {
            eprintln!("failed to encode sat result: {error}");
            return;
        }
    };
    if payload.len() > MAX_PUBLISH_BYTES {
        eprintln!(
            "sat result too large to publish: bytes={} max={MAX_PUBLISH_BYTES}",
            payload.len()
        );
        state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
        return;
    }
    if let Err(error) = nats
        .publish(state.result_subject.clone(), payload.into())
        .await
    {
        eprintln!("failed to publish sat result: {error}");
        return;
    }
    let _ = nats
        .publish(
            state.event_subject.clone(),
            json!({
                "type": "sat.solve.result",
                "source": "dd-sat-smt-server",
                "requestId": response.request_id,
                "status": response.status,
                "variables": response.variables,
                "clauses": response.clauses,
                "conflicts": response.stats.conflicts,
                "atMs": now_ms(),
            })
            .to_string()
            .into(),
        )
        .await;
}

fn record_outcome(metrics: &Metrics, response: &SolveResponse) {
    metrics.solves_total.fetch_add(1, Ordering::Relaxed);
    match response.status.as_str() {
        "sat" => metrics.sat_total.fetch_add(1, Ordering::Relaxed),
        "unsat" => metrics.unsat_total.fetch_add(1, Ordering::Relaxed),
        _ => metrics.unknown_total.fetch_add(1, Ordering::Relaxed),
    };
}

async fn healthz() -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "service": "dd-sat-smt-server",
        "mode": "dpll-sat-smt-nats",
        "atMs": now_ms(),
    }))
}

async fn metrics(State(state): State<AppState>) -> Response {
    let m = &state.metrics;
    let body = format!(
        "# HELP dd_sat_smt_requests_total HTTP solve requests.\n\
         # TYPE dd_sat_smt_requests_total counter\n\
         dd_sat_smt_requests_total {}\n\
         # HELP dd_sat_smt_solves_total Solve runs completed.\n\
         # TYPE dd_sat_smt_solves_total counter\n\
         dd_sat_smt_solves_total {}\n\
         # HELP dd_sat_smt_sat_total Satisfiable outcomes.\n\
         # TYPE dd_sat_smt_sat_total counter\n\
         dd_sat_smt_sat_total {}\n\
         # HELP dd_sat_smt_unsat_total Unsatisfiable outcomes.\n\
         # TYPE dd_sat_smt_unsat_total counter\n\
         dd_sat_smt_unsat_total {}\n\
         # HELP dd_sat_smt_unknown_total Budget-exhausted outcomes.\n\
         # TYPE dd_sat_smt_unknown_total counter\n\
         dd_sat_smt_unknown_total {}\n\
         # HELP dd_sat_smt_errors_total Solve or message errors.\n\
         # TYPE dd_sat_smt_errors_total counter\n\
         dd_sat_smt_errors_total {}\n\
         # HELP dd_sat_smt_rejected_busy_total Requests shed because the inflight cap was full.\n\
         # TYPE dd_sat_smt_rejected_busy_total counter\n\
         dd_sat_smt_rejected_busy_total {}\n\
         # HELP dd_sat_smt_nats_messages_total NATS solve requests received.\n\
         # TYPE dd_sat_smt_nats_messages_total counter\n\
         dd_sat_smt_nats_messages_total {}\n",
        m.requests_total.load(Ordering::Relaxed),
        m.solves_total.load(Ordering::Relaxed),
        m.sat_total.load(Ordering::Relaxed),
        m.unsat_total.load(Ordering::Relaxed),
        m.unknown_total.load(Ordering::Relaxed),
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

async fn solve_http(State(state): State<AppState>, Json(request): Json<SolveRequest>) -> Response {
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
            record_outcome(&state.metrics, &response);
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
        println!("sat-smt nats loop disabled: NATS_URL is not configured");
        return;
    };
    println!(
        "sat-smt nats loop starting: subject={subject} queue_group={queue_group} resultSubject={}",
        state.result_subject
    );
    let mut subscription = match nats.queue_subscribe(subject, queue_group).await {
        Ok(subscription) => subscription,
        Err(error) => {
            eprintln!("sat-smt nats subscribe failed: {error}");
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
                "sat-smt rejected oversize nats request: bytes={} max={MAX_NATS_PAYLOAD_BYTES}",
                payload.len()
            );
            continue;
        }
        // Backpressure: wait for an inflight slot before taking on more work so a
        // NATS flood can't spawn unbounded CPU-heavy solves. NATS buffers/redelivers.
        let Ok(permit) = state.inflight.clone().acquire_owned().await else {
            continue;
        };
        let task_state = state.clone();
        tokio::spawn(async move {
            let _permit = permit;
            match serde_json::from_slice::<SolveRequest>(&payload) {
                Ok(request) => match solve_in_background(request).await {
                    Ok(response) => {
                        record_outcome(&task_state.metrics, &response);
                        publish_result(&task_state, &response).await;
                    }
                    Err(error) => {
                        task_state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                        eprintln!("sat-smt failed nats solve: {error}");
                    }
                },
                Err(error) => {
                    task_state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                    eprintln!("sat-smt invalid nats request: {error}");
                }
            }
        });
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let host = env_value("HOST", "0.0.0.0");
    let port = env_value("PORT", "8130").parse::<u16>()?;
    let nats = match env::var("NATS_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        Some(url) => Some(async_nats::connect(url).await?),
        None => None,
    };
    let max_inflight = env_usize("SAT_MAX_INFLIGHT", DEFAULT_MAX_INFLIGHT);
    let state = AppState {
        nats,
        result_subject: env_value("SAT_RESULT_SUBJECT", SAT_SOLVE_RESULTS_SUBJECT),
        event_subject: env_value("SAT_EVENT_SUBJECT", RUNTIME_EVENTS_SUBJECT),
        metrics: Arc::new(Metrics::default()),
        inflight: Arc::new(tokio::sync::Semaphore::new(max_inflight)),
    };
    let subject = env_value("SAT_SOLVE_SUBJECT", SAT_SOLVE_REQUESTS_SUBJECT);
    let queue_group = env_value("SAT_QUEUE_GROUP", SAT_SOLVE_REQUESTS_QUEUE_GROUP);
    tokio::spawn(run_nats_loop(state.clone(), subject, queue_group));

    let app = Router::new()
        .route("/", get(healthz))
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics))
        .route("/solve", post(solve_http))
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .with_state(state)
        .merge(dd_runtime_config_client::router());

    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    println!("dd-sat-smt-server listening on http://{addr}");
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

    fn lit(var: &str, negated: bool) -> LiteralInput {
        LiteralInput {
            var: Some(var.to_string()),
            index: None,
            negated,
        }
    }

    fn base_request(clauses: Vec<Vec<LiteralInput>>) -> SolveRequest {
        SolveRequest {
            request_id: None,
            variables: None,
            clauses,
            at_most_one: Vec::new(),
            at_least_one: Vec::new(),
            exactly_one: Vec::new(),
            conflict_budget: None,
        }
    }

    #[test]
    fn solves_simple_satisfiable() {
        // (a ∨ b) ∧ (¬a ∨ b) ∧ (¬b) -> a=false, b=false fails clause1; expect unsat actually.
        // Use a clearly-sat instance: (a ∨ b) ∧ (¬a) -> a=false, b=true.
        let request = base_request(vec![
            vec![lit("a", false), lit("b", false)],
            vec![lit("a", true)],
        ]);
        let response = solve(request).unwrap();
        assert_eq!(response.status, "sat");
        let a = response.assignment.iter().find(|v| v.var == "a").unwrap();
        let b = response.assignment.iter().find(|v| v.var == "b").unwrap();
        assert!(!a.value);
        assert!(b.value);
    }

    #[test]
    fn detects_unsat() {
        // (a) ∧ (¬a)
        let request = base_request(vec![vec![lit("a", false)], vec![lit("a", true)]]);
        let response = solve(request).unwrap();
        assert_eq!(response.status, "unsat");
        assert_eq!(response.satisfiable, Some(false));
    }

    #[test]
    fn rejects_huge_literal_index_without_oom() {
        // A single out-of-range index must be rejected before materialising
        // auto-variables, not drive an unbounded allocation loop.
        let request = SolveRequest {
            request_id: None,
            variables: None,
            clauses: vec![vec![LiteralInput {
                var: None,
                index: Some(usize::MAX),
                negated: false,
            }]],
            at_most_one: Vec::new(),
            at_least_one: Vec::new(),
            exactly_one: Vec::new(),
            conflict_budget: None,
        };
        assert!(solve(request).is_err());
    }

    #[test]
    fn exactly_one_compiles_and_solves() {
        let mut request = base_request(Vec::new());
        request.exactly_one = vec![vec![lit("x", false), lit("y", false), lit("z", false)]];
        let response = solve(request).unwrap();
        assert_eq!(response.status, "sat");
        let trues = response.assignment.iter().filter(|v| v.value).count();
        assert_eq!(trues, 1);
    }
}
