//! Database connection bootstrap, shared in spirit with t2v-web.
//!
//! Postgres is the cluster's source of truth; its schema lives in the shared
//! `pg-defs` contract under the `t2v` namespace and is migrated declaratively
//! by dpm — so against Postgres this app **connects with search_path=t2v and
//! never runs DDL**. SQLite (local dev / tests) has no such external contract,
//! so there we run the bundled sea-orm migrator to self-provision.

use sea_orm::{ConnectOptions, Database, DatabaseConnection, DbErr};
use std::time::Duration;

/// Postgres schema namespace this service owns in the shared database.
pub const PG_SCHEMA: &str = "t2v";

/// Default local database when `DATABASE_URL` is unset: an on-disk SQLite file.
const DEFAULT_DATABASE_URL: &str = "sqlite://./t2v.sqlite?mode=rwc";

pub fn database_url() -> String {
    std::env::var("DATABASE_URL")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_DATABASE_URL.to_string())
}

pub fn is_postgres(url: &str) -> bool {
    url.starts_with("postgres://") || url.starts_with("postgresql://")
}

/// Connect and make the schema ready:
/// * Postgres → set `search_path=t2v`, assume dpm already applied the schema.
/// * SQLite   → run the bundled migrator (idempotent bootstrap).
pub async fn connect_and_prepare() -> Result<DatabaseConnection, DbErr> {
    let url = database_url();
    let mut opts = ConnectOptions::new(url.clone());
    opts.max_connections(
        std::env::var("DB_MAX_CONNECTIONS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10),
    )
    .connect_timeout(Duration::from_secs(10))
    .acquire_timeout(Duration::from_secs(10))
    .sqlx_logging(false);

    if is_postgres(&url) {
        // Every pooled connection resolves unqualified table names to our
        // namespace, and never touches another app's tables.
        opts.set_schema_search_path(PG_SCHEMA.to_string());
    }

    let db = Database::connect(opts).await?;

    if !is_postgres(&url) {
        use sea_orm_migration::MigratorTrait;
        t2v_migration::Migrator::up(&db, None).await?;
        tracing::info!("t2v: sqlite migrator applied (local dev bootstrap)");
    } else {
        tracing::info!(
            "t2v: connected to postgres with search_path={PG_SCHEMA} (schema owned by pg-defs/dpm)"
        );
    }

    Ok(db)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn postgres_detection() {
        assert!(is_postgres("postgres://localhost/db"));
        assert!(is_postgres("postgresql://localhost/db"));
        assert!(!is_postgres("sqlite://./t2v.sqlite"));
    }
}
