use std::{net::SocketAddr, sync::Arc};

use config::{Config, SERVICE_NAME};
use jobs::JobStore;
use metrics::Metrics;
use observability::log_info;
use routes::{router, AppState};

mod audit;
mod auth;
mod config;
mod jobs;
mod metrics;
mod models;
mod observability;
mod routes;
mod standards;
mod util;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = Arc::new(Config::from_env());
    tokio::fs::create_dir_all(&config.work_root).await?;
    let http = reqwest::Client::builder()
        .user_agent(format!(
            "{SERVICE_NAME}/0.1 (+https://github.com/ORESoftware/k8s-cluster)"
        ))
        .build()?;
    let state = AppState {
        config: config.clone(),
        metrics: Arc::new(Metrics::default()),
        jobs: Arc::new(JobStore::new(config.max_jobs)),
        http,
    };
    let app = router(state);
    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    let address: SocketAddr = format!("{}:{}", config.host, config.port).parse()?;
    log_info(
        "compliance.server.starting",
        "dd-compliance-rs starting",
        serde_json::json!({ "address": address.to_string() }),
    );
    let listener = tokio::net::TcpListener::bind(address).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}
