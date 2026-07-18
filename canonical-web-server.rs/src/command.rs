//! Process command dispatch kept separate from the binary entry point.

use crate::{
    config::{Config, MigrationConfig},
    database, server,
};

pub async fn run(command: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        Some("migrate") => {
            let config = MigrationConfig::from_env()?;
            database::run_migrations(&config.database_url, config.database_max_connections).await?;
            tracing::info!("database migrations complete");
        }
        None | Some("serve") => server::run(Config::from_env()?).await?,
        Some(command) => {
            return Err(
                format!("unknown command {command:?}; expected `serve` or `migrate`").into(),
            );
        }
    }
    Ok(())
}
