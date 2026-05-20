//! Wise Platform — observer-mode multi-currency activity integration.
//!
//! Wise is connected with a tenant-owned API token and profile id. Under the
//! repo's Model A posture we read activity / statement data only; tenants
//! initiate transfers from Wise itself.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WiseCredential {
    pub api_token: String,
    pub profile_id: String,
    #[serde(default = "default_env")]
    pub environment: String,
}

fn default_env() -> String {
    "production".into()
}

impl WiseCredential {
    pub fn base_url(&self) -> &'static str {
        if self.environment.eq_ignore_ascii_case("sandbox") {
            "https://api.wise-sandbox.com"
        } else {
            "https://api.wise.com"
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct WiseActivity {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub resource: Option<WiseResource>,
    pub status: Option<String>,
    #[serde(rename = "primaryAmount")]
    pub primary_amount: Option<String>,
    #[serde(rename = "secondaryAmount")]
    pub secondary_amount: Option<String>,
    #[serde(rename = "createdOn")]
    pub created_on: Option<DateTime<Utc>>,
    #[serde(rename = "updatedOn")]
    pub updated_on: Option<DateTime<Utc>>,
    #[serde(flatten)]
    pub raw: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WiseResource {
    #[serde(rename = "type")]
    pub kind: String,
    pub id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WiseActivityList {
    cursor: Option<String>,
    activities: Vec<WiseActivity>,
}

#[derive(Clone)]
pub struct WiseApi {
    cred: WiseCredential,
    http: reqwest::Client,
}

impl WiseApi {
    pub fn new(cred: WiseCredential) -> Self {
        Self {
            cred,
            http: reqwest::Client::new(),
        }
    }

    pub async fn list_activities(
        &self,
        next_cursor: Option<&str>,
        since: Option<DateTime<Utc>>,
        until: Option<DateTime<Utc>>,
        size: u32,
    ) -> AppResult<(Vec<WiseActivity>, Option<String>)> {
        let mut params: Vec<(&str, String)> = vec![("size", size.clamp(1, 100).to_string())];
        if let Some(cursor) = next_cursor {
            params.push(("nextCursor", cursor.to_string()));
        }
        if let Some(since) = since {
            params.push(("since", since.to_rfc3339()));
        }
        if let Some(until) = until {
            params.push(("until", until.to_rfc3339()));
        }
        let qs = serde_urlencoded::to_string(&params).map_err(|e| AppError::Provider {
            provider: "wise".into(),
            message: format!("encode query: {e}"),
        })?;
        let url = format!(
            "{}/v1/profiles/{}/activities?{qs}",
            self.cred.base_url(),
            self.cred.profile_id
        );

        let resp = self
            .http
            .get(url)
            .bearer_auth(&self.cred.api_token)
            .send()
            .await
            .map_err(|e| AppError::Provider {
                provider: "wise".into(),
                message: format!("activities HTTP: {e}"),
            })?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| AppError::Provider {
            provider: "wise".into(),
            message: format!("activities body: {e}"),
        })?;
        if !status.is_success() {
            return Err(AppError::Provider {
                provider: "wise".into(),
                message: format!("activities {status}: {}", String::from_utf8_lossy(&bytes)),
            });
        }

        let parsed: WiseActivityList =
            serde_json::from_slice(&bytes).map_err(|e| AppError::Provider {
                provider: "wise".into(),
                message: format!("activities decode: {e}"),
            })?;
        Ok((parsed.activities, parsed.cursor))
    }
}
