//! Coinbase Commerce / Coinbase Prime — observer-mode (API key auth).
//!
//! Coinbase Commerce uses an API key + webhook signing secret. There is no
//! OAuth flow for Commerce, so the tenant generates a key in their Coinbase
//! dashboard and pastes it into our connect screen. The key is sealed
//! immediately and never leaves the server in plaintext.

use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::error::{AppError, AppResult};

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
    payload: &[u8],
    header_sig: &str,
    shared_secret: &str,
) -> AppResult<()> {
    let provided = parse_provided(header_sig);
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(shared_secret.as_bytes())
        .map_err(|e| AppError::Crypto(format!("hmac init: {e}")))?;
    Mac::update(&mut mac, payload);
    let expected = hex::encode(Mac::finalize(mac).into_bytes());
    if constant_time_eq_str(&provided, &expected) {
        Ok(())
    } else {
        Err(AppError::Unauthorized)
    }
}

fn parse_provided(header: &str) -> String {
    let trimmed = header.trim();
    if let Some(rest) = trimmed.strip_prefix("sha256=") {
        return rest.to_string();
    }
    if let Some(rest) = trimmed.strip_prefix("v1=") {
        return rest.to_string();
    }
    trimmed.to_string()
}

fn constant_time_eq_str(a: &str, b: &str) -> bool {
    let ab = a.as_bytes();
    let bb = b.as_bytes();
    if ab.len() != bb.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in ab.iter().zip(bb.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_coinbase_hmac() {
        let payload = br#"{"id":"evt","type":"charge:confirmed"}"#;
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(b"secret").unwrap();
        Mac::update(&mut mac, payload);
        let sig = hex::encode(Mac::finalize(mac).into_bytes());

        verify_commerce_signature(payload, &sig, "secret").unwrap();
    }

    #[test]
    fn rejects_bad_coinbase_hmac() {
        let err = verify_commerce_signature(b"{}", "deadbeef", "secret").unwrap_err();
        assert!(matches!(err, AppError::Unauthorized));
    }
}
