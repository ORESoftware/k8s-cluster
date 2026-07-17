//! Database connection + small SeaORM helpers shared by every service.
//!
//! Schema management is declarative: `schema/schema.sql` is the source of
//! truth and operators converge the live database onto it with
//! `scripts/dpm.sh` (dpm — declarative-postgres-migrate). The server never
//! migrates at boot; the old `sqlx::migrate!` / `BILLING_RUN_MIGRATIONS`
//! path is gone and `migrations/` is a frozen historical record.

use sea_orm::{ConnectOptions, Database, DatabaseConnection, DbBackend, QueryResult, Statement};
use std::time::Duration;

use crate::config::Config;
use crate::error::{AppError, AppResult};

pub async fn connect(cfg: &Config) -> anyhow::Result<DatabaseConnection> {
    let mut opts = ConnectOptions::new(cfg.database_url.clone());
    opts.max_connections(32)
        .min_connections(2)
        .acquire_timeout(Duration::from_secs(10))
        .idle_timeout(Duration::from_secs(300))
        // Keep driver-level statement logging at DEBUG — the same visibility
        // the bare sqlx pool had. SeaORM's default of INFO would print every
        // statement under the production RUST_LOG=info filter.
        .sqlx_logging(true)
        .sqlx_logging_level(log::LevelFilter::Debug);

    let db = Database::connect(opts).await?;
    tracing::info!(
        "database connected; migrations are not run at boot — generate and review schema \
         changes with scripts/dpm.sh (declarative source: schema/schema.sql)"
    );
    Ok(db)
}

/// Build a Postgres [`Statement`] from raw SQL + positional binds. The SQL
/// strings across the services are kept byte-identical to the pre-SeaORM
/// sqlx queries wherever possible; enum-typed columns gain a `::TEXT` cast
/// on read because the driver cannot decode a custom enum OID into `String`.
pub fn stmt<I>(sql: &str, values: I) -> Statement
where
    I: IntoIterator<Item = sea_orm::Value>,
{
    Statement::from_sql_and_values(DbBackend::Postgres, sql, values)
}

/// `fetch_one` semantics on top of SeaORM's `query_one` (which returns
/// `Ok(None)` where sqlx returned `RowNotFound`).
pub fn require_row(row: Option<QueryResult>) -> AppResult<QueryResult> {
    row.ok_or_else(|| {
        AppError::Database(sea_orm::DbErr::RecordNotFound(
            "query expected one row, got none".into(),
        ))
    })
}

/// Decode a Postgres enum label (selected with `::TEXT`) into one of the
/// serde-tagged Rust enums. Reuses the serde rename table on each enum so
/// the Rust<->Postgres label mapping stays defined in exactly one place.
pub fn decode_enum<T: serde::de::DeserializeOwned>(column: &str, label: &str) -> AppResult<T> {
    serde_json::from_value(serde_json::Value::String(label.to_string())).map_err(|e| {
        AppError::Other(anyhow::anyhow!(
            "unexpected enum label {label:?} for column {column}: {e}"
        ))
    })
}
