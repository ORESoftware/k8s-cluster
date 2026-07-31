//! Vapi.ai REST client. There is no official Rust SDK, so the REST API is
//! called directly with reqwest — same pattern as the fleet's phone screener.

use serde_json::Value;
use std::time::Duration;

const DEFAULT_VAPI_BASE: &str = "https://api.vapi.ai";

#[derive(Debug, Clone)]
pub struct VapiClient {
    http: reqwest::Client,
    base: String,
    api_key: Option<String>,
}

#[derive(Debug)]
pub enum VapiError {
    MissingApiKey,
    Http(reqwest::Error),
    Api { status: u16, body: String },
}

impl std::fmt::Display for VapiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VapiError::MissingApiKey => write!(f, "VAPI_API_KEY is not configured"),
            VapiError::Http(e) => write!(f, "vapi http error: {e}"),
            VapiError::Api { status, body } => write!(f, "vapi API returned {status}: {body}"),
        }
    }
}

impl std::error::Error for VapiError {}

impl From<reqwest::Error> for VapiError {
    fn from(e: reqwest::Error) -> Self {
        VapiError::Http(e)
    }
}

impl VapiClient {
    pub fn from_env() -> Self {
        let base = std::env::var("VAPI_BASE_URL")
            .ok()
            .map(|v| v.trim().trim_end_matches('/').to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| DEFAULT_VAPI_BASE.to_string());
        let api_key = std::env::var("VAPI_API_KEY")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client construction cannot fail with static config");
        Self {
            http,
            base,
            api_key,
        }
    }

    pub fn is_configured(&self) -> bool {
        self.api_key.is_some()
    }

    async fn check(resp: reqwest::Response) -> Result<Value, VapiError> {
        let status = resp.status();
        if !status.is_success() {
            let mut body = resp.text().await.unwrap_or_default();
            body.truncate(600);
            return Err(VapiError::Api {
                status: status.as_u16(),
                body,
            });
        }
        Ok(resp.json::<Value>().await?)
    }

    /// POST /call — start an outbound call (or web call, depending on body).
    pub async fn create_call(&self, body: &Value) -> Result<Value, VapiError> {
        let key = self.api_key.as_ref().ok_or(VapiError::MissingApiKey)?;
        let resp = self
            .http
            .post(format!("{}/call", self.base))
            .bearer_auth(key)
            .json(body)
            .send()
            .await?;
        Self::check(resp).await
    }

    /// GET /call/{id}
    pub async fn get_call(&self, id: &str) -> Result<Value, VapiError> {
        let key = self.api_key.as_ref().ok_or(VapiError::MissingApiKey)?;
        let resp = self
            .http
            .get(format!("{}/call/{}", self.base, id))
            .bearer_auth(key)
            .send()
            .await?;
        Self::check(resp).await
    }
}

impl From<VapiError> for crate::error::ApiError {
    fn from(e: VapiError) -> Self {
        use axum::http::StatusCode;
        let status = match &e {
            VapiError::MissingApiKey => StatusCode::SERVICE_UNAVAILABLE,
            VapiError::Http(_) | VapiError::Api { .. } => StatusCode::BAD_GATEWAY,
        };
        crate::error::ApiError {
            status,
            message: e.to_string(),
        }
    }
}

/// Everything needed to build the inline assistant we return to Vapi on
/// `assistant-request`. Loaded once from the environment so the model, voice,
/// greeting, and — importantly — the callback server URL/secret are operator
/// config, not hardcoded.
#[derive(Debug, Clone)]
pub struct VapiAssistantConfig {
    /// Public URL of our `/vapi/webhook`. When set, it is put on the assistant's
    /// `server` block so Vapi routes tool-calls/status/end-of-call back to us
    /// (with `server.secret` echoed as `x-vapi-secret`), closing the loop
    /// explicitly instead of relying on the phone-number-level server.
    pub server_url: Option<String>,
    pub model_provider: String,
    pub model: String,
    pub voice_provider: String,
    pub voice_id: String,
    pub transcriber_provider: Option<String>,
    pub transcriber_model: Option<String>,
    pub first_message: String,
    /// Server-tool timeout Vapi waits for our webhook, in seconds.
    pub tool_timeout_secs: u64,
}

impl VapiAssistantConfig {
    pub fn from_env() -> Self {
        let server_url = std::env::var("VAPI_SERVER_URL")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        Self {
            server_url,
            model_provider: env_or("VAPI_MODEL_PROVIDER", "openai"),
            model: env_or("VAPI_MODEL", "gpt-4o"),
            voice_provider: env_or("VAPI_VOICE_PROVIDER", "openai"),
            voice_id: env_or("VAPI_VOICE_ID", "alloy"),
            transcriber_provider: env_opt("VAPI_TRANSCRIBER_PROVIDER"),
            transcriber_model: env_opt("VAPI_TRANSCRIBER_MODEL"),
            first_message: env_or(
                "VAPI_FIRST_MESSAGE",
                "Hi! I can translate between languages in real time. \
                 What would you like translated, and into which language?",
            ),
            tool_timeout_secs: std::env::var("VAPI_TOOL_TIMEOUT_SECS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .filter(|n| (1..=120).contains(n))
                .unwrap_or(30),
        }
    }
}

fn env_opt(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn env_or(name: &str, default: &str) -> String {
    env_opt(name).unwrap_or_else(|| default.to_string())
}
