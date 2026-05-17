use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use tokio::net::TcpListener;
use tokio::signal;
use tokio::sync::Semaphore;
use tracing::info;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

use formal_methods_service::analysis::pipeline::Pipeline;
use formal_methods_service::config::Config;
use formal_methods_service::dedupe::DeliveryDedupe;
use formal_methods_service::github::GithubClient;
use formal_methods_service::path_filter::PathFilter;
use formal_methods_service::repo_allowlist::RepoAllowlist;
use formal_methods_service::routes;
use formal_methods_service::state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let config = Config::from_env().context("failed to load configuration from environment")?;

    let bind: SocketAddr = format!("{}:{}", config.bind_addr, config.port)
        .parse()
        .with_context(|| {
            format!(
                "invalid BIND_ADDR/PORT: {}:{}",
                config.bind_addr, config.port
            )
        })?;

    let github = GithubClient::new(
        config.github_api_base_url.clone(),
        config.github_token.clone(),
    )
    .context("failed to build GitHub HTTP client")?;

    let pipeline = Pipeline::from_config(&config);
    let semaphore = Arc::new(Semaphore::new(config.max_concurrent_analyses));
    let repo_allowlist = Arc::new(RepoAllowlist::from_config(&config.allowed_repos));
    let path_filter = Arc::new(PathFilter::from_config(&config.path_prefixes));
    let delivery_dedupe = Arc::new(Mutex::new(DeliveryDedupe::new(
        config.delivery_dedupe_capacity,
        config.delivery_dedupe_ttl,
    )));

    info!(
        allow_all_repos = repo_allowlist.allow_all(),
        path_filter_active = !path_filter.is_empty(),
        max_concurrent = config.max_concurrent_analyses,
        "wiring up app state"
    );

    let state = AppState {
        config: Arc::new(config),
        github: Arc::new(github),
        pipeline: Arc::new(pipeline),
        analysis_semaphore: semaphore,
        repo_allowlist,
        path_filter,
        delivery_dedupe,
    };

    let router = routes::router(state);
    let listener = TcpListener::bind(bind)
        .await
        .with_context(|| format!("failed to bind {bind}"))?;

    info!(addr = %bind, "formal-methods-service listening");

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")?;

    info!("formal-methods-service shut down cleanly");
    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("formal_methods_service=info,tower_http=info"));

    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_level(true)
        .with_thread_ids(false)
        .compact();

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .init();
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => info!("received SIGINT, beginning graceful shutdown"),
        _ = terminate => info!("received SIGTERM, beginning graceful shutdown"),
    }
}
