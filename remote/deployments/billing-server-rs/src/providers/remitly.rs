//! Remitly — consumer remittance.
//!
//! Honest assessment: Remitly does not expose a broadly documented public
//! business API for ledger-style observation. We keep this provider classified
//! as `LimitedFit` and never run automatic sync from it.
//!
//! Some tenants can still receive partner-specific transfer exports. This
//! module models that surface with typed credentials, request wiring, and
//! response DTOs so those contracts can be tested without live calls. A
//! tenant must provide `api_base_url` and `api_key` before this client can be
//! used; there is deliberately no fake public default endpoint.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemitlyCredential {
    /// Partner API token when a tenant has a Remitly partner/export contract.
    pub api_key: Option<String>,
    /// Partner identifier, if Remitly assigned one for the tenant contract.
    pub partner_id: Option<String>,
    /// Reserved: stable recipient identifier list that we'd match
    /// against if Remitly published a transfers feed.
    #[serde(default)]
    pub watched_recipients: Vec<String>,
    /// Tenant-specific partner/export API origin. Required to instantiate
    /// `RemitlyApi`; omitted connections stay intent-only.
    pub api_base_url: Option<String>,
    #[serde(default = "default_env")]
    pub environment: String,
    pub notes: Option<String>,
}

fn default_env() -> String {
    "production".into()
}

#[derive(Clone, Debug, Deserialize)]
pub struct RemitlyTransferStatus {
    #[serde(rename = "id", alias = "transferId", alias = "transfer_id")]
    pub id: String,
    #[serde(
        default,
        rename = "recipientId",
        alias = "recipient_id",
        alias = "receiverId"
    )]
    pub recipient_id: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default, rename = "sendAmount", alias = "send_amount")]
    pub send_amount: Option<String>,
    #[serde(default, rename = "sendCurrency", alias = "send_currency")]
    pub send_currency: Option<String>,
    #[serde(default, rename = "receiveAmount", alias = "receive_amount")]
    pub receive_amount: Option<String>,
    #[serde(default, rename = "receiveCurrency", alias = "receive_currency")]
    pub receive_currency: Option<String>,
    #[serde(default, rename = "createdAt", alias = "created_at")]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default, rename = "updatedAt", alias = "updated_at")]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(flatten)]
    pub raw: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct RemitlyTransferPage {
    #[serde(default, alias = "transfers", alias = "items")]
    data: Vec<RemitlyTransferStatus>,
    #[serde(default, rename = "nextCursor", alias = "next_cursor")]
    next_cursor: Option<String>,
}

#[derive(Clone)]
pub struct RemitlyApi {
    api_key: String,
    partner_id: Option<String>,
    http: reqwest::Client,
    base_url: String,
}

impl RemitlyApi {
    pub fn new(cred: RemitlyCredential) -> AppResult<Self> {
        let api_key = required_option("remitly.api_key", cred.api_key.as_deref())?;
        let base_url = required_option("remitly.api_base_url", cred.api_base_url.as_deref())?;
        Self::build(api_key, cred.partner_id, base_url)
    }

    #[cfg(test)]
    pub fn with_base_url_for_tests(cred: RemitlyCredential, base_url: String) -> AppResult<Self> {
        let api_key = required_option("remitly.api_key", cred.api_key.as_deref())?;
        Self::build(api_key, cred.partner_id, base_url)
    }

    fn build(api_key: String, partner_id: Option<String>, base_url: String) -> AppResult<Self> {
        let base_url = normalize_base_url("remitly.api_base_url", &base_url)?;
        Ok(Self {
            api_key,
            partner_id,
            http: reqwest::Client::new(),
            base_url,
        })
    }

    fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Reads a tenant-specific Remitly partner/export transfer feed.
    ///
    /// The default path is intentionally generic (`/transfers`) because Remitly
    /// partner contracts are not public API references. The typed payload is
    /// what we can safely stabilize in this service.
    pub async fn list_partner_transfers(
        &self,
        limit: u32,
        cursor: Option<&str>,
        recipient_id: Option<&str>,
    ) -> AppResult<(Vec<RemitlyTransferStatus>, Option<String>)> {
        let mut params: Vec<(&str, String)> = vec![("limit", limit.clamp(1, 100).to_string())];
        if let Some(cursor) = cursor {
            params.push(("cursor", cursor.to_string()));
        }
        if let Some(recipient_id) = recipient_id {
            params.push(("recipientId", recipient_id.to_string()));
        }
        let qs = serde_urlencoded::to_string(&params).map_err(|e| AppError::Provider {
            provider: "remitly".into(),
            message: format!("encode query: {e}"),
        })?;
        let url = format!("{}/transfers?{qs}", self.base_url());

        let mut req = self.http.get(url).bearer_auth(&self.api_key);
        if let Some(partner_id) = self.partner_id.as_deref() {
            req = req.header("X-Remitly-Partner-Id", partner_id);
        }

        let resp = req.send().await.map_err(|e| AppError::Provider {
            provider: "remitly".into(),
            message: format!("transfers HTTP: {e}"),
        })?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| AppError::Provider {
            provider: "remitly".into(),
            message: format!("transfers body: {e}"),
        })?;
        if !status.is_success() {
            return Err(AppError::Provider {
                provider: "remitly".into(),
                message: format!("transfers {status}: {}", String::from_utf8_lossy(&bytes)),
            });
        }

        let parsed: RemitlyTransferPage =
            serde_json::from_slice(&bytes).map_err(|e| AppError::Provider {
                provider: "remitly".into(),
                message: format!("transfers decode: {e}"),
            })?;
        Ok((parsed.data, parsed.next_cursor))
    }
}

fn required_option(field: &str, value: Option<&str>) -> AppResult<String> {
    match value.map(str::trim).filter(|v| !v.is_empty()) {
        Some(v) => Ok(v.to_string()),
        None => Err(AppError::BadRequest(format!("{field} is required"))),
    }
}

fn normalize_base_url(field: &str, value: &str) -> AppResult<String> {
    let trimmed = value.trim().trim_end_matches('/');
    let parsed = url::Url::parse(trimmed)
        .map_err(|e| AppError::BadRequest(format!("{field} must be a valid URL: {e}")))?;
    if !matches!(parsed.scheme(), "https" | "http") {
        return Err(AppError::BadRequest(format!(
            "{field} must use http or https"
        )));
    }
    Ok(trimmed.to_string())
}
