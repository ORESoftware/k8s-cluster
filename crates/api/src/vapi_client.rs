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
