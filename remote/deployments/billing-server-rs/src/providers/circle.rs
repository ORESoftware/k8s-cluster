//! Circle Mint — USDC issuer (observer-mode integration).
//!
//! Circle Mint is the institutional on-ramp / off-ramp for USDC. It's
//! the only provider where stablecoin amounts can be treated as 1:1 USD
//! with full confidence — Circle redeems USDC for USD on demand from
//! their reserves.
//!
//! Connection: API key (bearer). Endpoint: `https://api.circle.com` or
//! `https://api-sandbox.circle.com` depending on environment.
//!
//! Sync surface (v1 API, still the actively-maintained one for Circle
//! Mint accounts):
//!   - `GET /v1/businessAccount/balances` — current balance per currency
//!   - `GET /v1/businessAccount/transfers` — paginated transfers
//!   - `GET /v1/businessAccount/payouts` — paginated payouts to bank
//!   - `GET /v1/businessAccount/deposits` — paginated deposits from bank
//!
//! Webhook model: SNS-style with HMAC signing (notification payloads
//! are POSTed by Circle to a tenant-configured URL; signature is in
//! `circle-signature` header, HMAC-SHA256 over the raw body).

use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::error::{AppError, AppResult};
use crate::providers::amount::constant_time_eq_str;

const PROD_BASE: &str = "https://api.circle.com";
const SANDBOX_BASE: &str = "https://api-sandbox.circle.com";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CircleCredential {
    /// Circle API key (`SAND_API_KEY:` / `LIVE_API_KEY:`-prefixed by Circle).
    pub api_key: String,
    /// "production" | "sandbox".
    pub environment: String,
    /// HMAC secret for webhook signature verification. Optional —
    /// tenants who haven't configured webhooks can still attach.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook_secret: Option<String>,
}

impl CircleCredential {
    pub fn api_base(&self) -> &'static str {
        if self.environment.eq_ignore_ascii_case("production")
            || self.environment.eq_ignore_ascii_case("live")
        {
            PROD_BASE
        } else {
            SANDBOX_BASE
        }
    }
}

// =========================================================================
// Wire types
// =========================================================================

#[derive(Debug, Deserialize)]
pub struct TransfersPage {
    pub data: Vec<CircleTransfer>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CircleTransfer {
    pub id: String,
    pub source: Option<CircleEndpoint>,
    pub destination: Option<CircleEndpoint>,
    pub amount: Option<CircleAmount>,
    /// One of: pending, complete, failed.
    pub status: Option<String>,
    #[serde(rename = "createDate")]
    pub create_date: Option<String>,
    #[serde(rename = "transactionHash")]
    pub transaction_hash: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CircleEndpoint {
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub id: Option<String>,
    pub chain: Option<String>,
    pub address: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CircleAmount {
    pub amount: String,
    pub currency: String,
}

// =========================================================================
// API client
// =========================================================================

pub struct CircleApi {
    cred: CircleCredential,
    http: reqwest::Client,
    base_url: String,
}

impl CircleApi {
    pub fn new(cred: CircleCredential) -> Self {
        let base_url = cred.api_base().to_string();
        Self {
            cred,
            http: reqwest::Client::new(),
            base_url,
        }
    }

    #[cfg(test)]
    pub fn with_base_url_for_tests(cred: CircleCredential, base_url: String) -> Self {
        Self {
            cred,
            http: reqwest::Client::new(),
            base_url,
        }
    }

    fn base_url(&self) -> &str {
        &self.base_url
    }

    /// `GET /v1/businessAccount/transfers` — cursor-paginated transfers
    /// using `pageBefore` / `pageAfter`. We walk forward using
    /// `pageAfter` (Circle's preferred forward-walk cursor).
    pub async fn list_transfers(
        &self,
        page_after: Option<&str>,
        page_size: u32,
    ) -> AppResult<TransfersPage> {
        let mut path = format!("/v1/businessAccount/transfers?pageSize={page_size}");
        if let Some(cursor) = page_after {
            path.push_str(&format!("&pageAfter={cursor}"));
        }
        let url = format!("{}{}", self.base_url(), path);
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.cred.api_key)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| AppError::Provider {
                provider: "circle".into(),
                message: format!("transfers HTTP: {e}"),
            })?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| AppError::Provider {
            provider: "circle".into(),
            message: format!("transfers body: {e}"),
        })?;
        if !status.is_success() {
            return Err(AppError::Provider {
                provider: "circle".into(),
                message: format!("transfers {status}: {}", String::from_utf8_lossy(&bytes)),
            });
        }
        let parsed: TransfersPage =
            serde_json::from_slice(&bytes).map_err(|e| AppError::Provider {
                provider: "circle".into(),
                message: format!("transfers decode: {e}"),
            })?;
        Ok(parsed)
    }
}

// =========================================================================
// Webhook signature verification (HMAC-SHA256)
// =========================================================================

pub fn verify_webhook_signature(
    body: &[u8],
    signature_header: &str,
    shared_secret: &str,
) -> AppResult<()> {
    // Circle's `circle-signature` header is bare hex; some tenants
    // proxy webhooks through middleware that prepends `sha256=` — be
    // tolerant of both.
    let provided_hex = signature_header
        .trim()
        .strip_prefix("sha256=")
        .unwrap_or(signature_header.trim());

    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(shared_secret.as_bytes())
        .map_err(|e| AppError::Crypto(format!("hmac init: {e}")))?;
    Mac::update(&mut mac, body);
    let expected = hex::encode(Mac::finalize(mac).into_bytes());
    if constant_time_eq_str(provided_hex, &expected) {
        Ok(())
    } else {
        Err(AppError::Unauthorized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_circle_hmac() {
        let payload = br#"{"id":"t","type":"transfers"}"#;
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(b"secret").unwrap();
        Mac::update(&mut mac, payload);
        let sig = hex::encode(Mac::finalize(mac).into_bytes());
        verify_webhook_signature(payload, &sig, "secret").unwrap();
    }

    #[test]
    fn rejects_bad_circle_hmac() {
        let err = verify_webhook_signature(b"{}", "deadbeef", "secret").unwrap_err();
        assert!(matches!(err, AppError::Unauthorized));
    }
}
