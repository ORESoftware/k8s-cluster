//! Shared state for the web tier.

use sea_orm::DatabaseConnection;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseConnection,
    /// Base URL of the t2v-api server that performs translate/TTS actions.
    pub api_base: Arc<str>,
    pub http: reqwest::Client,
}

impl AppState {
    pub fn new(db: DatabaseConnection) -> Self {
        let api_base = std::env::var("API_BASE_URL")
            .ok()
            .map(|v| v.trim().trim_end_matches('/').to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "http://localhost:8130".to_string());
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(190))
            .build()
            .expect("reqwest client construction cannot fail with static config");
        Self {
            db,
            api_base: Arc::from(api_base),
            http,
        }
    }
}
