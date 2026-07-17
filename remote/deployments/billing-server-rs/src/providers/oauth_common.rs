//! Common shape returned by every provider's OAuth (or OAuth-equivalent)
//! code-exchange call. Lets the callback router persist credentials and
//! register the backstop sync job uniformly regardless of provider.

use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct CodeExchangeResult {
    /// The provider's account id we now have access to. Becomes
    /// `provider_connections.external_account_id`.
    ///   * Stripe Connect: `stripe_user_id` (acct_...)
    ///   * PayPal: merchant id
    ///   * Braintree: merchant id
    ///   * Plaid: item id
    pub external_account_id: String,

    /// Bytes to seal and store in `sealed_credential`. The plaintext shape
    /// is provider-specific (each provider's `Credential` struct
    /// serialized to JSON). Treat this as a write-only secret — never log.
    pub sealed_plaintext: Vec<u8>,

    /// OAuth scopes the user actually granted (may be a subset of asked).
    pub scopes: Vec<String>,

    /// Token expiry, if applicable. None for providers whose access tokens
    /// don't expire (e.g. Plaid).
    pub expires_at: Option<DateTime<Utc>>,

    /// Human-friendly label for the dashboard, derived from the provider
    /// response (e.g. "Stripe acct_1ABC" or "Chase ••0123"). Optional.
    pub display_label_suggestion: Option<String>,
}
