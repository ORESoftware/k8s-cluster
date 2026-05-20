//! Braintree (PayPal-owned) — observer-mode integration.
//!
//! Braintree OAuth has been folded into the PayPal Partner platform; in
//! practice the connection flow is similar to PayPal's but yields a
//! Braintree-specific `merchant_id` + access token used against the
//! Braintree GraphQL endpoint.

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::{AppError, AppResult};

use super::oauth_common::CodeExchangeResult;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BraintreeCredential {
    pub merchant_id: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub environment: String,
    pub scopes: Vec<String>,
}

pub struct BraintreeOAuth<'a> {
    cfg: &'a Config,
}

#[derive(Debug, Deserialize)]
struct BraintreeTokenResponse {
    credentials: BraintreeCredentialsResp,
    merchant: BraintreeMerchantResp,
    scope: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BraintreeCredentialsResp {
    access_token: String,
    refresh_token: Option<String>,
    #[allow(dead_code)]
    token_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BraintreeMerchantResp {
    public_id: String,
}

#[derive(Debug, Deserialize)]
struct BraintreeErr {
    error: Option<String>,
    error_description: Option<String>,
}

impl<'a> BraintreeOAuth<'a> {
    pub fn new(cfg: &'a Config) -> Self {
        Self { cfg }
    }

    pub fn authorize_url(&self, state: &str) -> AppResult<String> {
        let client_id = self
            .cfg
            .braintree_client_id
            .as_ref()
            .ok_or_else(|| AppError::BadRequest("BRAINTREE_CLIENT_ID not configured".into()))?;
        let redirect = format!(
            "{}/v1/oauth/braintree/callback",
            self.cfg.oauth_redirect_base
        );
        let params = serde_urlencoded::to_string(&[
            ("client_id", client_id.as_str()),
            ("response_type", "code"),
            ("scope", "read_only"),
            ("redirect_uri", redirect.as_str()),
            ("state", state),
        ])
        .map_err(|e| AppError::Provider {
            provider: "braintree".into(),
            message: format!("authorize_url encode: {e}"),
        })?;
        Ok(format!(
            "{}/oauth/connect?{params}",
            self.cfg.braintree_api_base()
        ))
    }

    pub async fn exchange_code(&self, code: &str) -> AppResult<CodeExchangeResult> {
        let client_id = self
            .cfg
            .braintree_client_id
            .as_ref()
            .ok_or_else(|| AppError::BadRequest("BRAINTREE_CLIENT_ID not configured".into()))?;
        let client_secret =
            self.cfg.braintree_client_secret.as_ref().ok_or_else(|| {
                AppError::BadRequest("BRAINTREE_CLIENT_SECRET not configured".into())
            })?;

        let body = serde_urlencoded::to_string(&[
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("code", code),
            ("grant_type", "authorization_code"),
        ])
        .map_err(|e| AppError::Provider {
            provider: "braintree".into(),
            message: format!("encode form: {e}"),
        })?;
        let resp = reqwest::Client::new()
            .post(format!(
                "{}/oauth/access_tokens",
                self.cfg.braintree_api_base()
            ))
            .header("Accept", "application/json")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .map_err(|e| AppError::Provider {
                provider: "braintree".into(),
                message: format!("access_tokens HTTP: {e}"),
            })?;

        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| AppError::Provider {
            provider: "braintree".into(),
            message: format!("access_tokens body: {e}"),
        })?;
        if !status.is_success() {
            let err: BraintreeErr = serde_json::from_slice(&bytes).unwrap_or(BraintreeErr {
                error: Some(format!("http {status}")),
                error_description: Some(String::from_utf8_lossy(&bytes).into_owned()),
            });
            return Err(AppError::Provider {
                provider: "braintree".into(),
                message: format!(
                    "{}: {}",
                    err.error.unwrap_or_else(|| "error".into()),
                    err.error_description.unwrap_or_default()
                ),
            });
        }

        let parsed: BraintreeTokenResponse =
            serde_json::from_slice(&bytes).map_err(|e| AppError::Provider {
                provider: "braintree".into(),
                message: format!("access_tokens decode: {e}"),
            })?;

        let scopes: Vec<String> = parsed
            .scope
            .as_deref()
            .map(|s| s.split(',').map(str::trim).map(str::to_string).collect())
            .unwrap_or_default();

        let cred = BraintreeCredential {
            merchant_id: parsed.merchant.public_id.clone(),
            access_token: parsed.credentials.access_token,
            refresh_token: parsed.credentials.refresh_token,
            environment: self.cfg.braintree_env.as_str().into(),
            scopes: scopes.clone(),
        };
        let plaintext = serde_json::to_vec(&cred).map_err(|e| AppError::Provider {
            provider: "braintree".into(),
            message: format!("seal-encode: {e}"),
        })?;

        Ok(CodeExchangeResult {
            external_account_id: parsed.merchant.public_id.clone(),
            sealed_plaintext: plaintext,
            scopes,
            expires_at: None,
            display_label_suggestion: Some(format!("Braintree {}", parsed.merchant.public_id)),
        })
    }
}
