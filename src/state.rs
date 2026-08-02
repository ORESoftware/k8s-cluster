//! Shared application dependencies.

use crate::metrics::Metrics;
use crate::shared_auth::SharedAuthClient;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement};
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    db: Arc<DatabaseConnection>,
    pub metrics: Arc<Metrics>,
    shared_auth: Option<SharedAuthClient>,
}

impl AppState {
    pub fn new(mut db: DatabaseConnection) -> Result<Self, prometheus::Error> {
        let metrics = Arc::new(Metrics::new()?);
        let database_metrics = Arc::clone(&metrics);
        db.set_metric_callback(move |info| {
            database_metrics.observe_database_query(info.elapsed, info.failed);
        });
        Ok(Self {
            db: Arc::new(db),
            metrics,
            shared_auth: None,
        })
    }

    pub fn with_shared_auth(mut self, shared_auth: Option<SharedAuthClient>) -> Self {
        self.shared_auth = shared_auth;
        self
    }

    pub fn database(&self) -> &DatabaseConnection {
        self.db.as_ref()
    }

    /// Readiness check against the database.
    ///
    /// Deliberately *not* `DatabaseConnection::ping()`: sea-orm's `ping()`
    /// acquires a pooled connection and pings it directly, bypassing the
    /// `set_metric_callback` hook above (only `execute`/`query_*` are wrapped in
    /// `sea_orm::metric::metric!`). Readiness is by far the most frequent
    /// database operation in a running deployment — one per pod per probe
    /// interval — so leaving it uninstrumented meant a merely *slow* database
    /// stayed invisible in `threefa_database_queries_total` and its latency
    /// histogram until a probe finally timed out. A trivial `SELECT 1` through
    /// the ordinary query path exercises the same acquire-and-round-trip and is
    /// counted like everything else.
    pub async fn ping_database(&self) -> Result<(), DbErr> {
        self.database()
            .execute_raw(Statement::from_string(
                DatabaseBackend::Postgres,
                "SELECT 1",
            ))
            .await
            .map(|_| ())
    }

    pub fn shared_auth(&self) -> Option<&SharedAuthClient> {
        self.shared_auth.as_ref()
    }
}
