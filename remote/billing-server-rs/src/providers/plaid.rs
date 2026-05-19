//! Plaid Bank — observer-mode (Plaid Link, not standard OAuth).
//!
//! Connection model:
//!   1. Tenant clicks "Connect bank" in their dashboard.
//!   2. Frontend asks us for a Plaid Link token; we POST /link/token/create
//!      with our `client_id` + `secret` and the tenant_id as `client_user_id`.
//!   3. Frontend opens Plaid Link with that token. User picks bank + signs in
//!      with bank creds inside Plaid's iframe; we NEVER see them.
//!   4. Plaid Link returns a `public_token` to our callback.
//!   5. We POST /item/public_token/exchange to get a long-lived `access_token`
//!      and `item_id`. We seal these per tenant per institution.
//!   6. We poll /transactions/sync with the access_token + cursor.
//!
//! One tenant can have up to N (we target 10) institutions; each gets its
//! own `provider_connections` row.

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::{AppError, AppResult};

use super::oauth_common::CodeExchangeResult;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlaidCredential {
    pub access_token: String,
    pub item_id: String,
    pub institution_id: Option<String>,
    pub institution_name: Option<String>,
}

pub struct PlaidLink<'a> { cfg: &'a Config }

#[derive(Debug, Serialize)]
struct LinkTokenCreateReq<'a> {
    client_id: &'a str,
    secret: &'a str,
    client_name: &'a str,
    language: &'a str,
    country_codes: Vec<&'a str>,
    products: Vec<&'a str>,
    user: LinkUser<'a>,
    webhook: Option<String>,
}
#[derive(Debug, Serialize)]
struct LinkUser<'a> {
    client_user_id: &'a str,
}
#[derive(Debug, Deserialize)]
struct LinkTokenCreateResp {
    link_token: String,
    #[allow(dead_code)]
    expiration: Option<String>,
}

#[derive(Debug, Serialize)]
struct ExchangeReq<'a> {
    client_id: &'a str,
    secret: &'a str,
    public_token: &'a str,
}
#[derive(Debug, Deserialize)]
struct ExchangeResp {
    access_token: String,
    item_id: String,
}

#[derive(Debug, Deserialize)]
struct PlaidErr {
    error_code: Option<String>,
    error_message: Option<String>,
    error_type: Option<String>,
}

impl<'a> PlaidLink<'a> {
    pub fn new(cfg: &'a Config) -> Self { Self { cfg } }

    fn base(&self) -> &'static str {
        // Operators can switch via env later; default production.
        "https://production.plaid.com"
    }

    pub async fn create_link_token(&self, tenant_id: uuid::Uuid) -> AppResult<String> {
        let client_id = self.cfg.plaid_client_id.as_ref().ok_or_else(|| {
            AppError::BadRequest("PLAID_CLIENT_ID not configured".into())
        })?;
        let secret = self.cfg.plaid_secret.as_ref().ok_or_else(|| {
            AppError::BadRequest("PLAID_SECRET not configured".into())
        })?;

        let tenant_id_s = tenant_id.to_string();
        let webhook = Some(format!(
            "{}/v1/webhooks/plaid",
            self.cfg.oauth_redirect_base
        ));
        let body = LinkTokenCreateReq {
            client_id,
            secret,
            client_name: "billing-server",
            language: "en",
            country_codes: vec!["US"],
            products: vec!["transactions"],
            user: LinkUser { client_user_id: &tenant_id_s },
            webhook,
        };

        let resp = reqwest::Client::new()
            .post(format!("{}/link/token/create", self.base()))
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::Provider {
                provider: "plaid".into(),
                message: format!("link/token/create HTTP: {e}"),
            })?;

        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| AppError::Provider {
            provider: "plaid".into(),
            message: format!("link/token/create body: {e}"),
        })?;
        if !status.is_success() {
            return Err(plaid_err("link/token/create", status, &bytes));
        }
        let parsed: LinkTokenCreateResp =
            serde_json::from_slice(&bytes).map_err(|e| AppError::Provider {
                provider: "plaid".into(),
                message: format!("link/token/create decode: {e}"),
            })?;
        Ok(parsed.link_token)
    }

    /// Exchange the Plaid Link `public_token` for a permanent `access_token`.
    pub async fn exchange_public_token(
        &self,
        public_token: &str,
        institution_id: Option<&str>,
        institution_name: Option<&str>,
    ) -> AppResult<CodeExchangeResult> {
        let client_id = self.cfg.plaid_client_id.as_ref().ok_or_else(|| {
            AppError::BadRequest("PLAID_CLIENT_ID not configured".into())
        })?;
        let secret = self.cfg.plaid_secret.as_ref().ok_or_else(|| {
            AppError::BadRequest("PLAID_SECRET not configured".into())
        })?;

        let resp = reqwest::Client::new()
            .post(format!("{}/item/public_token/exchange", self.base()))
            .json(&ExchangeReq { client_id, secret, public_token })
            .send()
            .await
            .map_err(|e| AppError::Provider {
                provider: "plaid".into(),
                message: format!("public_token/exchange HTTP: {e}"),
            })?;

        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| AppError::Provider {
            provider: "plaid".into(),
            message: format!("public_token/exchange body: {e}"),
        })?;
        if !status.is_success() {
            return Err(plaid_err("public_token/exchange", status, &bytes));
        }
        let parsed: ExchangeResp =
            serde_json::from_slice(&bytes).map_err(|e| AppError::Provider {
                provider: "plaid".into(),
                message: format!("public_token/exchange decode: {e}"),
            })?;

        let cred = PlaidCredential {
            access_token: parsed.access_token,
            item_id: parsed.item_id.clone(),
            institution_id: institution_id.map(String::from),
            institution_name: institution_name.map(String::from),
        };
        let plaintext = serde_json::to_vec(&cred).map_err(|e| AppError::Provider {
            provider: "plaid".into(),
            message: format!("seal-encode: {e}"),
        })?;

        Ok(CodeExchangeResult {
            external_account_id: parsed.item_id.clone(),
            sealed_plaintext: plaintext,
            scopes: vec!["transactions".into()],
            // Plaid access_tokens don't expire.
            expires_at: None,
            display_label_suggestion: Some(
                institution_name
                    .map(|n| format!("Plaid {n}"))
                    .unwrap_or_else(|| format!("Plaid {}", parsed.item_id)),
            ),
        })
    }
}

fn plaid_err(op: &str, status: reqwest::StatusCode, bytes: &[u8]) -> AppError {
    let err: PlaidErr = serde_json::from_slice(bytes).unwrap_or(PlaidErr {
        error_code: Some(format!("http {status}")),
        error_message: Some(String::from_utf8_lossy(bytes).into_owned()),
        error_type: None,
    });
    AppError::Provider {
        provider: "plaid".into(),
        message: format!(
            "{op} failed [{}/{}]: {}",
            err.error_type.unwrap_or_else(|| "?".into()),
            err.error_code.unwrap_or_else(|| "?".into()),
            err.error_message.unwrap_or_default()
        ),
    }
}
