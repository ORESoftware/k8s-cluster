use std::{env, net::SocketAddr};

use axum::{routing::get, Router};

mod agents;
mod container_pool;
mod grafana;
mod handlers;
mod home;
mod jello;
mod labs;
mod lambda;
mod metrics;
mod shared;
mod state;

use crate::grafana::{
    grafana_deployment_redirect, grafana_fabrication_redirect, grafana_observability_redirect,
};
use crate::handlers::*;
use crate::state::AppState;

#[tokio::main]
async fn main() {
    let _otel = dd_telemetry::init("dd-remote-web-home");

    let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port = env::var("PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(8080);

    let state = AppState {
        server_label:
            "Rust home server (/ + /home + /jello + /agents/tasks + /agents/threads + /lambdas/functions)"
                .to_string(),
        control_plane_label: "Kubernetes Ingress selects the UUID-bound worker Service".to_string(),
        workers_label: "Node.js containers pinned to one chat/thread".to_string(),
        queue_consumer_label: "Rust NATS shadow preparer (dd-remote-queue-consumer)".to_string(),
    };

    // Mount the receive helper at /internal/update-runtime-config (+ snapshot
    // + reset). The control plane pushes a payload here every 5 min.
    let runtime_config_router = dd_runtime_config_client::router();
    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    let app = Router::new()
        .route("/", get(root))
        .route("/home", get(home))
        .route("/home/", get(root))
        .route("/jello", get(jello_page))
        .route("/jello/", get(jello_page))
        .route("/jello/sample", get(jello_sample))
        .route("/jello/sample/", get(jello_sample))
        .route("/agents/tasks", get(agents_tasks_page))
        .route("/agents/tasks/", get(agents_tasks_page))
        .route("/agents/threads", get(agents_threads_page))
        .route("/agents/threads/", get(agents_threads_page))
        .route("/assets/web-home/agents-tasks.css", get(agents_tasks_css))
        .route("/assets/web-home/agents-tasks.js", get(agents_tasks_js))
        .route("/assets/web-home/shared-header.css", get(shared_header_css))
        .route("/assets/web-home/shared-header.js", get(shared_header_js))
        .route("/service-worker.js", get(service_worker_js))
        .route(
            "/assets/web-home/agents-tasks.html",
            get(agents_tasks_html_fragment),
        )
        .route(
            "/assets/web-home/agents-threads.css",
            get(agents_threads_css),
        )
        .route("/assets/web-home/agents-threads.js", get(agents_threads_js))
        .route(
            "/assets/web-home/agents-threads.html",
            get(agents_threads_html_fragment),
        )
        .route("/lambdas/functions", get(lambda_functions_page))
        .route("/lambdas/functions/", get(lambda_functions_page))
        .route("/container-pool/config", get(container_pool_config_page))
        .route("/container-pool/config/", get(container_pool_config_page))
        .route("/presence-test", get(presence_test_page))
        .route("/presence-test/", get(presence_test_page))
        .route("/wss-test", get(wss_test_page))
        .route("/wss-test/", get(wss_test_page))
        .route(
            "/grafana/observability",
            get(grafana_observability_redirect),
        )
        .route(
            "/grafana/observability/",
            get(grafana_observability_redirect),
        )
        .route("/grafana/fabrication", get(grafana_fabrication_redirect))
        .route("/grafana/fabrication/", get(grafana_fabrication_redirect))
        .route(
            "/grafana/depl/{deployment}",
            get(grafana_deployment_redirect),
        )
        .route(
            "/grafana/depl/{deployment}/",
            get(grafana_deployment_redirect),
        )
        .route("/healthz", get(healthz))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .route("/api-docs", get(api_docs_index_html))
        .route("/api-docs/", get(api_docs_index_html))
        .route("/api-docs.json", get(api_docs_index_json))
        .route("/factmachine-markets", get(factmachine_markets_html))
        .route("/factmachine-markets/", get(factmachine_markets_html))
        .route("/metrics", get(metrics))
        .route("/favicon.ico", get(favicon))
        .with_state(state)
        .merge(runtime_config_router);

    let address: SocketAddr = format!("{host}:{port}")
        .parse()
        .expect("failed to parse bind address");
    tracing::info!("dd-remote-web-home listening on http://{address}");

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
