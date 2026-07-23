//! Opaque refresh-token generation and one-way hashing.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::rngs::OsRng;
use rand::RngCore;
use sha2::{Digest, Sha256};

pub const REFRESH_TOKEN_PREFIX: &str = "sat_refresh_";

#[derive(Clone, Debug)]
pub struct RefreshToken {
    pub plaintext: String,
    pub hash: String,
}

impl RefreshToken {
    pub fn generate() -> Self {
        let mut entropy = [0_u8; 32];
        OsRng.fill_bytes(&mut entropy);
        let plaintext = format!("{REFRESH_TOKEN_PREFIX}{}", URL_SAFE_NO_PAD.encode(entropy));
        let hash = hash_token(&plaintext);
        Self { plaintext, hash }
    }
}

pub fn hash_token(token: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(token.as_bytes()))
}

pub fn hashed_identifier(value: &str) -> String {
    hash_token(value.trim().to_ascii_lowercase().as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_tokens_are_random_and_only_hashes_are_fixed_length() {
        let a = RefreshToken::generate();
        let b = RefreshToken::generate();
        assert_ne!(a.plaintext, b.plaintext);
        assert!(a.plaintext.starts_with(REFRESH_TOKEN_PREFIX));
        assert_eq!(a.hash.len(), 43);
        assert_eq!(a.hash, hash_token(&a.plaintext));
        assert!(!a.hash.contains(&a.plaintext));
    }
}
