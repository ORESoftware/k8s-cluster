//! Modern Treasury — ACH, RTP, wire, and stablecoin payment orders.
//!
//! Modern Treasury exposes one `Payment Order` resource across bank rails such
//! as ACH and RTP/FedNow-style real-time payments, plus stablecoin rails for
//! eligible tenants. This module gives the billing server typed request/response
//! DTOs and a mockable HTTP client, but does not expose a public money-moving
//! route. Operators can wire tenant-specific payment-order workflows later
//! without changing the connection/sealing surface.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::Duration;

use crate::error::{AppError, AppResult};

const PROD_BASE: &str = "https://app.moderntreasury.com";
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Serialize, Deserialize)]
pub struct ModernTreasuryCredential {
    pub organization_id: String,
    pub api_key: String,
    #[serde(default = "default_env")]
    pub environment: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_originating_account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook_secret: Option<String>,
}

impl fmt::Debug for ModernTreasuryCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModernTreasuryCredential")
            .field("organization_id", &self.organization_id)
            .field("api_key", &"<redacted>")
            .field("environment", &self.environment)
            .field("api_base_url", &self.api_base_url)
            .field(
                "default_originating_account_id",
                &self.default_originating_account_id,
            )
            .field(
                "webhook_secret",
                &self.webhook_secret.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl ModernTreasuryCredential {
    pub fn base_url(&self) -> AppResult<String> {
        match self.api_base_url.as_deref() {
            Some(url) => {
                normalize_base_url("modern_treasury.api_base_url", url, BaseUrlMode::Runtime)
            }
            None => Ok(PROD_BASE.into()),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModernTreasuryPaymentType {
    Ach,
    Rtp,
    Wire,
    Stablecoin,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModernTreasuryDirection {
    Credit,
    Debit,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModernTreasuryPaymentOrderInput {
    #[serde(rename = "type")]
    pub payment_type: ModernTreasuryPaymentType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_type: Option<ModernTreasuryPaymentType>,
    pub amount: i64,
    #[serde(default = "default_currency")]
    pub currency: String,
    pub direction: ModernTreasuryDirection,
    pub originating_account_id: String,
    pub receiving_account_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remittance_information: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    #[serde(skip)]
    pub idempotency_key: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ModernTreasuryPaymentOrder {
    pub id: String,
    #[serde(default, rename = "type")]
    pub payment_type: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub amount: Option<i64>,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub direction: Option<String>,
    #[serde(default)]
    pub originating_account_id: Option<String>,
    #[serde(default)]
    pub receiving_account_id: Option<String>,
    #[serde(flatten)]
    pub raw: serde_json::Value,
}

#[derive(Clone)]
pub struct ModernTreasuryApi {
    cred: ModernTreasuryCredential,
    http: reqwest::Client,
    base_url: String,
}

impl ModernTreasuryApi {
    pub fn new(cred: ModernTreasuryCredential) -> AppResult<Self> {
        let base_url = cred.base_url()?;
        Self::build(cred, base_url, BaseUrlMode::Runtime)
    }

    #[cfg(test)]
    pub fn with_base_url_for_tests(
        cred: ModernTreasuryCredential,
        base_url: String,
    ) -> AppResult<Self> {
        Self::build(cred, base_url, BaseUrlMode::Test)
    }

    fn build(
        cred: ModernTreasuryCredential,
        base_url: String,
        mode: BaseUrlMode,
    ) -> AppResult<Self> {
        let base_url = normalize_base_url("modern_treasury.api_base_url", &base_url, mode)?;
        Ok(Self {
            cred,
            http: http_client("modern_treasury")?,
            base_url,
        })
    }

    pub async fn create_payment_order(
        &self,
        input: ModernTreasuryPaymentOrderInput,
    ) -> AppResult<ModernTreasuryPaymentOrder> {
        validate_payment_order(&input)?;
        let url = format!("{}/api/payment_orders", self.base_url);
        let mut req = self
            .http
            .post(url)
            .basic_auth(&self.cred.organization_id, Some(&self.cred.api_key))
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .json(&input);
        if let Some(key) = input
            .idempotency_key
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            req = req.header("Idempotency-Key", key);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| provider_err("modern_treasury", format!("payment order HTTP: {e}")))?;
        decode_json_response("modern_treasury", resp, "payment order").await
    }

    pub async fn get_payment_order(
        &self,
        payment_order_id: &str,
    ) -> AppResult<ModernTreasuryPaymentOrder> {
        let payment_order_id = required("modern_treasury.payment_order_id", payment_order_id)?;
        validate_path_segment("modern_treasury.payment_order_id", &payment_order_id)?;
        let url = format!("{}/api/payment_orders/{payment_order_id}", self.base_url);
        let resp = self
            .http
            .get(url)
            .basic_auth(&self.cred.organization_id, Some(&self.cred.api_key))
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| provider_err("modern_treasury", format!("get payment order HTTP: {e}")))?;
        decode_json_response("modern_treasury", resp, "get payment order").await
    }
}

fn default_env() -> String {
    "production".into()
}

fn default_currency() -> String {
    "USD".into()
}

pub fn validate_modern_treasury_api_base_url(value: &str) -> AppResult<String> {
    normalize_base_url("modern_treasury.api_base_url", value, BaseUrlMode::Runtime)
}

fn validate_payment_order(input: &ModernTreasuryPaymentOrderInput) -> AppResult<()> {
    if input.amount <= 0 {
        return Err(AppError::BadRequest(
            "modern_treasury.amount must be positive minor units".into(),
        ));
    }
    validate_currency("modern_treasury.currency", &input.currency)?;
    required(
        "modern_treasury.originating_account_id",
        &input.originating_account_id,
    )?;
    required(
        "modern_treasury.receiving_account_id",
        &input.receiving_account_id,
    )?;
    if input.fallback_type == Some(input.payment_type) {
        return Err(AppError::BadRequest(
            "modern_treasury.fallback_type must differ from type".into(),
        ));
    }
    Ok(())
}

fn validate_currency(field: &str, value: &str) -> AppResult<()> {
    let trimmed = value.trim();
    if trimmed.len() != 3 || !trimmed.bytes().all(|b| b.is_ascii_uppercase()) {
        return Err(AppError::BadRequest(format!(
            "{field} must be a 3-letter uppercase ISO currency code"
        )));
    }
    Ok(())
}

fn required(field: &str, value: &str) -> AppResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AppError::BadRequest(format!("{field} must not be empty")));
    }
    Ok(trimmed.to_string())
}

fn validate_path_segment(field: &str, value: &str) -> AppResult<()> {
    if value.contains('/')
        || value.contains('?')
        || value.contains('#')
        || value.chars().any(char::is_control)
    {
        return Err(AppError::BadRequest(format!(
            "{field} must be a single URL path segment"
        )));
    }
    Ok(())
}

fn http_client(provider: &str) -> AppResult<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|e| provider_err(provider, format!("HTTP client build: {e}")))
}

#[derive(Clone, Copy)]
enum BaseUrlMode {
    Runtime,
    Test,
}

fn normalize_base_url(field: &str, value: &str, mode: BaseUrlMode) -> AppResult<String> {
    let trimmed = value.trim().trim_end_matches('/');
    let parsed = url::Url::parse(trimmed)
        .map_err(|e| AppError::BadRequest(format!("{field} must be a valid URL: {e}")))?;
    let allow_http = matches!(mode, BaseUrlMode::Test);
    if parsed.scheme() != "https" && !(allow_http && parsed.scheme() == "http") {
        return Err(AppError::BadRequest(format!("{field} must use https")));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(AppError::BadRequest(format!(
            "{field} must not include URL credentials"
        )));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(AppError::BadRequest(format!(
            "{field} must not include query or fragment components"
        )));
    }
    if matches!(mode, BaseUrlMode::Runtime) {
        validate_runtime_host(field, &parsed)?;
    }
    Ok(trimmed.to_string())
}

fn validate_runtime_host(field: &str, parsed: &url::Url) -> AppResult<()> {
    let Some(host) = parsed.host() else {
        return Err(AppError::BadRequest(format!("{field} must include a host")));
    };
    match host {
        url::Host::Domain(domain) => {
            let host = domain.trim_end_matches('.').to_ascii_lowercase();
            if host == "localhost"
                || host.ends_with(".localhost")
                || host.ends_with(".local")
                || host.ends_with(".internal")
                || !host.contains('.')
            {
                return Err(AppError::BadRequest(format!(
                    "{field} must use a public provider hostname"
                )));
            }
        }
        url::Host::Ipv4(addr) => {
            if addr.is_private()
                || addr.is_loopback()
                || addr.is_link_local()
                || addr.is_unspecified()
                || addr.is_broadcast()
                || addr.is_multicast()
            {
                return Err(AppError::BadRequest(format!(
                    "{field} must not target a private or local IP"
                )));
            }
        }
        url::Host::Ipv6(addr) => {
            if addr.is_loopback()
                || addr.is_unspecified()
                || addr.is_unique_local()
                || addr.is_unicast_link_local()
                || addr.is_multicast()
            {
                return Err(AppError::BadRequest(format!(
                    "{field} must not target a private or local IP"
                )));
            }
        }
    }
    Ok(())
}

async fn decode_json_response<T: for<'de> Deserialize<'de>>(
    provider: &str,
    resp: reqwest::Response,
    label: &str,
) -> AppResult<T> {
    let status = resp.status();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| provider_err(provider, format!("{label} body: {e}")))?;
    if !status.is_success() {
        return Err(provider_err(
            provider,
            format!("{label} {status}: {}", String::from_utf8_lossy(&bytes)),
        ));
    }
    serde_json::from_slice(&bytes)
        .map_err(|e| provider_err(provider, format!("{label} decode: {e}")))
}

fn provider_err(provider: &str, message: String) -> AppError {
    AppError::Provider {
        provider: provider.into(),
        message,
    }
}
