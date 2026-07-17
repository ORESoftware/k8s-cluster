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

pub mod adyen;
pub mod amount;
pub mod braintree;
pub mod bridge;
pub mod circle;
pub mod coinbase;
pub mod coinflow;
pub mod connection;
pub mod dwolla;
pub mod ethereum;
pub mod fireblocks;
pub mod gocardless;
pub mod mercury;
pub mod modern_treasury;
pub mod moneygram;
pub mod oauth_common;
pub mod paypal;
pub mod plaid;
pub mod remitly;
pub mod revolut;
pub mod robinhood;
pub mod solana;
pub mod square;
pub mod stripe;
pub mod swift;
pub mod western_union;
pub mod wise;
pub mod zelle_disbursements;

#[cfg(test)]
mod api_mocks_tests;
#[cfg(test)]
pub(crate) mod mock_http;

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
    // The DB stores `gocardless` (one word) while heck snake_case would
    // produce `go_cardless`. Pin the variant explicitly.
    #[sqlx(rename = "gocardless")]
    #[serde(rename = "gocardless")]
    GoCardless,
    // Crypto houses (added 2026-05-22)
    Fireblocks,
    Circle,
    // Remittance / money-transfer partners (added 2026-06-06)
    #[sqlx(rename = "moneygram")]
    #[serde(rename = "moneygram")]
    MoneyGram,
    #[sqlx(rename = "western_union")]
    #[serde(rename = "western_union")]
    WesternUnion,
    // Bank-sponsored Zelle disbursement APIs (added 2026-06-07).
    #[sqlx(rename = "us_bank_zelle")]
    #[serde(rename = "us_bank_zelle")]
    UsBankZelle,
    #[sqlx(rename = "jpmorgan_zelle")]
    #[serde(rename = "jpmorgan_zelle")]
    JpmorganZelle,
    #[sqlx(rename = "bofa_cashpro_gdd")]
    #[serde(rename = "bofa_cashpro_gdd")]
    BofaCashProGdd,
    // Faster payment and on-chain observer rails (added 2026-06-07).
    #[sqlx(rename = "modern_treasury")]
    #[serde(rename = "modern_treasury")]
    ModernTreasury,
    #[sqlx(rename = "dwolla")]
    #[serde(rename = "dwolla")]
    Dwolla,
    #[sqlx(rename = "ethereum_wallet")]
    #[serde(rename = "ethereum_wallet")]
    EthereumWallet,
    // Card acquiring partners (added 2026-06-09): real connection +
    // webhook-signature verification; programmatic sync stubbed.
    Adyen,
    Square,
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
            Self::Fireblocks => "fireblocks",
            Self::Circle => "circle",
            Self::MoneyGram => "moneygram",
            Self::WesternUnion => "western_union",
            Self::UsBankZelle => "us_bank_zelle",
            Self::JpmorganZelle => "jpmorgan_zelle",
            Self::BofaCashProGdd => "bofa_cashpro_gdd",
            Self::ModernTreasury => "modern_treasury",
            Self::Dwolla => "dwolla",
            Self::EthereumWallet => "ethereum_wallet",
            Self::Adyen => "adyen",
            Self::Square => "square",
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
            | Self::Bridge
            | Self::Fireblocks
            | Self::Circle
            | Self::MoneyGram
            | Self::WesternUnion
            | Self::UsBankZelle
            | Self::JpmorganZelle
            | Self::BofaCashProGdd
            | Self::ModernTreasury
            | Self::Dwolla
            | Self::EthereumWallet
            | Self::Adyen
            | Self::Square => ProviderAuthKind::ApiKey,

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
            Self::Paypal | Self::Braintree | Self::PlaidBank | Self::CoinbasePrime => Full,
            Self::Fireblocks | Self::Circle => Full,
            Self::SwiftWire | Self::AchDirect => Stub,
            // Real connection + webhook-signature verification; programmatic
            // settlement/payout sync not wired yet.
            Self::Adyen | Self::Square => Stub,
            Self::ModernTreasury | Self::Dwolla => LimitedFit,
            Self::EthereumWallet => LimitedFit,
            Self::Remitly
            | Self::Robinhood
            | Self::MoneyGram
            | Self::WesternUnion
            | Self::UsBankZelle
            | Self::JpmorganZelle
            | Self::BofaCashProGdd => LimitedFit,
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
    /// We accept the connection but the provider's public/partner API either
    /// does not expose ledger-style sync or requires a tenant-specific program
    /// contract before it maps cleanly to our postings. We surface this clearly
    /// so tenants don't expect parity.
    LimitedFit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "provider_auth_kind", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ProviderAuthKind {
    // The DB stores `oauth2` (no underscore), but heck's snake_case turns
    // `OAuth2` into `o_auth2`. Pin the variant explicitly so sqlx decodes
    // the existing column values correctly.
    #[sqlx(rename = "oauth2")]
    #[serde(rename = "oauth2")]
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
