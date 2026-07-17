//! Postgres connection for the search subsystem, via SeaORM.
//!
//! The schema is dpm-managed and declarative (schema/schema.sql is the source
//! of truth, applied with scripts/dpm.sh — the same workflow as
//! remote/libs/pg-defs/scripts/dpm.sh). The server never runs migrations at
//! boot.

use std::time::Duration;

use sea_orm::{ConnectOptions, Database, DatabaseConnection};

pub async fn connect(database_url: &str) -> anyhow::Result<DatabaseConnection> {
    let mut options = ConnectOptions::new(database_url);
    options
        .max_connections(16)
        .min_connections(1)
        .acquire_timeout(Duration::from_secs(10))
        .idle_timeout(Duration::from_secs(300));
    let pool = Database::connect(options).await?;
    tracing::info!(
        "search schema is dpm-managed; boot-time migrations are disabled — generate/apply \
         reviewed migrations with remote/deployments/dd-embeddings-rs/scripts/dpm.sh \
         (workflow of remote/libs/pg-defs/scripts/dpm.sh)"
    );
    Ok(pool)
}
