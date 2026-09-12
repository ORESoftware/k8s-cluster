use sqlx::postgres::{PgPool, PgPoolOptions};
use std::time::Duration;

use crate::config::Config;

pub async fn connect(cfg: &Config) -> anyhow::Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(32)
        .min_connections(2)
        .acquire_timeout(Duration::from_secs(10))
        .idle_timeout(Duration::from_secs(300))
        .connect(&cfg.database_url)
        .await?;

    if cfg.run_migrations {
        tracing::info!("running migrations");
        sqlx::migrate!("./migrations").run(&pool).await?;
        tracing::info!("migrations complete");
    } else {
        tracing::info!("migrations skipped (BILLING_RUN_MIGRATIONS=false)");
    }

    Ok(pool)
}
