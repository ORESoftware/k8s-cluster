//! Coinbase Commerce / Coinbase Prime — observer-mode (API key auth).
//!
//! Coinbase Commerce uses an API key + webhook signing secret. There is no
//! OAuth flow for Commerce, so the tenant generates a key in their Coinbase
//! dashboard and pastes it into our connect screen. The key is sealed
//! immediately and never leaves the server in plaintext.

use serde::{Deserialize, Serialize};

use crate::error::AppResult;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CoinbaseCredential {
    pub api_key: String,
    pub webhook_secret: String,
    pub variant: CoinbaseVariant,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CoinbaseVariant {
    Commerce,
    Prime,
}

pub fn verify_commerce_signature(
    _payload: &[u8],
    _header_sig: &str,
    _shared_secret: &str,
) -> AppResult<()> {
    // TODO(real impl): HMAC-SHA256(payload, secret) == hex(header_sig)
    Ok(())
}
