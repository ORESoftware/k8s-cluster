//! Provider integrations.
//!
//! Each provider has three responsibilities:
//!
//!   1. **Connection** — establish auth (OAuth, API key, wallet pubkey, etc.)
//!      and store sealed credentials.
//!   2. **Ingestor** — receive webhooks and/or poll for new events, normalize
//!      to canonical postings, and write them through `LedgerService`.
//!   3. **Reconciler** — periodically pull authoritative reports / chain state
//!      and prove zero drift; raise breaks otherwise.
//!
//! For the scaffold, providers expose the right shape but most ingestor /
//! reconciler bodies are stubs marked with `// TODO(real impl)`. Filling them
//! in is the bulk of the actual engineering work; the surface around them
//! (sealing, replay, breaks, anchoring) is already in place.

pub mod braintree;
pub mod bridge;
pub mod coinbase;
pub mod coinflow;
pub mod connection;
pub mod gocardless;
pub mod mercury;
pub mod oauth_common;
pub mod paypal;
pub mod plaid;
pub mod remitly;
pub mod revolut;
pub mod robinhood;
pub mod solana;
pub mod stripe;
pub mod swift;
pub mod wise;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "provider_kind", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Stripe,
    Paypal,
    Braintree,
    CoinbaseCommerce,
    CoinbasePrime,
    Coinflow,
    PlaidBank,
    SwiftWire,
    AchDirect,
    Wise,
    SolanaWallet,
    // Card / e-money / cross-border (added 2026-05-20)
    Revolut,
    Remitly,
    Robinhood,
    Mercury,
    Bridge,
    GoCardless,
}

impl ProviderKind {
    pub fn tag(self) -> &'static str {
        match self {
            Self::Stripe => "stripe",
            Self::Paypal => "paypal",
            Self::Braintree => "braintree",
            Self::CoinbaseCommerce => "coinbase_commerce",
            Self::CoinbasePrime => "coinbase_prime",
            Self::Coinflow => "coinflow",
            Self::PlaidBank => "plaid_bank",
            Self::SwiftWire => "swift_wire",
            Self::AchDirect => "ach_direct",
            Self::Wise => "wise",
            Self::SolanaWallet => "solana_wallet",
            Self::Revolut => "revolut",
            Self::Remitly => "remitly",
            Self::Robinhood => "robinhood",
            Self::Mercury => "mercury",
            Self::Bridge => "bridge",
            Self::GoCardless => "gocardless",
        }
    }

    pub fn auth_kind(self) -> ProviderAuthKind {
        match self {
            // True OAuth2 redirect flows
            Self::Stripe
            | Self::Paypal
            | Self::Braintree
            | Self::PlaidBank
            | Self::Revolut
            | Self::GoCardless => ProviderAuthKind::OAuth2,

            // API key (or "personal access token") attached via
            // POST /v1/tenants/{t}/connections/{id}/attach-api-key
            Self::CoinbaseCommerce
            | Self::CoinbasePrime
            | Self::Wise
            | Self::Coinflow
            | Self::Remitly
            | Self::Robinhood
            | Self::Mercury
            | Self::Bridge => ProviderAuthKind::ApiKey,

            Self::SwiftWire | Self::AchDirect => ProviderAuthKind::BankCoordinates,
            Self::SolanaWallet => ProviderAuthKind::WalletPubkey,
        }
    }

    /// Human-friendly fit assessment, surfaced to tenants in the connect UI
    /// so they don't try to wire up a provider we don't actually support
    /// well yet. Hard-earned honesty saves support tickets.
    pub fn maturity(self) -> ProviderMaturity {
        use ProviderMaturity::*;
        match self {
            Self::Stripe | Self::Coinflow | Self::Wise | Self::SolanaWallet => Full,
            Self::Revolut | Self::CoinbaseCommerce => Full,
            Self::Mercury | Self::Bridge | Self::GoCardless => Full,
            Self::Paypal | Self::Braintree | Self::PlaidBank | Self::CoinbasePrime => Stub,
            Self::SwiftWire | Self::AchDirect => Stub,
            Self::Remitly | Self::Robinhood => LimitedFit,
        }
    }
}

/// What we tell tenants about a provider's integration depth.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderMaturity {
    /// Production-ready: real auth, real sync, real signature verification,
    /// real normalization templates.
    Full,
    /// Connection + sealing works, sync returns not_implemented until we
    /// finish the body. Webhooks still record to webhook_events.
    Stub,
    /// We accept the connection but the provider's public API doesn't
    /// support what we'd need to do useful work (e.g. Remitly has no B2B
    /// API; Robinhood is a brokerage, not a payments rail). We surface
    /// this clearly so tenants don't expect parity.
    LimitedFit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "provider_auth_kind", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ProviderAuthKind {
    OAuth2,
    ApiKey,
    BankCoordinates,
    WalletPubkey,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "connection_status", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ConnectionStatus {
    Pending,
    Active,
    TokenRefreshFailed,
    Revoked,
    Expired,
}
