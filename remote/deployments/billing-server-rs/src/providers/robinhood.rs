//! Robinhood — brokerage + Robinhood Crypto.
//!
//! Honest assessment: Robinhood is a **brokerage, not a payments rail**.
//! Their public API surface (account positions, orders, transfers) does
//! not map cleanly to the AR/AP / billing-ledger model:
//!
//!   * No webhook delivery for account activity
//!   * No counterparty identifier on positions
//!   * "Transfers" are between Robinhood and a linked bank, not between
//!     two business counterparties
//!   * Robinhood Connect (in-app crypto on-ramp) is consumer-app shaped
//!
//! What we expose:
//!   * a `RobinhoodCredential` shape so a tenant can attach an OAuth
//!     access token + crypto API key (if they have one)
//!   * `ProviderMaturity::LimitedFit` so the dashboard surfaces "asset
//!     observation only — not a billing rail"
//!   * sync returns `not_implemented` until we (a) decide we want to
//!     read crypto holdings into the ledger as an asset balance and
//!     (b) get a real OAuth client setup
//!
//! If a tenant truly needs Robinhood Crypto observation, the right
//! integration target is reading `GET /v1/crypto/holdings` and posting
//! a daily snapshot to `asset/crypto/robinhood/<symbol>` — kept as a
//! follow-up because it's narrow.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RobinhoodCredential {
    pub oauth_access_token: Option<String>,
    pub oauth_refresh_token: Option<String>,
    pub crypto_api_key: Option<String>,
    pub crypto_api_secret: Option<String>,
    pub notes: Option<String>,
}
