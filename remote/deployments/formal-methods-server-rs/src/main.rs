// dd-formal-methods-server
//
// Authenticated Rust HTTP service that ingests a codebase (git repo or inline
// source) and runs formal-methods style analysis over a small language-agnostic
// annotation DSL embedded in source comments. Verification conditions are
// discharged by shelling out to the `z3` SMT solver and the results are
// returned as structured findings with counter-examples where applicable.
//
// The annotation DSL recognised inside line comments (// ... | # ... | -- ...):
//
//     @var name: <Int|Real|Bool>          -- declare a logical variable
//     @assume <expr>                      -- assume the expression unconditionally
//     @requires <expr>                    -- precondition for the next contract
//     @ensures <expr>                     -- postcondition to prove
//     @invariant <expr>                   -- loop invariant to prove (with @variant for progress)
//     @variant <int-expr>                 -- monotonically decreasing termination measure
//     @assert <expr>                      -- ad-hoc property to prove right here
//
// Each contiguous block of these annotations is a "verification unit". The
// service emits one SMT query per @ensures / @assert / @invariant goal:
//
//     (and <requires...> <assume...>) AND (not <goal>)
//
// If Z3 reports sat the postcondition is falsifiable and the counterexample
// model is returned as the bug. If unsat the postcondition follows by
// deduction from the assumptions. If unknown the result is reported as
// undetermined.
//
// In addition to the explicit annotation system the service performs a small
// suite of automatic heuristic checks that do not require any annotations:
//
//   - tautology / contradiction detection on `if (cond)` lines that only
//     reference variables declared in the current @var scope.
//   - dead nested branch detection: the conjunction of outer and inner
//     `if (...)` path conditions is checked for satisfiability.
//   - unsatisfiable @requires: if the conjunction of preconditions for a
//     contract is itself unsat the function is unreachable as specified.

use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};

use axum::{
    extract::DefaultBodyLimit,
    routing::{get, post},
    Router,
};
use tokio::{
    fs,
    sync::{RwLock, Semaphore},
};

mod annotations;
mod expr;
mod github;
mod handlers;
mod jobs;
mod scan;
mod smt;
mod state;
mod types;
mod validation;
mod verify;

use crate::handlers::{
    descriptor, get_analysis, get_analysis_logs, get_pull_request_status, github_webhook, healthz,
    list_analyses, metrics, submit_analysis, validate_inline,
};
use crate::state::{config_from_env, env_u64, env_usize, env_value, AppState, Counters};

const SERVICE_NAME: &str = "dd-formal-methods-server";
const DEFAULT_PORT: u16 = 8110;
const SCHEMA_VERSION: &str = "formal-methods.v1";
const MAX_REQUEST_BODY_BYTES: usize = 8 * 1024 * 1024;

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

async fn api_docs_html() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("../generated/api-docs.html"))
}

async fn api_docs_json() -> impl axum::response::IntoResponse {
    (
        [("content-type", "application/json; charset=utf-8")],
        include_str!("../generated/api-docs.json"),
    )
}

#[tokio::main]
async fn main() {
    let _otel = dd_telemetry::init("dd-formal-methods-server");

    let config = Arc::new(config_from_env());
    let host = env_value("HOST", "0.0.0.0");
    let port = env_u64("PORT", DEFAULT_PORT as u64) as u16;
    let max_concurrent = env_usize("FORMAL_METHODS_MAX_CONCURRENT", 2);

    if let Err(error) = fs::create_dir_all(&config.work_root).await {
        panic!("failed to create formal-methods work root: {error}");
    }

    let http = reqwest::Client::builder()
        .user_agent(format!(
            "{SERVICE_NAME}/0.1 (+https://github.com/ORESoftware/k8s-cluster)"
        ))
        .timeout(Duration::from_secs(30))
        .build()
        .expect("failed to build reqwest client");

    let state = AppState {
        config,
        http,
        jobs: Arc::new(RwLock::new(HashMap::new())),
        semaphore: Arc::new(Semaphore::new(max_concurrent)),
        counters: Arc::new(Counters::default()),
    };

    let app = Router::new()
        .route("/", get(descriptor))
        .route("/healthz", get(healthz))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .route("/metrics", get(metrics))
        .route("/analyses", get(list_analyses).post(submit_analysis))
        .route("/analyses/:job_id", get(get_analysis))
        .route("/analyses/:job_id/logs", get(get_analysis_logs))
        .route("/validate", post(validate_inline))
        .route("/webhooks/github", post(github_webhook))
        .route("/pulls/:owner/:repo/:number", get(get_pull_request_status))
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES))
        .with_state(state)
        .merge(dd_runtime_config_client::router());

    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    let address: SocketAddr = format!("{host}:{port}")
        .parse()
        .expect("failed to parse bind address");
    tracing::info!("{SERVICE_NAME} listening on http://{address}");

    let listener = tokio::net::TcpListener::bind(address)
        .await
        .expect("failed to bind tcp listener");
    axum::serve(listener, app.layer(dd_telemetry::http_trace_layer()))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("axum server crashed");
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}
