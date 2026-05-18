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

pub mod connection;
pub mod stripe;
pub mod paypal;
pub mod braintree;
pub mod coinbase;
pub mod plaid;
pub mod swift;
pub mod solana;

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
    PlaidBank,
    SwiftWire,
    AchDirect,
    Wise,
    SolanaWallet,
}

impl ProviderKind {
    pub fn tag(self) -> &'static str {
        match self {
            Self::Stripe => "stripe",
            Self::Paypal => "paypal",
            Self::Braintree => "braintree",
            Self::CoinbaseCommerce => "coinbase_commerce",
            Self::CoinbasePrime => "coinbase_prime",
            Self::PlaidBank => "plaid_bank",
            Self::SwiftWire => "swift_wire",
            Self::AchDirect => "ach_direct",
            Self::Wise => "wise",
            Self::SolanaWallet => "solana_wallet",
        }
    }

    pub fn auth_kind(self) -> ProviderAuthKind {
        match self {
            Self::Stripe | Self::Paypal | Self::Braintree | Self::PlaidBank => ProviderAuthKind::OAuth2,
            Self::CoinbaseCommerce | Self::CoinbasePrime | Self::Wise => ProviderAuthKind::ApiKey,
            Self::SwiftWire | Self::AchDirect => ProviderAuthKind::BankCoordinates,
            Self::SolanaWallet => ProviderAuthKind::WalletPubkey,
        }
    }
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
