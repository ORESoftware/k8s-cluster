//! Solana wallet — observer-mode (wallet pubkey auth, READ-ONLY).
//!
//! IMPORTANT — Model A constraint:
//!
//!   This provider is STRICTLY READ-ONLY. We never custody private keys, we
//!   never request delegated SPL token authority, we never co-sign on behalf
//!   of a tenant. Tenants initiate their own crypto transfers from their own
//!   wallets; we observe the chain via the recorded pubkey and post into the
//!   ledger.
//!
//!   If you ever feel the need to add a `SolanaSpendingDelegate` here, that
//!   is a Model B feature and requires an explicit business-side decision,
//!   not just a code change. (See docs/billing-platform-brief.md.)
//!
//! Connection model:
//!   * Tenant clicks "Connect wallet" in their dashboard.
//!   * Frontend uses wallet-adapter (Phantom, Solflare, Backpack, etc.) to
//!     sign a one-time challenge ("Connect <tenant-slug> at <timestamp>").
//!   * Frontend POSTs `{pubkey, signature, challenge}` to us.
//!   * We verify the signature with ed25519, then store ONLY the pubkey.
//!
//! Sync model:
//!   * Per wallet, poll `getSignaturesForAddress(pubkey, until=last_cursor)`
//!     at chain-finalized commitment.
//!   * For each new signature, `getTransaction` -> parse SPL token transfers
//!     -> normalize to canonical posting -> write through LedgerService.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SolanaWalletCredential {
    /// Base58-encoded Solana public key (32 bytes). NEVER store the secret key.
    pub pubkey_b58: String,
    /// SPL token mints the tenant has declared interest in tracking on this
    /// wallet. Defaults to ["USDC", "SOL"] if empty.
    pub tracked_mints: Vec<String>,
}

/// Verify an ed25519 signature against a 32-byte Solana pubkey.
///
/// TODO(real impl): use ed25519-dalek or the `solana-sdk` crate. We deliberately
/// keep the heavy `solana-sdk` dep out of the scaffold to keep build times low;
/// adding it is a one-line `cargo add` when the rest of the wallet-connect
/// flow lands.
pub fn verify_wallet_signature(
    _pubkey_b58: &str,
    _challenge: &str,
    _signature_b58: &str,
) -> Result<(), String> {
    Err("stub: implement ed25519 verification against pubkey".into())
}
