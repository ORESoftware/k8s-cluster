//! SeaORM connection pooling against the shared pg-defs RDS database.
//!
//! Entities come from the generated `dd-pg-defs-sea-orm` adapter and carry
//! `schema_name = "daedalus"`, so every statement is schema-qualified at the
//! entity level. `search_path` is set as defence in depth for the occasional
//! raw statement, not as the primary namespacing mechanism.
//!
//! This service never migrates at boot. Schema changes go through
//! `scripts/dpm.sh` against pg-defs' `schema/schema.sql`, reviewed by a human.

use std::time::Duration;

use sea_orm::{ConnectOptions, Database, DatabaseConnection};

use crate::config::{env_bool, env_u64, optional_env};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub(crate) enum Persistence {
    Disabled,
    SeaOrm(DatabaseConnection),
}

impl Persistence {
    #[tracing::instrument(
        name = "persistence.connect",
        skip_all,
        fields(db.system = "postgresql", db.client = "seaorm", db.schema = "daedalus")
    )]
    pub(crate) async fn from_env() -> Result<Self, PersistenceError> {
        let required = env_bool("DAEDALUS_WEB_DATABASE_REQUIRED", false);
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

        let max_connections = env_u64("DAEDALUS_WEB_DATABASE_MAX_CONNECTIONS", 8, 1, 64) as u32;
        let min_connections = env_u64(
            "DAEDALUS_WEB_DATABASE_MIN_CONNECTIONS",
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
            .max_lifetime(Duration::from_secs(1_800))
            .set_schema_search_path("daedalus,public");

        let connection = Database::connect(options)
            .await
            .map_err(|_| PersistenceError::Connect)?;
        tracing::info!(
            db.client = "seaorm",
            db.system = "postgresql",
            db.schema = "daedalus",
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

    /// The live connection, or a service-unavailable error when persistence is
    /// switched off. Routes call this rather than matching on the enum so the
    /// disabled case degrades uniformly.
    pub(crate) fn connection(&self) -> Result<&DatabaseConnection, crate::error::ServiceError> {
        match self {
            Self::SeaOrm(connection) => Ok(connection),
            Self::Disabled => Err(crate::error::ServiceError::Unavailable(
                "database persistence is disabled (set DAEDALUS_WEB_DATABASE_URL)".to_string(),
            )),
        }
    }
}

/// Resolution order matches scripts/dpm.sh so the server and the migration tool
/// can never disagree about which database they are pointed at.
fn database_url() -> Option<String> {
    optional_env("DAEDALUS_WEB_DATABASE_URL")
        .or_else(|| optional_env("DATABASE_URL"))
        .or_else(|| optional_env("RDS_DATABASE_URL"))
}

#[derive(Debug)]
pub(crate) enum PersistenceError {
    MissingUrl,
    Connect,
}

impl std::fmt::Display for PersistenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingUrl => write!(
                f,
                "DAEDALUS_WEB_DATABASE_REQUIRED is set but no database URL was provided \
                 (DAEDALUS_WEB_DATABASE_URL, DATABASE_URL, or RDS_DATABASE_URL)"
            ),
            // Deliberately opaque: the URL carries a password, and connection
            // errors from sqlx echo the DSN back.
            Self::Connect => write!(f, "could not connect to the daedalus database"),
        }
    }
}

impl std::error::Error for PersistenceError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_error_never_echoes_the_dsn() {
        // sqlx connection errors embed the DSN (with password). Assert the
        // Display impl stays opaque so a boot failure cannot log a credential.
        let rendered = PersistenceError::Connect.to_string();
        assert!(!rendered.contains("postgres://"));
        assert!(!rendered.contains('@'));
    }

    #[test]
    fn disabled_persistence_yields_unavailable_not_a_panic() {
        let err = Persistence::Disabled.connection().unwrap_err();
        assert!(matches!(err, crate::error::ServiceError::Unavailable(_)));
        assert!(!Persistence::Disabled.is_enabled());
    }
}
