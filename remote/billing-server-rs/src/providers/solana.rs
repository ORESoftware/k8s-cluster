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

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SolanaWalletCredential {
    /// Base58-encoded Solana public key (32 bytes). NEVER store the secret key.
    pub pubkey_b58: String,
    /// SPL token mints the tenant has declared interest in tracking on this
    /// wallet. Defaults to ["USDC", "SOL"] if empty.
    #[serde(default)]
    pub tracked_mints: Vec<String>,
}

/// Verify an ed25519 signature against a 32-byte Solana pubkey.
///
pub fn verify_wallet_signature(
    pubkey_b58: &str,
    challenge: &str,
    signature_b58: &str,
) -> Result<(), String> {
    if challenge.trim().is_empty() {
        return Err("challenge must not be empty".into());
    }

    let pubkey = bs58::decode(pubkey_b58)
        .into_vec()
        .map_err(|e| format!("invalid Solana pubkey base58: {e}"))?;
    let pubkey: [u8; 32] = pubkey
        .try_into()
        .map_err(|_| "Solana pubkey must decode to 32 bytes".to_string())?;

    let signature = bs58::decode(signature_b58)
        .into_vec()
        .map_err(|e| format!("invalid Solana signature base58: {e}"))?;
    let signature: [u8; 64] = signature
        .try_into()
        .map_err(|_| "Solana signature must decode to 64 bytes".to_string())?;

    let key = VerifyingKey::from_bytes(&pubkey)
        .map_err(|e| format!("invalid ed25519 pubkey: {e}"))?;
    let sig = Signature::from_bytes(&signature);
    key.verify(challenge.as_bytes(), &sig)
        .map_err(|_| "wallet signature did not verify challenge".to_string())
}
