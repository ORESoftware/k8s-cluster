//! Plaid Bank — observer-mode (Plaid Link OAuth-equivalent flow).
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlaidCredential {
    pub access_token: String,
    pub item_id: String,
    pub institution_id: String,
    pub institution_name: String,
}

pub struct PlaidLink<'a> { cfg: &'a Config }

impl<'a> PlaidLink<'a> {
    pub fn new(cfg: &'a Config) -> Self { Self { cfg } }

    /// Issue a Plaid Link token bound to the given tenant.
    ///
    /// TODO(real impl): POST https://production.plaid.com/link/token/create
    /// with body { client_id, secret, client_name, language, country_codes,
    /// user: { client_user_id: tenant_id }, products: ["transactions"],
    /// webhook: "<our base>/v1/webhooks/plaid" }.
    pub async fn create_link_token(&self, _tenant_id: uuid::Uuid) -> AppResult<String> {
        if self.cfg.plaid_client_id.is_none() || self.cfg.plaid_secret.is_none() {
            return Err(AppError::BadRequest(
                "PLAID_CLIENT_ID / PLAID_SECRET not configured".into(),
            ));
        }
        Err(AppError::Provider {
            provider: "plaid".into(),
            message: "stub: implement POST /link/token/create".into(),
        })
    }

    /// Exchange the Plaid Link `public_token` for a permanent `access_token`.
    pub async fn exchange_public_token(&self, _public_token: &str) -> AppResult<PlaidCredential> {
        Err(AppError::Provider {
            provider: "plaid".into(),
            message: "stub: implement POST /item/public_token/exchange".into(),
        })
    }
}
