//! Database pool + migration runner. Uses sqlx's runtime query API (no
//! compile-time `query!` macros) so the crate builds without a live database.

use sqlx::postgres::{PgPool, PgPoolOptions};
use std::time::Duration;

/// Connect and run pending migrations from `./migrations`.
pub async fn connect(database_url: &str) -> Result<PgPool, sqlx::Error> {
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(Duration::from_secs(5))
        .connect(database_url)
        .await?;

    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}
