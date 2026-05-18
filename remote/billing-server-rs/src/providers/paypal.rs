//! PayPal Partner Referrals + Log In with PayPal — observer-mode integration.
//!
//! Connection model: OAuth 2.0 authorization-code flow against PayPal Connect
//! using a Partner client. We obtain `access_token`, `refresh_token`, and the
//! tenant's merchant id. We then read transactions via the Reporting API and
//! reconcile against the TRR/SSR settlement reports (SFTP).

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::{AppError, AppResult};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PaypalCredential {
    pub merchant_id: String,
    pub access_token: String,
    pub refresh_token: String,
    pub scopes: Vec<String>,
}

pub struct PaypalOAuth<'a> {
    cfg: &'a Config,
}

impl<'a> PaypalOAuth<'a> {
    pub fn new(cfg: &'a Config) -> Self { Self { cfg } }

    pub fn authorize_url(&self, state: &str) -> AppResult<String> {
        let client_id = self.cfg.paypal_client_id.as_ref()
            .ok_or_else(|| AppError::BadRequest("PAYPAL_CLIENT_ID not configured".into()))?;
        let redirect = format!("{}/v1/oauth/paypal/callback", self.cfg.oauth_redirect_base);
        Ok(format!(
            "https://www.paypal.com/connect/\
             ?flowEntry=static&client_id={client_id}\
             &scope=openid%20https://uri.paypal.com/services/reporting/search/read\
             &redirect_uri={redirect}\
             &state={state}"
        ))
    }

    pub async fn exchange_code(&self, _code: &str) -> AppResult<PaypalCredential> {
        if self.cfg.paypal_client_secret.is_none() {
            return Err(AppError::BadRequest(
                "PAYPAL_CLIENT_SECRET not configured".into(),
            ));
        }
        Err(AppError::Provider {
            provider: "paypal".into(),
            message: "stub: implement POST /v1/oauth2/token".into(),
        })
    }
}
