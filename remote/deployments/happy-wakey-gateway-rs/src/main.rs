use happy_wakey_gateway_rs::{
    app, openapi_document, run_scheduler, AppConfig, AppState, NatsContactPublisher, ReminderStore,
    SharedAuthVerifier,
};
use std::{sync::Arc, time::Duration};

#[tokio::main]
async fn main() -> Result<(), String> {
    if std::env::args().any(|argument| argument == "--export-openapi") {
        println!(
            "{}",
            serde_json::to_string_pretty(&openapi_document())
                .map_err(|_| "OpenAPI serialization failed".to_string())?
        );
        return Ok(());
    }

    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tower_http=info".into()),
        )
        .init();

    let config = AppConfig::from_env()?;
    let shared_auth_base = required_env("SHARED_AUTH_BASE_URL")?;
    let introspect_secret = required_env("SHARED_AUTH_INTROSPECT_SECRET")?;
    let nats_url = required_env("NATS_URL")?;
    let nats_secret = optional_env("NATS_SHARED_SECRET");

    let verifier = Arc::new(SharedAuthVerifier::new(
        &shared_auth_base,
        introspect_secret,
    )?);
    let publisher = Arc::new(
        NatsContactPublisher::connect(&nats_url, nats_secret)
            .await
            .map_err(|error| format!("contact queue unavailable: {error}"))?,
    );
    let store = Arc::new(ReminderStore::open(config.state_path.clone())?);
    let state = AppState::new(config, verifier, publisher, store);
    tokio::spawn(run_scheduler(state.clone()));

    let host = optional_env("HOST").unwrap_or_else(|| "0.0.0.0".into());
    let port = optional_env("PORT")
        .unwrap_or_else(|| "8128".into())
        .parse::<u16>()
        .map_err(|_| "PORT must be a valid TCP port".to_string())?;
    let address = format!("{host}:{port}");
    let listener = tokio::net::TcpListener::bind(&address)
        .await
        .map_err(|_| "gateway bind failed".to_string())?;
    tracing::info!(listen.address = %address, "happy-wakey gateway listening");
    axum::serve(listener, app(state))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|_| "gateway server failed".to_string())
}

fn required_env(name: &str) -> Result<String, String> {
    optional_env(name).ok_or_else(|| format!("{name} is required"))
}

fn optional_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
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
            let _ = signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
        _ = tokio::time::sleep(Duration::from_secs(u64::MAX)) => {}
    }
}
