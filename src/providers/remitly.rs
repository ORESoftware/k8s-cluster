//! Remitly — consumer remittance.
//!
//! Honest assessment: **Remitly does not currently expose a public
//! business / partner API for ledger-style observation**. Their
//! "Remitly for Developers" surface is focused on consumer SDKs for
//! in-app remittance flows, not on programmatic AR/AP integration.
//!
//! What we expose:
//!   * a `RemitlyCredential` shape so a tenant *can* attach an api key
//!     and a recipient list (the same fields we'd plumb if/when Remitly
//!     opens a business API)
//!   * `ProviderMaturity::LimitedFit` on `ProviderKind::Remitly` so the
//!     dashboard surfaces "no programmatic sync — see notes"
//!   * sync returns `not_implemented` — the dispatcher catches this and
//!     emits a clear summary instead of panicking
//!
//! If/when Remitly publishes a real B2B API, this module is the obvious
//! place to add it (mirror Wise/Revolut structure: API client, list
//! transfers, normalize to ledger postings, signature verify on webhooks).

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemitlyCredential {
    /// Reserved for if/when Remitly exposes a partner API key. Today
    /// this is purely a placeholder; nothing reads it.
    pub api_key: Option<String>,
    /// Reserved: stable recipient identifier list that we'd match
    /// against if Remitly published a transfers feed.
    #[serde(default)]
    pub watched_recipients: Vec<String>,
    pub notes: Option<String>,
}
