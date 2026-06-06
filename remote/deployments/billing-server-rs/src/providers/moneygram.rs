//! MoneyGram — remittance status observer.
//!
//! MoneyGram publishes partner APIs for consumer transfer, business
//! disbursement, status lookup, webhooks, and support/reference data. For this
//! billing server we keep the provider `LimitedFit`: the typed client can look
//! up authoritative transfer status when a tenant has partner credentials, but
//! automatic ledger sync is not enabled until a tenant-specific remittance
//! contract is mapped into our posting model.

use base64::Engine as _;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::Duration;
use uuid::Uuid;

use crate::error::{AppError, AppResult};

const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Serialize, Deserialize)]
pub struct MoneyGramCredential {
    pub client_id: String,
    pub client_secret: String,
    pub agent_partner_id: String,
    #[serde(default = "default_user_language")]
    pub user_language: String,
    #[serde(default = "default_env")]
    pub environment: String,
    pub webhook_secret: Option<String>,
}

impl fmt::Debug for MoneyGramCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MoneyGramCredential")
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("agent_partner_id", &self.agent_partner_id)
            .field("user_language", &self.user_language)
            .field("environment", &self.environment)
            .field(
                "webhook_secret",
                &self.webhook_secret.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

fn default_env() -> String {
    "production".into()
}

fn default_user_language() -> String {
    "en-US".into()
}

impl MoneyGramCredential {
    pub fn base_url(&self) -> &'static str {
        if self.environment.eq_ignore_ascii_case("sandbox") {
            "https://sandboxapi.moneygram.com"
        } else {
            "https://api.moneygram.com"
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct MoneyGramAccessToken {
    pub access_token: String,
    #[serde(default)]
    pub token_type: Option<String>,
    #[serde(default)]
    pub expires_in: Option<String>,
    #[serde(flatten)]
    pub raw: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MoneyGramTransactionStatus {
    #[serde(default, rename = "transactionId", alias = "transaction_id")]
    pub transaction_id: Option<String>,
    #[serde(
        default,
        rename = "partnerTransactionId",
        alias = "partner_transaction_id"
    )]
    pub partner_transaction_id: Option<String>,
    #[serde(default, rename = "referenceNumber", alias = "reference_number")]
    pub reference_number: Option<String>,
    #[serde(default, rename = "transactionStatus", alias = "transaction_status")]
    pub transaction_status: Option<String>,
    #[serde(
        default,
        rename = "transactionSubStatus",
        alias = "transaction_sub_status"
    )]
    pub transaction_sub_status: Option<String>,
    #[serde(
        default,
        rename = "transactionSendDateTime",
        alias = "transaction_send_date_time"
    )]
    pub transaction_send_date_time: Option<DateTime<Utc>>,
    #[serde(default, rename = "sendAmount", alias = "send_amount")]
    pub send_amount: Option<String>,
    #[serde(default, rename = "sendCurrency", alias = "send_currency")]
    pub send_currency: Option<String>,
    #[serde(default, rename = "receiveAmount", alias = "receive_amount")]
    pub receive_amount: Option<String>,
    #[serde(default, rename = "receiveCurrency", alias = "receive_currency")]
    pub receive_currency: Option<String>,
    #[serde(flatten)]
    pub raw: serde_json::Value,
}

#[derive(Clone)]
pub struct MoneyGramApi {
    cred: MoneyGramCredential,
    http: reqwest::Client,
    base_url: String,
}

impl MoneyGramApi {
    pub fn new(cred: MoneyGramCredential) -> AppResult<Self> {
        let base_url = cred.base_url().to_string();
        Ok(Self {
            cred,
            http: http_client()?,
            base_url,
        })
    }

    #[cfg(test)]
    pub fn with_base_url_for_tests(cred: MoneyGramCredential, base_url: String) -> Self {
        Self {
            cred,
            http: http_client().expect("build MoneyGram test HTTP client"),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    fn base_url(&self) -> &str {
        &self.base_url
    }

    pub async fn get_access_token(&self) -> AppResult<MoneyGramAccessToken> {
        let auth = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!(
                "{}:{}",
                self.cred.client_id, self.cred.client_secret
            ))
        );
        let qs =
            serde_urlencoded::to_string(&[("grant_type", "client_credentials")]).map_err(|e| {
                AppError::Provider {
                    provider: "moneygram".into(),
                    message: format!("access token query encode: {e}"),
                }
            })?;
        let url = format!("{}/oauth/accesstoken?{qs}", self.base_url());

        let resp = self
            .http
            .get(url)
            .header("Authorization", auth)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| AppError::Provider {
                provider: "moneygram".into(),
                message: format!("access token HTTP: {e}"),
            })?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| AppError::Provider {
            provider: "moneygram".into(),
            message: format!("access token body: {e}"),
        })?;
        if !status.is_success() {
            return Err(AppError::Provider {
                provider: "moneygram".into(),
                message: format!("access token {status}: {}", String::from_utf8_lossy(&bytes)),
            });
        }

        let parsed: MoneyGramAccessToken =
            serde_json::from_slice(&bytes).map_err(|e| AppError::Provider {
                provider: "moneygram".into(),
                message: format!("access token decode: {e}"),
            })?;
        if parsed.access_token.trim().is_empty() {
            return Err(AppError::Provider {
                provider: "moneygram".into(),
                message: "access token response did not include access_token".into(),
            });
        }
        Ok(parsed)
    }

    pub async fn retrieve_transaction_status(
        &self,
        reference_number: &str,
        target_audience: Option<&str>,
    ) -> AppResult<MoneyGramTransactionStatus> {
        self.retrieve_status_at("/status/v1/transactions", reference_number, target_audience)
            .await
    }

    pub async fn retrieve_disbursement_status(
        &self,
        reference_number: &str,
        target_audience: Option<&str>,
    ) -> AppResult<MoneyGramTransactionStatus> {
        self.retrieve_status_at(
            "/disbursement/status/v1/transactions",
            reference_number,
            target_audience,
        )
        .await
    }

    async fn retrieve_status_at(
        &self,
        path: &str,
        reference_number: &str,
        target_audience: Option<&str>,
    ) -> AppResult<MoneyGramTransactionStatus> {
        let reference_number = required("moneygram.reference_number", reference_number)?;
        let token = self.get_access_token().await?;
        let mut params: Vec<(&str, String)> = vec![
            ("agentPartnerId", self.cred.agent_partner_id.clone()),
            ("referenceNumber", reference_number),
            ("userLanguage", self.cred.user_language.clone()),
        ];
        if let Some(target_audience) = target_audience {
            params.push((
                "targetAudience",
                required("moneygram.target_audience", target_audience)?,
            ));
        }
        let qs = serde_urlencoded::to_string(&params).map_err(|e| AppError::Provider {
            provider: "moneygram".into(),
            message: format!("status query encode: {e}"),
        })?;
        let url = format!("{}{}?{qs}", self.base_url(), path);

        let resp = self
            .http
            .get(url)
            .bearer_auth(&token.access_token)
            .header("Accept", "application/json")
            .header("X-MG-ClientRequestId", Uuid::new_v4().to_string())
            .send()
            .await
            .map_err(|e| AppError::Provider {
                provider: "moneygram".into(),
                message: format!("status HTTP: {e}"),
            })?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| AppError::Provider {
            provider: "moneygram".into(),
            message: format!("status body: {e}"),
        })?;
        if !status.is_success() {
            return Err(AppError::Provider {
                provider: "moneygram".into(),
                message: format!("status {status}: {}", String::from_utf8_lossy(&bytes)),
            });
        }

        serde_json::from_slice(&bytes).map_err(|e| AppError::Provider {
            provider: "moneygram".into(),
            message: format!("status decode: {e}"),
        })
    }
}

fn http_client() -> AppResult<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|e| AppError::Provider {
            provider: "moneygram".into(),
            message: format!("HTTP client build: {e}"),
        })
}

fn required(field: &str, value: &str) -> AppResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AppError::BadRequest(format!("{field} must not be empty")));
    }
    Ok(trimmed.to_string())
}
