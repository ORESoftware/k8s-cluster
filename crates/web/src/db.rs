//! Database connection for the web tier. Same contract as t2v-api: Postgres
//! uses `search_path=t2v` and its schema is owned by pg-defs/dpm (no in-app
//! DDL); SQLite local dev self-provisions via the bundled migrator.

use sea_orm::{ConnectOptions, Database, DatabaseConnection, DbErr};
use std::time::Duration;

pub const PG_SCHEMA: &str = "t2v";
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

pub async fn connect_and_prepare() -> Result<DatabaseConnection, DbErr> {
    let url = database_url();
    let mut opts = ConnectOptions::new(url.clone());
    opts.max_connections(
        std::env::var("DB_MAX_CONNECTIONS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(5),
    )
    .connect_timeout(Duration::from_secs(10))
    .acquire_timeout(Duration::from_secs(10))
    .sqlx_logging(false);

    if is_postgres(&url) {
        opts.set_schema_search_path(PG_SCHEMA.to_string());
    }

    let db = Database::connect(opts).await?;

    if !is_postgres(&url) {
        use sea_orm_migration::MigratorTrait;
        t2v_migration::Migrator::up(&db, None).await?;
        tracing::info!("t2v-web: sqlite migrator applied (local dev bootstrap)");
    } else {
        tracing::info!("t2v-web: connected to postgres with search_path={PG_SCHEMA}");
    }
    Ok(db)
}
