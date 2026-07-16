//! Database pool initialization. Uses sqlx's runtime query API (no
//! compile-time `query!` macros) so the crate builds without a live database.

use sqlx::postgres::{PgPool, PgPoolOptions};
use std::time::Duration;

/// Connect to the reviewed database contract.
///
/// Schema changes are deliberately not applied by the server at startup. A
/// failed rollout must not acquire DDL authority or mutate production before a
/// human has reviewed and applied the migration separately.
pub async fn connect(database_url: &str) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(Duration::from_secs(5))
        .connect(database_url)
        .await
}
