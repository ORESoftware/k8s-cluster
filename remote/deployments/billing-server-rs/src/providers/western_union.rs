//! Western Union — partner remittance/mass-payments observer.
//!
//! Western Union partner APIs are enrollment-gated and mTLS-based. We expose a
//! typed client for balance and batch-payment status endpoints, but keep sync
//! `LimitedFit` until a tenant's WU contract and settlement model are mapped.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::Duration;

use crate::error::{AppError, AppResult};

const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Serialize, Deserialize)]
pub struct WesternUnionCredential {
    pub client_id: String,
    #[serde(default = "default_env")]
    pub environment: String,
    pub client_certificate_pem: Option<String>,
    pub client_private_key_pem: Option<String>,
    pub notes: Option<String>,
}

impl fmt::Debug for WesternUnionCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WesternUnionCredential")
            .field("client_id", &self.client_id)
            .field("environment", &self.environment)
            .field(
                "client_certificate_pem",
                &self.client_certificate_pem.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "client_private_key_pem",
                &self.client_private_key_pem.as_ref().map(|_| "<redacted>"),
            )
            .field("notes", &self.notes)
            .finish()
    }
}

fn default_env() -> String {
    "production".into()
}

impl WesternUnionCredential {
    pub fn base_url(&self) -> &'static str {
        if self.environment.eq_ignore_ascii_case("sandbox") {
            "https://api-sandbox.westernunion.com"
        } else {
            "https://api.westernunion.com"
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct WesternUnionHoldingBalance {
    #[serde(default, rename = "currencyCode", alias = "currency_code")]
    pub currency_code: Option<String>,
    #[serde(default)]
    pub amount: Option<f64>,
    #[serde(flatten)]
    pub raw: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WesternUnionHoldingBalanceResponse {
    #[serde(default, rename = "clientId", alias = "client_id")]
    pub client_id: Option<String>,
    #[serde(default, rename = "currencyCode", alias = "currency_code")]
    pub currency_code: Option<String>,
    #[serde(default)]
    pub balance: Option<WesternUnionHoldingBalance>,
    #[serde(flatten)]
    pub raw: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WesternUnionPayment {
    pub id: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default, rename = "partnerReference", alias = "partner_reference")]
    pub partner_reference: Option<String>,
    #[serde(default, rename = "paymentReference", alias = "payment_reference")]
    pub payment_reference: Option<String>,
    #[serde(default)]
    pub amount: Option<i64>,
    #[serde(default, rename = "currencyCode", alias = "currency_code")]
    pub currency_code: Option<String>,
    #[serde(default, rename = "settlementAmount", alias = "settlement_amount")]
    pub settlement_amount: Option<i64>,
    #[serde(
        default,
        rename = "settlementCurrencyCode",
        alias = "settlement_currency_code"
    )]
    pub settlement_currency_code: Option<String>,
    #[serde(default, rename = "createdOn", alias = "created_on")]
    pub created_on: Option<DateTime<Utc>>,
    #[serde(default, rename = "lastUpdatedOn", alias = "last_updated_on")]
    pub last_updated_on: Option<DateTime<Utc>>,
    #[serde(flatten)]
    pub raw: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct WesternUnionPaymentsResponse {
    #[serde(default)]
    payments: Vec<WesternUnionPayment>,
}

#[derive(Clone)]
pub struct WesternUnionApi {
    cred: WesternUnionCredential,
    http: reqwest::Client,
    base_url: String,
}

impl WesternUnionApi {
    pub fn new(cred: WesternUnionCredential) -> AppResult<Self> {
        let base_url = cred.base_url().to_string();
        let http = http_client_with_optional_identity(&cred)?;
        Ok(Self {
            cred,
            http,
            base_url,
        })
    }

    #[cfg(test)]
    pub fn with_base_url_for_tests(cred: WesternUnionCredential, base_url: String) -> Self {
        Self {
            cred,
            http: reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(HTTP_TIMEOUT)
                .build()
                .expect("build Western Union test HTTP client"),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    fn base_url(&self) -> &str {
        &self.base_url
    }

    pub async fn get_holding_balance(
        &self,
        currency_code: &str,
    ) -> AppResult<WesternUnionHoldingBalanceResponse> {
        let currency = normalize_currency_code(currency_code)?;
        let url = url_with_segments(
            self.base_url(),
            &["HoldingBalance", self.cred.client_id.as_str(), currency.as_str()],
        )?;

        let resp = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| AppError::Provider {
                provider: "western_union".into(),
                message: format!("holding balance HTTP: {e}"),
            })?;
        decode_json_response(resp, "holding balance").await
    }

    pub async fn list_batch_payments(&self, batch_id: &str) -> AppResult<Vec<WesternUnionPayment>> {
        let batch_id = required_segment("western_union.batch_id", batch_id)?;
        let url = url_with_segments(
            self.base_url(),
            &[
                "customers",
                self.cred.client_id.as_str(),
                "batches",
                batch_id.as_str(),
                "payments",
            ],
        )?;

        let resp = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| AppError::Provider {
                provider: "western_union".into(),
                message: format!("batch payments HTTP: {e}"),
            })?;
        let page: WesternUnionPaymentsResponse =
            decode_json_response(resp, "batch payments").await?;
        Ok(page.payments)
    }
}

fn http_client_with_optional_identity(cred: &WesternUnionCredential) -> AppResult<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(HTTP_TIMEOUT);
    match (
        cred.client_certificate_pem.as_deref(),
        cred.client_private_key_pem.as_deref(),
    ) {
        (Some(cert), Some(key)) => {
            builder = builder.identity(client_identity_from_pem(cert, key)?);
        }
        (None, None) => {}
        _ => {
            return Err(AppError::BadRequest(
                "western_union requires both client_certificate_pem and client_private_key_pem"
                    .into(),
            ));
        }
    }

    builder.build().map_err(|e| AppError::Provider {
        provider: "western_union".into(),
        message: format!("HTTP client build: {e}"),
    })
}

pub fn validate_client_identity_pem(cert: &str, key: &str) -> AppResult<()> {
    client_identity_from_pem(cert, key).map(|_| ())
}

fn client_identity_from_pem(cert: &str, key: &str) -> AppResult<reqwest::Identity> {
    let bundle = format!("{}\n{}", cert.trim(), key.trim());
    reqwest::Identity::from_pem(bundle.as_bytes()).map_err(|e| {
        AppError::BadRequest(format!(
            "western_union client_certificate_pem/client_private_key_pem are not a valid PEM identity: {e}"
        ))
    })
}

fn normalize_currency_code(value: &str) -> AppResult<String> {
    let code = value.trim().to_ascii_uppercase();
    if code.len() != 3 || !code.bytes().all(|b| b.is_ascii_uppercase()) {
        return Err(AppError::BadRequest(
            "western_union.currency_code must be a 3-letter ISO currency code".into(),
        ));
    }
    Ok(code)
}

fn required_segment(field: &str, value: &str) -> AppResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AppError::BadRequest(format!("{field} must not be empty")));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(AppError::BadRequest(format!(
            "{field} must not contain control characters"
        )));
    }
    Ok(trimmed.to_string())
}

fn url_with_segments(base_url: &str, segments: &[&str]) -> AppResult<String> {
    let mut url = url::Url::parse(base_url).map_err(|e| AppError::Provider {
        provider: "western_union".into(),
        message: format!("base URL parse: {e}"),
    })?;
    {
        let mut path = url.path_segments_mut().map_err(|_| AppError::Provider {
            provider: "western_union".into(),
            message: "base URL cannot accept path segments".into(),
        })?;
        path.extend(segments.iter().copied());
    }
    Ok(url.to_string())
}

async fn decode_json_response<T: for<'de> Deserialize<'de>>(
    resp: reqwest::Response,
    label: &str,
) -> AppResult<T> {
    let status = resp.status();
    let bytes = resp.bytes().await.map_err(|e| AppError::Provider {
        provider: "western_union".into(),
        message: format!("{label} body: {e}"),
    })?;
    if !status.is_success() {
        return Err(AppError::Provider {
            provider: "western_union".into(),
            message: format!("{label} {status}: {}", String::from_utf8_lossy(&bytes)),
        });
    }
    serde_json::from_slice(&bytes).map_err(|e| AppError::Provider {
        provider: "western_union".into(),
        message: format!("{label} decode: {e}"),
    })
}
