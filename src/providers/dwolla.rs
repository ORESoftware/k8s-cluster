//! Dwolla — bank transfers over ACH and instant-payment networks.
//!
//! Dwolla's transfer API can initiate and track transfers between funding
//! sources, including ACH and instant-payment rails when the tenant's Dwolla
//! program supports them. This module models that surface with typed payloads
//! and mockable HTTP calls. It does not add a public money-moving route.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::Duration;

use crate::error::{AppError, AppResult};

const PROD_BASE: &str = "https://api.dwolla.com";
const SANDBOX_BASE: &str = "https://api-sandbox.dwolla.com";
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Serialize, Deserialize)]
pub struct DwollaCredential {
    pub access_token: String,
    #[serde(default = "default_env")]
    pub environment: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook_secret: Option<String>,
}

impl fmt::Debug for DwollaCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DwollaCredential")
            .field("access_token", &"<redacted>")
            .field("environment", &self.environment)
            .field("api_base_url", &self.api_base_url)
            .field("account_id", &self.account_id)
            .field(
                "webhook_secret",
                &self.webhook_secret.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl DwollaCredential {
    pub fn base_url(&self) -> AppResult<String> {
        if let Some(url) = self.api_base_url.as_deref() {
            return normalize_base_url("dwolla.api_base_url", url, BaseUrlMode::Runtime);
        }
        if self.environment.eq_ignore_ascii_case("sandbox") {
            Ok(SANDBOX_BASE.into())
        } else {
            Ok(PROD_BASE.into())
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DwollaTransferRail {
    Ach,
    Rtp,
    FedNow,
    Wire,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DwollaTransferInput {
    pub source_funding_source_url: String,
    pub destination_funding_source_url: String,
    pub amount: String,
    #[serde(default = "default_currency")]
    pub currency: String,
    #[serde(default = "default_dwolla_rail")]
    pub rail: DwollaTransferRail,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct DwollaTransfer {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub amount: Option<DwollaAmount>,
    #[serde(default, rename = "rtpDetails", alias = "rtp_details")]
    pub rtp_details: Option<serde_json::Value>,
    #[serde(default, rename = "fedNowDetails", alias = "fed_now_details")]
    pub fed_now_details: Option<serde_json::Value>,
    #[serde(flatten)]
    pub raw: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize)]
pub struct DwollaAmount {
    pub value: String,
    pub currency: String,
}

#[derive(Clone, Debug)]
pub struct DwollaTransferCreateResponse {
    pub location: Option<String>,
    pub transfer: Option<DwollaTransfer>,
}

#[derive(Clone)]
pub struct DwollaApi {
    cred: DwollaCredential,
    http: reqwest::Client,
    base_url: String,
}

impl DwollaApi {
    pub fn new(cred: DwollaCredential) -> AppResult<Self> {
        let base_url = cred.base_url()?;
        Self::build(cred, base_url, BaseUrlMode::Runtime)
    }

    #[cfg(test)]
    pub fn with_base_url_for_tests(cred: DwollaCredential, base_url: String) -> AppResult<Self> {
        Self::build(cred, base_url, BaseUrlMode::Test)
    }

    fn build(cred: DwollaCredential, base_url: String, mode: BaseUrlMode) -> AppResult<Self> {
        let base_url = normalize_base_url("dwolla.api_base_url", &base_url, mode)?;
        Ok(Self {
            cred,
            http: http_client("dwolla")?,
            base_url,
        })
    }

    pub async fn initiate_transfer(
        &self,
        input: DwollaTransferInput,
    ) -> AppResult<DwollaTransferCreateResponse> {
        validate_transfer_input(&input)?;
        let payload = transfer_payload(&input);
        let mut req = self
            .http
            .post(format!("{}/transfers", self.base_url))
            .bearer_auth(&self.cred.access_token)
            .header("Accept", "application/vnd.dwolla.v1.hal+json")
            .header("Content-Type", "application/vnd.dwolla.v1.hal+json")
            .json(&payload);
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
            .map_err(|e| provider_err("dwolla", format!("transfer HTTP: {e}")))?;
        decode_create_response(resp).await
    }

    pub async fn get_transfer(&self, transfer_id: &str) -> AppResult<DwollaTransfer> {
        let transfer_id = required("dwolla.transfer_id", transfer_id)?;
        validate_path_segment("dwolla.transfer_id", &transfer_id)?;
        let resp = self
            .http
            .get(format!("{}/transfers/{transfer_id}", self.base_url))
            .bearer_auth(&self.cred.access_token)
            .header("Accept", "application/vnd.dwolla.v1.hal+json")
            .send()
            .await
            .map_err(|e| provider_err("dwolla", format!("get transfer HTTP: {e}")))?;
        decode_json_response("dwolla", resp, "get transfer").await
    }
}

fn transfer_payload(input: &DwollaTransferInput) -> serde_json::Value {
    let mut payload = serde_json::json!({
        "_links": {
            "source": { "href": input.source_funding_source_url },
            "destination": { "href": input.destination_funding_source_url }
        },
        "amount": {
            "currency": input.currency,
            "value": input.amount
        }
    });
    if let Some(correlation_id) = input.correlation_id.as_deref() {
        payload["correlationId"] = serde_json::json!(correlation_id);
    }
    if let Some(metadata) = input.metadata.as_ref() {
        payload["metadata"] = metadata.clone();
    }
    match input.rail {
        DwollaTransferRail::Ach => {}
        DwollaTransferRail::Rtp => {
            payload["rtpDetails"] = serde_json::json!({ "destination": "instant" });
        }
        DwollaTransferRail::FedNow => {
            payload["fedNowDetails"] = serde_json::json!({ "destination": "instant" });
        }
        DwollaTransferRail::Wire => {
            payload["wireDetails"] = serde_json::json!({});
        }
    }
    payload
}

fn default_env() -> String {
    "production".into()
}

fn default_currency() -> String {
    "USD".into()
}

fn default_dwolla_rail() -> DwollaTransferRail {
    DwollaTransferRail::Ach
}

pub fn validate_dwolla_api_base_url(value: &str) -> AppResult<String> {
    normalize_base_url("dwolla.api_base_url", value, BaseUrlMode::Runtime)
}

fn validate_transfer_input(input: &DwollaTransferInput) -> AppResult<()> {
    validate_funding_source_url(
        "dwolla.source_funding_source_url",
        &input.source_funding_source_url,
    )?;
    validate_funding_source_url(
        "dwolla.destination_funding_source_url",
        &input.destination_funding_source_url,
    )?;
    validate_decimal_amount("dwolla.amount", &input.amount)?;
    validate_currency("dwolla.currency", &input.currency)?;
    Ok(())
}

fn validate_funding_source_url(field: &str, value: &str) -> AppResult<()> {
    let trimmed = required(field, value)?;
    let parsed = url::Url::parse(&trimmed)
        .map_err(|e| AppError::BadRequest(format!("{field} must be a valid URL: {e}")))?;
    if parsed.scheme() != "https" && parsed.scheme() != "http" {
        return Err(AppError::BadRequest(format!("{field} must be an HTTP URL")));
    }
    if !parsed.path().contains("/funding-sources/") {
        return Err(AppError::BadRequest(format!(
            "{field} must reference a Dwolla funding source"
        )));
    }
    Ok(())
}

fn validate_decimal_amount(field: &str, value: &str) -> AppResult<()> {
    let trimmed = required(field, value)?;
    let parts: Vec<&str> = trimmed.split('.').collect();
    let valid = match parts.as_slice() {
        [whole] => !whole.is_empty() && whole.bytes().all(|b| b.is_ascii_digit()),
        [whole, frac] => {
            !whole.is_empty()
                && whole.bytes().all(|b| b.is_ascii_digit())
                && !frac.is_empty()
                && frac.len() <= 2
                && frac.bytes().all(|b| b.is_ascii_digit())
        }
        _ => false,
    };
    if !valid || trimmed == "0" || trimmed == "0.00" {
        return Err(AppError::BadRequest(format!(
            "{field} must be a positive decimal string with at most 2 decimals"
        )));
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

async fn decode_create_response(
    resp: reqwest::Response,
) -> AppResult<DwollaTransferCreateResponse> {
    let status = resp.status();
    let location = resp
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| provider_err("dwolla", format!("transfer body: {e}")))?;
    if !status.is_success() {
        return Err(provider_err(
            "dwolla",
            format!("transfer {status}: {}", String::from_utf8_lossy(&bytes)),
        ));
    }
    let transfer = if bytes.is_empty() {
        None
    } else {
        Some(
            serde_json::from_slice(&bytes)
                .map_err(|e| provider_err("dwolla", format!("transfer decode: {e}")))?,
        )
    };
    Ok(DwollaTransferCreateResponse { location, transfer })
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
