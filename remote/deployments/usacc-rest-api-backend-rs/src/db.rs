use sqlx::{postgres::PgPoolOptions, PgPool};

use crate::{
    config::Config,
    error::{ApiError, ApiResult},
    state::AppState,
};

pub async fn connect(config: &Config) -> Result<Option<PgPool>, sqlx::Error> {
    let Some(database_url) = config.database_url.as_ref() else {
        return Ok(None);
    };

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(config.request_timeout)
        .connect(database_url)
        .await?;

    Ok(Some(pool))
}

pub fn pool(state: &AppState) -> ApiResult<&PgPool> {
    state.pool.as_ref().ok_or_else(|| {
        state.metrics.inc_db_error();
        ApiError::unavailable(
            "USACC database is not configured; set USACC_DATABASE_URL for database-backed routes",
        )
    })
}
