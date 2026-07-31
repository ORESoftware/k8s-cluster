//! Shared application state handed to every axum handler.

use crate::error::ApiError;
use crate::metrics::Metrics;
use crate::vapi_client::{VapiAssistantConfig, VapiClient};
use sea_orm::DatabaseConnection;
use std::sync::Arc;
use t2v_llm::LlmClient;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Default ceiling on concurrent upstream LLM calls (STT/TTS/translate). Bounds
/// memory and third-party spend under load; excess requests get 503.
const DEFAULT_MAX_INFLIGHT_LLM: usize = 32;

#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseConnection,
    pub llm: LlmClient,
    pub vapi: VapiClient,
    pub metrics: Arc<Metrics>,
    /// Shared secret required on the Vapi webhook (`x-vapi-secret`).
    pub vapi_webhook_secret: Option<Arc<str>>,
    /// When the webhook secret is unset, only accept unauthenticated webhooks
    /// if this was explicitly enabled (`T2V_ALLOW_INSECURE_WEBHOOK=true`).
    pub allow_insecure_webhook: bool,
    /// Bearer secret guarding operator (`/vapi/call`) and history endpoints.
    /// None → those endpoints fail closed (503).
    pub server_auth_secret: Option<Arc<str>>,
    /// Config for the inline assistant returned on Vapi `assistant-request`.
    pub vapi_assistant: VapiAssistantConfig,
    /// Limits concurrent upstream LLM calls.
    llm_semaphore: Arc<Semaphore>,
}

fn secret_from_env(name: &str) -> Option<Arc<str>> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .map(Arc::from)
}

fn flag_from_env(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

impl AppState {
    pub fn new(db: DatabaseConnection) -> Self {
        let vapi_webhook_secret = secret_from_env("VAPI_WEBHOOK_SECRET");
        let allow_insecure_webhook = flag_from_env("T2V_ALLOW_INSECURE_WEBHOOK");
        if vapi_webhook_secret.is_none() {
            if allow_insecure_webhook {
                tracing::warn!(
                    "VAPI_WEBHOOK_SECRET unset and T2V_ALLOW_INSECURE_WEBHOOK=true — the Vapi \
                     webhook will accept UNAUTHENTICATED posts. Do not use this in production."
                );
            } else {
                tracing::warn!(
                    "VAPI_WEBHOOK_SECRET is unset — the Vapi webhook will reject all posts \
                     (fail closed). Set the secret, or T2V_ALLOW_INSECURE_WEBHOOK=true for dev."
                );
            }
        }

        let server_auth_secret = secret_from_env("T2V_SERVER_AUTH_SECRET");
        if server_auth_secret.is_none() {
            tracing::warn!(
                "T2V_SERVER_AUTH_SECRET is unset — operator (/vapi/call) and history endpoints \
                 will return 503 until it is configured."
            );
        }

        let max_inflight = std::env::var("T2V_MAX_INFLIGHT_LLM")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(DEFAULT_MAX_INFLIGHT_LLM);

        Self {
            db,
            llm: LlmClient::from_env(),
            vapi: VapiClient::from_env(),
            metrics: Arc::new(Metrics::default()),
            vapi_webhook_secret,
            allow_insecure_webhook,
            server_auth_secret,
            vapi_assistant: VapiAssistantConfig::from_env(),
            llm_semaphore: Arc::new(Semaphore::new(max_inflight)),
        }
    }

    /// Acquire a permit before an upstream LLM call. Returns 503 when the
    /// server is already at its concurrency ceiling, shedding load instead of
    /// piling up unbounded work. The permit releases when dropped.
    pub fn acquire_llm(&self) -> Result<OwnedSemaphorePermit, ApiError> {
        self.llm_semaphore.clone().try_acquire_owned().map_err(|_| {
            Metrics::bump(&self.metrics.llm_overloaded_total);
            ApiError::service_unavailable("server is at its LLM concurrency limit; retry shortly")
        })
    }
}
