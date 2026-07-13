use canonical_web_server::{
    config::{Config, MigrationConfig},
    run, run_migrations,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "canonical_web_server=info,tower_http=info".into()),
        )
        .init();

    match std::env::args().nth(1).as_deref() {
        Some("migrate") => {
            let config = MigrationConfig::from_env()?;
            run_migrations(&config.database_url, config.database_max_connections).await?;
            tracing::info!("database migrations complete");
        }
        None | Some("serve") => run(Config::from_env()?).await?,
        Some(command) => {
            return Err(
                format!("unknown command {command:?}; expected `serve` or `migrate`").into(),
            );
        }
    }
    Ok(())
}
