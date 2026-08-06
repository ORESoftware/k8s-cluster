//! SeaORM connection and explicit migration entry points.

use sea_orm::{ConnectionTrait, DatabaseBackend};
use sea_orm_migration::MigratorTrait;

use crate::{db::migration::Migrator, error::AppError};

pub async fn connect(
    database_url: &str,
    database_max_connections: u32,
) -> Result<sea_orm::DatabaseConnection, sea_orm::DbErr> {
    crate::db::connect_database(database_url, database_max_connections).await
}

/// Applies all database migrations and exits without constructing HTTP or
/// Supabase clients. Production schema changes remain a reviewed dpm workflow;
/// this command is retained for local and fresh-database initialization.
pub async fn run_migrations(
    database_url: &str,
    database_max_connections: u32,
) -> Result<(), AppError> {
    let db = connect(database_url, database_max_connections).await?;
    Migrator::up(&db, None).await?;
    if db.get_database_backend() == DatabaseBackend::Postgres {
        db.execute_unprepared(include_str!("../db/quote-schema.sql"))
            .await?;
    }
    db.close().await?;
    Ok(())
}
