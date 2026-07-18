//! Shared application dependencies.

use crate::metrics::Metrics;
use sea_orm::DatabaseConnection;
use std::sync::Arc;
use tokio::sync::Semaphore;

#[derive(Clone)]
pub struct AppState {
    db: Arc<DatabaseConnection>,
    pub metrics: Arc<Metrics>,
    pub(crate) auth_slots: Arc<Semaphore>,
}

impl AppState {
    pub fn new(
        mut db: DatabaseConnection,
        auth_max_concurrent: usize,
    ) -> Result<Self, prometheus::Error> {
        let metrics = Arc::new(Metrics::new()?);
        let database_metrics = Arc::clone(&metrics);
        db.set_metric_callback(move |info| {
            database_metrics.observe_database_query(info.elapsed, info.failed);
        });
        Ok(Self {
            db: Arc::new(db),
            metrics,
            auth_slots: Arc::new(Semaphore::new(auth_max_concurrent)),
        })
    }

    pub fn database(&self) -> &DatabaseConnection {
        self.db.as_ref()
    }
}
