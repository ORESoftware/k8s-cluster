use std::{env, net::SocketAddr};

use axum::{
    routing::{get, post},
    Router,
};

mod api_docs;
mod container_pool_routes;
mod context;
mod db;
mod db_routes;
mod dispatch;
mod events;
mod graphql_routes;
mod handlers;
mod k8s;
mod lambdas;
mod metrics;
mod pg_contract;
mod shared;
mod state;
mod threads;
mod types;

pub(crate) use crate::db::{
    connect_postgres, connect_postgres_with_url, fetch_agent_events_from_postgres,
    fetch_agents_snapshot, fetch_known_git_repos_from_postgres, fetch_thread_context_from_postgres,
};
pub(crate) use crate::lambdas::{
    fetch_lambda_function_by_identifier, fetch_lambda_functions_from_postgres,
    lambda_limit_from_query, lambda_search_pattern,
};
pub(crate) use crate::metrics::record_request;
pub(crate) use crate::shared::{
    authorized_internal_request, env_bool, env_u64, env_usize, first_env,
    missing_worker_auth_secret_message, now_ms, postgres_database_url, public_data_source_error,
    row_opt_string, row_string, unauthorized_response, worker_auth_secret,
};
pub(crate) use crate::state::runtime_thread_context;
pub(crate) use crate::types::{
    AgentEventRow, AgentTaskRow, AgentThreadRow, AgentsDataConfig, AgentsSnapshot, AgentsSummary,
    KnownGitRepoRow, LambdaFunctionRow, LambdasQuery, ThreadContextResponse,
};

use crate::context::thread_context_candidates;
use crate::dispatch::dispatch_thread_task;
use crate::events::run_cdc_fanout_subscriptions;
use crate::handlers::{
    agent_task_events, agent_task_feedback, agent_thread_breadcrumb_tail, agents_tasks, healthz,
    ingest_agent_breadcrumb, ingest_agent_event, known_git_repos, runtime_config_snapshot,
    save_known_git_repo, thread_context,
};
use crate::lambdas::{
    create_lambda_function, image_builder_readyz, lambda_function, lambda_functions,
    package_lambda_image_internal, update_lambda_function,
};
use crate::metrics::metrics;
use crate::shared::{image_builder_role, internal_db_routes_enabled, service_name};
use crate::threads::{
    archive_thread, hard_delete_thread, make_commit_thread, merge_upstream_thread, open_pr_thread,
    prepare_thread, sleep_thread, stream_thread_task, terminal_thread, thread_runtime,
};

fn code_first_router() -> Router {
    Router::new()
        .route(
            "/api/runtime-config/snapshot/:env",
            get(runtime_config_snapshot),
        )
        .route("/api/agents/tasks", get(agents_tasks))
        .route(
            "/api/agents/git-repos",
            get(known_git_repos).post(save_known_git_repo),
        )
        .route(
            "/api/lambdas/functions",
            get(lambda_functions).post(create_lambda_function),
        )
        .route(
            "/api/lambdas/functions/:id",
            get(lambda_function).patch(update_lambda_function),
        )
        .route("/api/agents/tasks/:task_id/events", get(agent_task_events))
        .route(
            "/api/agents/tasks/:task_id/feedback",
            post(agent_task_feedback),
        )
        .route("/api/agents/events", post(ingest_agent_event))
        .route(
            "/api/agents/threads/:thread_id/breadcrumbs",
            post(ingest_agent_breadcrumb),
        )
        .route(
            "/api/agents/threads/:thread_id/breadcrumbs/tail",
            get(agent_thread_breadcrumb_tail),
        )
        .route(
            "/api/agents/threads/:thread_id/context",
            get(thread_context),
        )
        .route(
            "/api/agents/threads/:thread_id/context-candidates",
            post(thread_context_candidates),
        )
        .route(
            "/api/agents/threads/:thread_id/runtime",
            get(thread_runtime),
        )
        .route(
            "/api/agents/threads/:thread_id/prepare",
            post(prepare_thread),
        )
        .route(
            "/api/agents/threads/:thread_id/tasks",
            post(dispatch_thread_task),
        )
        .route(
            "/api/agents/threads/:thread_id/stream/:task_id",
            get(stream_thread_task),
        )
        .route("/api/agents/threads/:thread_id/sleep", post(sleep_thread))
        .route(
            "/api/agents/threads/:thread_id/archive",
            post(archive_thread),
        )
        .route(
            "/api/agents/threads/:thread_id/hard-delete",
            post(hard_delete_thread),
        )
        .route(
            "/api/agents/threads/:thread_id/merge-upstream",
            post(merge_upstream_thread),
        )
        .route(
            "/api/agents/threads/:thread_id/open-pr",
            post(open_pr_thread),
        )
        .route(
            "/api/agents/threads/:thread_id/make-commit",
            post(make_commit_thread),
        )
        .route(
            "/api/agents/threads/:thread_id/terminal",
            post(terminal_thread),
        )
        .merge(container_pool_routes::router())
}

fn app_router() -> Router {
    if image_builder_role() {
        return Router::new()
            .route("/healthz", get(healthz))
            .route("/readyz", get(image_builder_readyz))
            .route("/docs/api", get(api_docs::html))
            .route("/api/docs", get(api_docs::html))
            .route("/api/docs.json", get(api_docs::json))
            .route("/metrics", get(metrics))
            .route(
                "/internal/lambda-images/:function_id/package",
                post(package_lambda_image_internal),
            )
            .merge(container_pool_routes::builder_router())
            .merge(dd_runtime_config_client::router());
    }

    let router = Router::new()
        .route("/healthz", get(healthz))
        .route("/docs/api", get(api_docs::html))
        .route("/api/docs", get(api_docs::html))
        .route("/api/docs.json", get(api_docs::json))
        .merge(graphql_routes::router())
        .merge(code_first_router())
        .merge(dd_runtime_config_client::router())
        .route("/metrics", get(metrics));

    if internal_db_routes_enabled() {
        router.nest("/internal/db", db_routes::router())
    } else {
        router
    }
}

#[tokio::main]
async fn main() {
    let _otel = dd_telemetry::init(service_name());

    // Fail fast at startup if `remote/libs/pg-defs/schema/schema.sql`
    // has drifted away from what this service reads or writes against
    // RDS Postgres. The CI workflow `pg-defs-check` also enforces this
    // at PR time, but the runtime assertion guarantees the wiring is
    // exercised every time the binary boots (so a broken local build
    // can't ship even if CI was skipped).
    pg_contract::assert_canonical_schema_matches_local_reads();

    if !image_builder_role() {
        tokio::spawn(run_cdc_fanout_subscriptions());
    }
    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port = env::var("PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(8082);

    let app = app_router();

    let address: SocketAddr = format!("{host}:{port}")
        .parse()
        .expect("failed to parse bind address");
    tracing::info!(service = service_name(), "listening on http://{address}");

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
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut sigterm) = signal(SignalKind::terminate()) {
            let _ = sigterm.recv().await;
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
