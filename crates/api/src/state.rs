//! Shared application state handed to every axum handler.

use crate::metrics::Metrics;
use crate::vapi_client::VapiClient;
use sea_orm::DatabaseConnection;
use std::sync::Arc;
use t2v_llm::LlmClient;

#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseConnection,
    pub llm: LlmClient,
    pub vapi: VapiClient,
    pub metrics: Arc<Metrics>,
    /// Shared secret required on the Vapi webhook (`x-vapi-secret`). None
    /// disables auth — only acceptable in local dev.
    pub vapi_webhook_secret: Option<Arc<str>>,
}

impl AppState {
    pub fn new(db: DatabaseConnection) -> Self {
        let vapi_webhook_secret = std::env::var("VAPI_WEBHOOK_SECRET")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .map(Arc::from);
        if vapi_webhook_secret.is_none() {
            tracing::warn!(
                "VAPI_WEBHOOK_SECRET is unset — the Vapi webhook will accept unauthenticated posts"
            );
        }
        Self {
            db,
            llm: LlmClient::from_env(),
            vapi: VapiClient::from_env(),
            metrics: Arc::new(Metrics::default()),
            vapi_webhook_secret,
        }
    }
}
