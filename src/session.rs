//! Opaque refresh-token generation and one-way hashing.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hmac::{Hmac, KeyInit, Mac};
use rand::{rngs::SysRng, TryRng};
use sha2::{Digest, Sha256};

pub const REFRESH_TOKEN_PREFIX: &str = "sat_refresh_";
pub const MAGIC_LINK_TOKEN_PREFIX: &str = "sat_magic_";

#[derive(Clone, Debug)]
pub struct RefreshToken {
    pub plaintext: String,
    pub hash: String,
}

impl RefreshToken {
    pub fn generate() -> Self {
        let mut entropy = [0_u8; 32];
        SysRng
            .try_fill_bytes(&mut entropy)
            .expect("operating system randomness is required for refresh-token generation");
        let plaintext = format!("{REFRESH_TOKEN_PREFIX}{}", URL_SAFE_NO_PAD.encode(entropy));
        let hash = hash_token(&plaintext);
        Self { plaintext, hash }
    }
}

#[derive(Clone, Debug)]
pub struct MagicLinkToken {
    pub plaintext: String,
    pub hash: String,
    pub otp: String,
}

impl MagicLinkToken {
    pub fn generate() -> Self {
        let mut entropy = [0_u8; 32];
        SysRng
            .try_fill_bytes(&mut entropy)
            .expect("operating system randomness is required for magic-link generation");
        let plaintext = format!(
            "{MAGIC_LINK_TOKEN_PREFIX}{}",
            URL_SAFE_NO_PAD.encode(entropy)
        );
        let hash = hash_token(&plaintext);
        let otp = generate_six_digit_code();
        Self {
            plaintext,
            hash,
            otp,
        }
    }
}

pub fn hash_token(token: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(token.as_bytes()))
}

pub fn hashed_identifier(value: &str) -> String {
    hash_token(value.trim().to_ascii_lowercase().as_str())
}

pub fn hash_otp(pepper: &str, identifier: &str, otp: &str) -> String {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(pepper.as_bytes())
        .expect("HMAC accepts any key length");
    mac.update(identifier.trim().to_ascii_lowercase().as_bytes());
    mac.update(&[0]);
    mac.update(otp.as_bytes());
    URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}

fn generate_six_digit_code() -> String {
    const RANGE: u32 = 1_000_000;
    const MAX_UNBIASED: u32 = u32::MAX - (u32::MAX % RANGE);
    loop {
        let mut bytes = [0_u8; 4];
        SysRng
            .try_fill_bytes(&mut bytes)
            .expect("operating system randomness is required for OTP generation");
        let candidate = u32::from_le_bytes(bytes);
        if candidate < MAX_UNBIASED {
            return format!("{:06}", candidate % RANGE);
        }
    }
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

    #[test]
    fn magic_link_tokens_are_random_and_only_hashes_are_fixed_length() {
        let a = MagicLinkToken::generate();
        let b = MagicLinkToken::generate();
        assert_ne!(a.plaintext, b.plaintext);
        assert!(a.plaintext.starts_with(MAGIC_LINK_TOKEN_PREFIX));
        assert_eq!(a.hash.len(), 43);
        assert_eq!(a.hash, hash_token(&a.plaintext));
        assert!(!a.hash.contains(&a.plaintext));
        assert_eq!(a.otp.len(), 6);
        assert!(a.otp.bytes().all(|byte| byte.is_ascii_digit()));
    }

    #[test]
    fn otp_hashes_are_peppered_and_identifier_bound() {
        let a = hash_otp("pepper-a", "user@example.com", "123456");
        assert_eq!(a.len(), 43);
        assert_ne!(a, hash_otp("pepper-b", "user@example.com", "123456"));
        assert_ne!(a, hash_otp("pepper-a", "other@example.com", "123456"));
        assert_ne!(a, hash_otp("pepper-a", "user@example.com", "654321"));
    }
}
