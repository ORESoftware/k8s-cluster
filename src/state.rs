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

    pub fn shared_auth(&self) -> Option<&SharedAuthClient> {
        self.shared_auth.as_ref()
    }
}
