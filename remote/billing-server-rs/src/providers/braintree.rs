//! Braintree (PayPal-owned) — observer-mode integration.
//!
//! Braintree OAuth has been folded into the PayPal Partner platform; in
//! practice the connection flow is similar to PayPal's but yields a
//! Braintree-specific `merchant_id` + access token used against the
//! Braintree GraphQL / SOAP-ish APIs.

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::{AppError, AppResult};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BraintreeCredential {
    pub merchant_id: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub environment: String,
}

pub struct BraintreeOAuth<'a> { cfg: &'a Config }

impl<'a> BraintreeOAuth<'a> {
    pub fn new(cfg: &'a Config) -> Self { Self { cfg } }

    pub fn authorize_url(&self, state: &str) -> AppResult<String> {
        let client_id = self.cfg.braintree_client_id.as_ref()
            .ok_or_else(|| AppError::BadRequest("BRAINTREE_CLIENT_ID not configured".into()))?;
        let redirect = format!("{}/v1/oauth/braintree/callback", self.cfg.oauth_redirect_base);
        Ok(format!(
            "https://api.braintreegateway.com/oauth/connect\
             ?client_id={client_id}\
             &response_type=code\
             &scope=read_write\
             &redirect_uri={redirect}\
             &state={state}"
        ))
    }

    pub async fn exchange_code(&self, _code: &str) -> AppResult<BraintreeCredential> {
        if self.cfg.braintree_client_secret.is_none() {
            return Err(AppError::BadRequest(
                "BRAINTREE_CLIENT_SECRET not configured".into(),
            ));
        }
        Err(AppError::Provider {
            provider: "braintree".into(),
            message: "stub: implement POST /oauth/access_tokens".into(),
        })
    }
}
