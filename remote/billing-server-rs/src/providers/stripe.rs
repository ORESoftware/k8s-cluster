//! Stripe Connect (OAuth Standard) — observer-mode integration.
//!
//! Connection model:
//!   * Tenant clicks "Connect Stripe" in their dashboard.
//!   * We redirect to `https://connect.stripe.com/oauth/authorize?...`
//!     with `client_id`, `scope=read_write`, `redirect_uri`, `state`.
//!   * Stripe redirects back to `/v1/oauth/stripe/callback?code=…&state=…`.
//!   * We POST `code` to `https://connect.stripe.com/oauth/token`, receive
//!     `stripe_user_id`, `access_token`, `refresh_token`.
//!   * We seal `{access_token, refresh_token, stripe_user_id}` and store.
//!
//! At runtime, we never see the tenant's Stripe secret key. We act on their
//! account via the access token with `Stripe-Account: <stripe_user_id>` header.

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::{AppError, AppResult};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StripeCredential {
    pub stripe_user_id: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub livemode: bool,
}

pub struct StripeOAuth<'a> {
    cfg: &'a Config,
}

impl<'a> StripeOAuth<'a> {
    pub fn new(cfg: &'a Config) -> Self { Self { cfg } }

    pub fn authorize_url(&self, state: &str) -> AppResult<String> {
        let client_id = self.cfg.stripe_client_id.as_ref()
            .ok_or_else(|| AppError::BadRequest("STRIPE_CLIENT_ID not configured".into()))?;
        let redirect = format!("{}/v1/oauth/stripe/callback", self.cfg.oauth_redirect_base);
        Ok(format!(
            "https://connect.stripe.com/oauth/authorize\
             ?response_type=code&client_id={client_id}\
             &scope=read_write\
             &redirect_uri={redirect}\
             &state={state}"
        ))
    }

    /// Exchange the auth code for tokens.
    ///
    /// TODO(real impl): POST to https://connect.stripe.com/oauth/token with
    /// `client_secret` + `code` + `grant_type=authorization_code` and parse
    /// the JSON response. For the scaffold we return a placeholder so the
    /// surrounding flow can be exercised end-to-end without external creds.
    pub async fn exchange_code(&self, _code: &str) -> AppResult<StripeCredential> {
        if self.cfg.stripe_client_secret.is_none() {
            return Err(AppError::BadRequest(
                "STRIPE_CLIENT_SECRET not configured; cannot exchange code".into(),
            ));
        }
        Err(AppError::Provider {
            provider: "stripe".into(),
            message: "stub: implement POST to /oauth/token".into(),
        })
    }
}

// --- Ingestor --------------------------------------------------------------

/// Stripe webhook event (subset of fields we care about). Real impl should
/// validate the `Stripe-Signature` header using the webhook signing secret.
#[derive(Clone, Debug, Deserialize)]
pub struct StripeWebhookEvent {
    pub id: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub data: serde_json::Value,
    pub livemode: bool,
    pub account: Option<String>,
}

pub fn verify_signature(
    _payload: &[u8],
    _header: &str,
    _signing_secret: &str,
) -> AppResult<()> {
    // TODO(real impl): HMAC-SHA256 of "{t}.{payload}" against the v1 sig in header.
    // For now we accept everything; webhooks_events.signature_ok is set false
    // by the caller until this is implemented for real.
    Ok(())
}
