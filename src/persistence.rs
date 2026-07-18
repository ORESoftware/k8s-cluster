use std::{error::Error, fmt, time::Duration};

use sea_orm::{ConnectOptions, Database, DatabaseConnection};

use crate::config::{env_bool, env_u64, optional_env};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const READY_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone)]
pub(crate) enum Persistence {
    Disabled,
    SeaOrm(DatabaseConnection),
}

impl Persistence {
    #[tracing::instrument(
        name = "persistence.connect",
        skip_all,
        fields(db.system = "postgresql", db.client = "seaorm")
    )]
    pub(crate) async fn from_env() -> Result<Self, PersistenceError> {
        let required = env_bool("FABRICATION_DATABASE_REQUIRED", false);
        let Some(url) = database_url() else {
            if required {
                return Err(PersistenceError::MissingUrl);
            }
            tracing::info!(
                db.client = "seaorm",
                persistence.enabled = false,
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
            pool.max_connections = max_connections,
            pool.min_connections = min_connections,
            "SeaORM persistence initialized"
        );
        Ok(Self::SeaOrm(connection))
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

fn database_url() -> Option<String> {
    [
        "FABRICATION_DATABASE_URL",
        "RDS_DATABASE_URL",
        "DATABASE_URL",
    ]
    .into_iter()
    .find_map(optional_env)
}

#[derive(Debug)]
pub(crate) enum PersistenceError {
    MissingUrl,
    Connect,
}

impl fmt::Display for PersistenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingUrl => formatter.write_str(
                "FABRICATION_DATABASE_REQUIRED is enabled but no FABRICATION_DATABASE_URL, RDS_DATABASE_URL, or DATABASE_URL was provided",
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
