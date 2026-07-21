use std::{error::Error, fmt, sync::Arc, time::Duration};

use sea_orm::{ConnectOptions, Database, DatabaseConnection};

use crate::config::{env_bool, env_u64, optional_env};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const READY_TIMEOUT: Duration = Duration::from_secs(3);
pub(crate) const DECLARATIVE_SCHEMA: &str =
    "remote/libs/pg-defs/schema/databases/dd_fabrication_server/schema.sql";

#[derive(Clone)]
pub(crate) enum Persistence {
    Disabled,
    /// One pool, shared. `Arc` rather than a bare `DatabaseConnection` because
    /// SeaORM drops the `Clone` impl on `DatabaseConnection` when the `mock`
    /// feature is on (`#[cfg_attr(not(feature = "mock"), derive(Clone))]`), and
    /// the store tests in `src/stores.rs` need that feature. Sharing one pool
    /// behind an `Arc` is what the code did anyway — `Clone` on
    /// `DatabaseConnection` clones the handle, not the pool.
    SeaOrm(Arc<DatabaseConnection>),
}

impl Persistence {
    #[tracing::instrument(
        name = "persistence.connect",
        skip_all,
        fields(db.system = "postgresql", db.client = "seaorm")
    )]
    pub(crate) async fn from_env() -> Result<Self, PersistenceError> {
        Self::from_keys(
            "FABRICATION_DATABASE_REQUIRED",
            &[
                "FABRICATION_DATABASE_URL",
                "RDS_DATABASE_URL",
                "DATABASE_URL",
            ],
        )
        .await
    }

    pub(crate) async fn from_web_env() -> Result<Self, PersistenceError> {
        Self::from_keys(
            "FABRICATION_WEB_DATABASE_REQUIRED",
            &[
                "SUPABASE_DATABASE_URL",
                "FABRICATION_WEB_DATABASE_URL",
                "DATABASE_URL",
            ],
        )
        .await
    }

    async fn from_keys(
        required_key: &'static str,
        url_keys: &[&str],
    ) -> Result<Self, PersistenceError> {
        let required = env_bool(required_key, false);
        let Some(url) = database_url(url_keys) else {
            if required {
                return Err(PersistenceError::MissingUrl(required_key));
            }
            tracing::info!(
                db.client = "seaorm",
                persistence.enabled = false,
                db.schema.contract = DECLARATIVE_SCHEMA,
                "database persistence is disabled"
            );
            return Ok(Self::Disabled);
        };

        let max_connections = env_u64("FABRICATION_DATABASE_MAX_CONNECTIONS", 8, 1, 64) as u32;
        let min_connections = env_u64(
            "FABRICATION_DATABASE_MIN_CONNECTIONS",
            0,
            0,
            max_connections as u64,
        ) as u32;
        let mut options = ConnectOptions::new(url);
        options
            .max_connections(max_connections)
            .min_connections(min_connections)
            .connect_timeout(CONNECT_TIMEOUT)
            .acquire_timeout(CONNECT_TIMEOUT)
            .idle_timeout(Duration::from_secs(300))
            .max_lifetime(Duration::from_secs(1_800));

        let connection = Database::connect(options)
            .await
            .map_err(|_| PersistenceError::Connect)?;
        tracing::info!(
            db.client = "seaorm",
            db.system = "postgresql",
            persistence.enabled = true,
            db.schema.contract = DECLARATIVE_SCHEMA,
            pool.max_connections = max_connections,
            pool.min_connections = min_connections,
            "SeaORM persistence initialized"
        );
        Ok(Self::SeaOrm(Arc::new(connection)))
    }

    pub(crate) fn is_enabled(&self) -> bool {
        matches!(self, Self::SeaOrm(_))
    }

    #[tracing::instrument(
        name = "persistence.ready",
        skip_all,
        fields(db.system = "postgresql", db.client = "seaorm")
    )]
    pub(crate) async fn is_ready(&self) -> bool {
        match self {
            Self::Disabled => true,
            Self::SeaOrm(connection) => {
                matches!(
                    tokio::time::timeout(READY_TIMEOUT, connection.ping()).await,
                    Ok(Ok(()))
                )
            }
        }
    }
}

fn database_url(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| optional_env(key))
}

#[derive(Debug)]
pub(crate) enum PersistenceError {
    MissingUrl(&'static str),
    Connect,
}

impl fmt::Display for PersistenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingUrl(required_key) => write!(
                formatter,
                "{required_key} is enabled but no supported Postgres connection URL was provided"
            ),
            Self::Connect => formatter.write_str(
                "SeaORM could not connect to the configured Postgres database; connection details were redacted",
            ),
        }
    }
}

impl Error for PersistenceError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn disabled_persistence_is_ready() {
        let persistence = Persistence::Disabled;
        assert!(!persistence.is_enabled());
        assert!(persistence.is_ready().await);
    }
}
