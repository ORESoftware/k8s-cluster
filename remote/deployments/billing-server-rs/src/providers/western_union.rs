//! Western Union — partner remittance/mass-payments observer.
//!
//! Western Union partner APIs are enrollment-gated and mTLS-based. We expose a
//! typed client for balance and batch-payment status endpoints, but keep sync
//! `LimitedFit` until a tenant's WU contract and settlement model are mapped.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WesternUnionCredential {
    pub client_id: String,
    #[serde(default = "default_env")]
    pub environment: String,
    pub client_certificate_pem: Option<String>,
    pub client_private_key_pem: Option<String>,
    pub notes: Option<String>,
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
            http: reqwest::Client::new(),
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
        let currency = currency_code.trim().to_ascii_uppercase();
        let url = format!(
            "{}/HoldingBalance/{}/{}",
            self.base_url(),
            self.cred.client_id,
            currency
        );

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
        let url = format!(
            "{}/customers/{}/batches/{}/payments",
            self.base_url(),
            self.cred.client_id,
            batch_id
        );

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
    let mut builder = reqwest::Client::builder();
    match (
        cred.client_certificate_pem.as_deref(),
        cred.client_private_key_pem.as_deref(),
    ) {
        (Some(cert), Some(key)) => {
            let bundle = format!("{}\n{}", cert.trim(), key.trim());
            let identity = reqwest::Identity::from_pem(bundle.as_bytes()).map_err(|e| {
                AppError::Crypto(format!("western_union client certificate/key PEM: {e}"))
            })?;
            builder = builder.identity(identity);
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
