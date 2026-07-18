//! Shared application dependencies.

use crate::config::{Config, SupabaseConfig};
use crate::metrics::Metrics;
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct AppState {
    pub supabase: Option<SupabaseConfig>,
    pub server_secret: Arc<Vec<u8>>,
    pub http: reqwest::Client,
    pub metrics: Arc<Metrics>,
}

impl AppState {
    pub fn from_config(config: Config) -> Result<Self, prometheus::Error> {
        Self::new(config.supabase, config.server_secret)
    }

    pub fn new(
        supabase: Option<SupabaseConfig>,
        server_secret: Vec<u8>,
    ) -> Result<Self, prometheus::Error> {
        Ok(Self {
            supabase,
            server_secret: Arc::new(server_secret),
            http: reqwest::Client::new(),
            metrics: Arc::new(Metrics::new()?),
        })
    }
}
