use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE, URL_SAFE_NO_PAD},
    Engine as _,
};
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::error::AuthError;

use super::{SupabaseHandoffTokens, CODE_PREFIX};

type HmacSha256 = Hmac<Sha256>;
const SECRET_COMPARISON_KEY: &[u8] = b"shared-auth browser client secret comparison v1";

pub(super) struct TokenCipher(Aes256Gcm);

impl TokenCipher {
    pub fn from_encoded_key(value: &str) -> anyhow::Result<Self> {
        let key = decode_key(value).ok_or_else(|| {
            anyhow::anyhow!("AUTH_HANDOFF_ENCRYPTION_KEY must be base64 for 32 bytes")
        })?;
        let cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|_| anyhow::anyhow!("AUTH_HANDOFF_ENCRYPTION_KEY has an invalid length"))?;
        Ok(Self(cipher))
    }

    pub fn encrypt(
        &self,
        tokens: &SupabaseHandoffTokens,
        aad: &[u8],
    ) -> Result<String, AuthError> {
        let plaintext = serde_json::to_vec(tokens).map_err(|_| AuthError::Internal)?;
        let mut nonce = [0_u8; 12];
        rand::rng().fill_bytes(&mut nonce);
        let ciphertext = self
            .0
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &plaintext,
                    aad,
                },
            )
            .map_err(|_| AuthError::Internal)?;
        let mut encoded = Vec::with_capacity(nonce.len() + ciphertext.len());
        encoded.extend_from_slice(&nonce);
        encoded.extend_from_slice(&ciphertext);
        Ok(URL_SAFE_NO_PAD.encode(encoded))
    }

    pub fn decrypt(
        &self,
        encrypted: &str,
        aad: &[u8],
    ) -> Result<SupabaseHandoffTokens, AuthError> {
        let encoded = URL_SAFE_NO_PAD
            .decode(encrypted)
            .map_err(|_| AuthError::Internal)?;
        if encoded.len() <= 12 {
            return Err(AuthError::Internal);
        }
        let (nonce, ciphertext) = encoded.split_at(12);
        let plaintext = self
            .0
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad,
                },
            )
            .map_err(|_| AuthError::Internal)?;
        serde_json::from_slice(&plaintext).map_err(|_| AuthError::Internal)
    }
}

fn decode_key(value: &str) -> Option<Vec<u8>> {
    [
        STANDARD.decode(value).ok(),
        URL_SAFE_NO_PAD.decode(value).ok(),
        URL_SAFE.decode(value).ok(),
    ]
    .into_iter()
    .flatten()
    .find(|decoded| decoded.len() == 32)
}

pub(super) fn random_code() -> String {
    let mut bytes = [0_u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    format!("{CODE_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes))
}

pub(super) fn token_hash(token: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(token.as_bytes()))
}

pub(super) fn handoff_aad(client_id: &str, code_hash: &str) -> Vec<u8> {
    format!("shared-auth-browser-handoff-v1\0{client_id}\0{code_hash}").into_bytes()
}

pub(super) fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

pub(super) fn is_base64url(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

pub(super) fn constant_time_secret_eq(expected: &str, provided: &str) -> bool {
    let Ok(mut expected_mac) = <HmacSha256 as Mac>::new_from_slice(SECRET_COMPARISON_KEY) else {
        return false;
    };
    expected_mac.update(expected.as_bytes());

    let Ok(mut provided_mac) = <HmacSha256 as Mac>::new_from_slice(SECRET_COMPARISON_KEY) else {
        return false;
    };
    provided_mac.update(provided.as_bytes());
    let provided_tag = provided_mac.finalize().into_bytes();

    expected_mac.verify_slice(&provided_tag).is_ok()
}

#[cfg(test)]
mod tests {
    use super::{constant_time_secret_eq, pkce_challenge};

    #[test]
    fn pkce_is_s256_base64url() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn client_secret_comparison_is_exact() {
        let secret = "correct horse battery staple plus entropy";
        assert!(constant_time_secret_eq(secret, secret));
        assert!(!constant_time_secret_eq(secret, "wrong"));
    }
}
