//! Optional service-level database connectivity.
//!
//! Database access in this crate goes through SeaORM. The soccer engine still
//! owns its legacy policy-loading adapter; new backend queries must use the
//! connection exposed here rather than importing SQLx directly.

use std::time::Duration;

use sea_orm::{ConnectOptions, Database, DatabaseConnection};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Default)]
pub(crate) struct DatabaseState {
    connection: Option<DatabaseConnection>,
    configured: bool,
}

impl DatabaseState {
    pub(crate) async fn from_env() -> Self {
        let Some(url) = env_string("AKRION_DATABASE_URL") else {
            tracing::info!(
                event = "akrion_database_disabled",
                orm = "seaorm",
                reason = "AKRION_DATABASE_URL is unset"
            );
            return Self::default();
        };

        let mut options = ConnectOptions::new(url);
        options
            .max_connections(env_u32("AKRION_DATABASE_MAX_CONNECTIONS", 5))
            .min_connections(env_u32("AKRION_DATABASE_MIN_CONNECTIONS", 0))
            .connect_timeout(CONNECT_TIMEOUT)
            .acquire_timeout(ACQUIRE_TIMEOUT)
            .sqlx_logging(false);

        match Database::connect(options).await {
            Ok(connection) => {
                tracing::info!(
                    event = "akrion_database_connected",
                    orm = "seaorm",
                    backend = "postgres"
                );
                Self {
                    connection: Some(connection),
                    configured: true,
                }
            }
            Err(error) => {
                tracing::error!(
                    event = "akrion_database_connect_failed",
                    orm = "seaorm",
                    error = %error
                );
                Self {
                    connection: None,
                    configured: true,
                }
            }
        }
    }

    #[allow(dead_code)]
    pub(crate) fn connection(&self) -> Option<&DatabaseConnection> {
        self.connection.as_ref()
    }

    pub(crate) async fn readiness(&self) -> DatabaseReadiness {
        match (&self.connection, self.configured) {
            (_, false) => DatabaseReadiness::Disabled,
            (None, true) => DatabaseReadiness::Unreachable,
            (Some(connection), true) => match connection.ping().await {
                Ok(()) => DatabaseReadiness::Connected,
                Err(error) => {
                    tracing::warn!(
                        event = "akrion_database_ping_failed",
                        orm = "seaorm",
                        error = %error
                    );
                    DatabaseReadiness::Unreachable
                }
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DatabaseReadiness {
    Disabled,
    Connected,
    Unreachable,
}

fn env_string(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_u32(name: &str, default: u32) -> u32 {
    env_string(name)
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}
