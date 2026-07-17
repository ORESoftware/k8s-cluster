//! PayPal Partner Referrals + Log In with PayPal — observer-mode integration.
//!
//! Connection model: OAuth 2.0 authorization-code flow against PayPal Connect
//! using a Partner client. We obtain `access_token`, `refresh_token`, and the
//! tenant's merchant id. We then read transactions via the Reporting API
//! (`/v1/reporting/transactions`) and reconcile against the TRR/SSR
//! settlement reports (SFTP).

use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::{AppError, AppResult};

use super::oauth_common::CodeExchangeResult;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PaypalCredential {
    pub merchant_id: String,
    pub access_token: String,
    pub refresh_token: String,
    pub scopes: Vec<String>,
    pub environment: String, // "live" | "sandbox"
}

pub struct PaypalOAuth<'a> {
    cfg: &'a Config,
}

#[derive(Debug, Deserialize)]
struct PaypalTokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: i64,
    scope: Option<String>,
    // Optionally returned for Log In with PayPal flows; for Partner Referrals
    // the merchant id arrives via the seller onboarding webhook instead. We
    // accept it best-effort here and fall back to "" so the connection row
    // can still be created — the merchant id is then patched in by the
    // onboarding webhook handler.
    #[serde(default)]
    payer_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PaypalErr {
    error: Option<String>,
    error_description: Option<String>,
    message: Option<String>,
}

impl<'a> PaypalOAuth<'a> {
    pub fn new(cfg: &'a Config) -> Self {
        Self { cfg }
    }

    pub fn authorize_url(&self, state: &str) -> AppResult<String> {
        let client_id = self
            .cfg
            .paypal_client_id
            .as_ref()
            .ok_or_else(|| AppError::BadRequest("PAYPAL_CLIENT_ID not configured".into()))?;
        let redirect = format!("{}/v1/oauth/paypal/callback", self.cfg.oauth_redirect_base);
        let scope = "openid https://uri.paypal.com/services/reporting/search/read";
        let params = serde_urlencoded::to_string([
            ("flowEntry", "static"),
            ("client_id", client_id.as_str()),
            ("scope", scope),
            ("redirect_uri", redirect.as_str()),
            ("state", state),
        ])
        .map_err(|e| AppError::Provider {
            provider: "paypal".into(),
            message: format!("authorize_url encode: {e}"),
        })?;
        Ok(format!(
            "{}/connect/?{params}",
            self.cfg.paypal_connect_base()
        ))
    }

    pub async fn exchange_code(&self, code: &str) -> AppResult<CodeExchangeResult> {
        let client_id = self
            .cfg
            .paypal_client_id
            .as_ref()
            .ok_or_else(|| AppError::BadRequest("PAYPAL_CLIENT_ID not configured".into()))?;
        let client_secret =
            self.cfg.paypal_client_secret.as_ref().ok_or_else(|| {
                AppError::BadRequest("PAYPAL_CLIENT_SECRET not configured".into())
            })?;

        let url = format!("{}/v1/oauth2/token", self.cfg.paypal_api_base());
        let body =
            serde_urlencoded::to_string([("grant_type", "authorization_code"), ("code", code)])
                .map_err(|e| AppError::Provider {
                    provider: "paypal".into(),
                    message: format!("encode form: {e}"),
                })?;
        let resp = reqwest::Client::new()
            .post(&url)
            .basic_auth(client_id, Some(client_secret))
            .header("content-type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .map_err(|e| AppError::Provider {
                provider: "paypal".into(),
                message: format!("oauth2/token HTTP: {e}"),
            })?;

        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| AppError::Provider {
            provider: "paypal".into(),
            message: format!("oauth2/token body: {e}"),
        })?;
        if !status.is_success() {
            let err: PaypalErr = serde_json::from_slice(&bytes).unwrap_or(PaypalErr {
                error: Some(format!("http {status}")),
                error_description: Some(String::from_utf8_lossy(&bytes).into_owned()),
                message: None,
            });
            return Err(AppError::Provider {
                provider: "paypal".into(),
                message: format!(
                    "{}: {}",
                    err.error.unwrap_or_else(|| "error".into()),
                    err.error_description.or(err.message).unwrap_or_default()
                ),
            });
        }
        let token: PaypalTokenResponse =
            serde_json::from_slice(&bytes).map_err(|e| AppError::Provider {
                provider: "paypal".into(),
                message: format!("oauth2/token decode: {e}"),
            })?;

        let merchant_id = token.payer_id.clone().unwrap_or_default();
        let cred = PaypalCredential {
            merchant_id: merchant_id.clone(),
            access_token: token.access_token,
            refresh_token: token.refresh_token,
            scopes: token
                .scope
                .as_deref()
                .map(|s| s.split_whitespace().map(str::to_string).collect())
                .unwrap_or_default(),
            environment: self.cfg.paypal_env.as_str().into(),
        };
        let plaintext = serde_json::to_vec(&cred).map_err(|e| AppError::Provider {
            provider: "paypal".into(),
            message: format!("seal-encode: {e}"),
        })?;

        Ok(CodeExchangeResult {
            external_account_id: if merchant_id.is_empty() {
                "pending".into()
            } else {
                merchant_id
            },
            sealed_plaintext: plaintext,
            scopes: cred.scopes.clone(),
            expires_at: Some(Utc::now() + Duration::seconds(token.expires_in.max(0))),
            display_label_suggestion: Some("PayPal".into()),
        })
    }
}

#[derive(Debug, Deserialize)]
struct PaypalClientToken {
    access_token: String,
}

#[derive(Debug, Deserialize)]
struct PaypalVerifyResp {
    verification_status: String,
}

pub async fn verify_webhook_signature(
    cfg: &Config,
    auth_algo: &str,
    cert_url: &str,
    transmission_id: &str,
    transmission_sig: &str,
    transmission_time: &str,
    webhook_event: &serde_json::Value,
) -> AppResult<bool> {
    let client_id = cfg
        .paypal_client_id
        .as_ref()
        .ok_or_else(|| AppError::BadRequest("PAYPAL_CLIENT_ID not configured".into()))?;
    let client_secret = cfg
        .paypal_client_secret
        .as_ref()
        .ok_or_else(|| AppError::BadRequest("PAYPAL_CLIENT_SECRET not configured".into()))?;
    let webhook_id = cfg
        .paypal_webhook_id
        .as_ref()
        .ok_or_else(|| AppError::BadRequest("PAYPAL_WEBHOOK_ID not configured".into()))?;

    let http = reqwest::Client::new();
    let token_resp = http
        .post(format!("{}/v1/oauth2/token", cfg.paypal_api_base()))
        .basic_auth(client_id, Some(client_secret))
        .header("content-type", "application/x-www-form-urlencoded")
        .body("grant_type=client_credentials")
        .send()
        .await
        .map_err(|e| AppError::Provider {
            provider: "paypal".into(),
            message: format!("client token HTTP: {e}"),
        })?;
    let status = token_resp.status();
    let bytes = token_resp.bytes().await.map_err(|e| AppError::Provider {
        provider: "paypal".into(),
        message: format!("client token body: {e}"),
    })?;
    if !status.is_success() {
        return Err(AppError::Provider {
            provider: "paypal".into(),
            message: format!("client token {status}: {}", String::from_utf8_lossy(&bytes)),
        });
    }
    let token: PaypalClientToken =
        serde_json::from_slice(&bytes).map_err(|e| AppError::Provider {
            provider: "paypal".into(),
            message: format!("client token decode: {e}"),
        })?;

    let body = serde_json::json!({
        "auth_algo": auth_algo,
        "cert_url": cert_url,
        "transmission_id": transmission_id,
        "transmission_sig": transmission_sig,
        "transmission_time": transmission_time,
        "webhook_id": webhook_id,
        "webhook_event": webhook_event,
    });

    let verify_resp = http
        .post(format!(
            "{}/v1/notifications/verify-webhook-signature",
            cfg.paypal_api_base()
        ))
        .bearer_auth(token.access_token)
        .json(&body)
        .send()
        .await
        .map_err(|e| AppError::Provider {
            provider: "paypal".into(),
            message: format!("verify-webhook-signature HTTP: {e}"),
        })?;
    let status = verify_resp.status();
    let bytes = verify_resp.bytes().await.map_err(|e| AppError::Provider {
        provider: "paypal".into(),
        message: format!("verify-webhook-signature body: {e}"),
    })?;
    if !status.is_success() {
        return Err(AppError::Provider {
            provider: "paypal".into(),
            message: format!(
                "verify-webhook-signature {status}: {}",
                String::from_utf8_lossy(&bytes)
            ),
        });
    }
    let verified: PaypalVerifyResp =
        serde_json::from_slice(&bytes).map_err(|e| AppError::Provider {
            provider: "paypal".into(),
            message: format!("verify-webhook-signature decode: {e}"),
        })?;
    Ok(verified.verification_status == "SUCCESS")
}
