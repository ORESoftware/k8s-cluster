use canonical_auth::SupabaseAuth;
use canonical_config::SessionRevokerConfig;
use canonical_session::SessionRevoker;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "canonical_web_server=info,canonical_session_revoker=info".into()
            }),
        )
        .init();

    let config = SessionRevokerConfig::from_env()?;
    let db =
        canonical_store::connect_database(&config.database_url, config.database_max_connections)
            .await?;
    let auth = Arc::new(SupabaseAuth::new(
        config.supabase_url,
        config.supabase_publishable_key,
    )?);
    let revoker = SessionRevoker::new(db.clone(), auth, &config.session_encryption_key)?;
    revoker.verify_database_role().await?;

    match std::env::args().nth(1).as_deref() {
        None | Some("run") => {
            let worker = revoker.spawn();
            tracing::info!(
                service = "canonical-session-revoker",
                "session revoker started"
            );
            shutdown_signal().await;
            worker.shutdown().await;
        }
        Some("check") => {}
        Some(command) => {
            return Err(format!("unknown command {command:?}; expected `run` or `check`").into());
        }
    }
    drop(revoker);
    db.close().await?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::error!(%error, "failed to install Ctrl+C handler");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => tracing::error!(%error, "failed to install SIGTERM handler"),
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}
