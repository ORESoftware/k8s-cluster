//! Postgres connection for the search subsystem. Mirrors the
//! `billing-server-rs` pattern: a pooled connection plus hand-authored
//! migrations embedded at compile time and run on boot when enabled.

use std::time::Duration;

use sqlx::postgres::{PgPool, PgPoolOptions};

pub async fn connect(database_url: &str, run_migrations: bool) -> anyhow::Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(16)
        .min_connections(1)
        .acquire_timeout(Duration::from_secs(10))
        .idle_timeout(Duration::from_secs(300))
        .connect(database_url)
        .await?;

    if run_migrations {
        tracing::info!("running search migrations");
        sqlx::migrate!("./migrations").run(&pool).await?;
        tracing::info!("search migrations complete");
    } else {
        tracing::info!("search migrations skipped (SEARCH_RUN_MIGRATIONS=false)");
    }

    Ok(pool)
}
