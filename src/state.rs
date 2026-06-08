use std::sync::Arc;

use reqwest::Client;
use sqlx::PgPool;

use crate::{config::Config, metrics::Metrics};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub pool: Option<PgPool>,
    pub http: Client,
    pub metrics: Arc<Metrics>,
}

impl AppState {
    pub fn new(config: Config, pool: Option<PgPool>, http: Client) -> Self {
        Self {
            config: Arc::new(config),
            pool,
            http,
            metrics: Arc::new(Metrics::default()),
        }
    }

    pub fn database_configured(&self) -> bool {
        self.pool.is_some()
    }
}
