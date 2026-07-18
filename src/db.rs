//! SeaORM database connection initialization.

use sea_orm::{ConnectOptions, Database, DatabaseConnection, DbErr};
use std::time::Duration;

/// Connect to the reviewed database contract.
///
/// Schema changes are deliberately not applied by the server at startup. A
/// failed rollout must not acquire DDL authority or mutate production before a
/// human has reviewed and applied the migration separately.
pub async fn connect(database_url: &str) -> Result<DatabaseConnection, DbErr> {
    let mut options = ConnectOptions::new(database_url.to_owned());
    options
        .max_connections(10)
        .min_connections(1)
        .connect_timeout(Duration::from_secs(5))
        .acquire_timeout(Duration::from_secs(5))
        .idle_timeout(Duration::from_secs(300))
        .test_before_acquire(true);
    Database::connect(options).await
}
