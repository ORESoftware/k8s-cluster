//! Account authentication and device tokens.
//!
//! **Today:** account login uses an Argon2id verifier — the server stores a PHC
//! hash from which the password cannot be recovered, and issues a per-device
//! bearer token (only its SHA-256 is stored).
//!
//! **Planned upgrade (seam):** replace [`verify_password`]/[`hash_password`] with
//! an OPAQUE PAKE exchange (`opaque-ke`) so the password never reaches the server
//! even transiently. The `auth_secret` column already holds an opaque string, so
//! this is a localized change. Either way the *vault* is zero-knowledge: clients
//! E2E-encrypt before upload (`shared::SealedBlob`).

use crate::error::ApiError;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use axum::http::HeaderMap;
use base64::Engine;
use rand::RngCore;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

/// An authenticated device, resolved from a bearer token.
#[derive(Debug, Clone, Copy)]
pub struct AuthedDevice {
    pub account_id: Uuid,
    /// Resolved for completeness and future per-device auditing; not yet read.
    #[allow(dead_code)]
    pub device_id: Uuid,
}

fn hasher() -> Argon2<'static> {
    // Server-side account verifier params (independent of the client KDF).
    let params = Params::new(64 * 1024, 3, 1, None).expect("valid argon2 params");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

pub fn hash_password(password: &[u8]) -> Result<String, ApiError> {
    let salt = SaltString::generate(&mut rand::rngs::OsRng);
    hasher()
        .hash_password(password, &salt)
        .map(|h| h.to_string())
        .map_err(|_| ApiError::Internal)
}

pub fn verify_password(password: &[u8], phc: &str) -> bool {
    match PasswordHash::new(phc) {
        Ok(parsed) => hasher().verify_password(password, &parsed).is_ok(),
        Err(_) => false,
    }
}

/// Generate a fresh bearer token (returned to the client once) and its stored
/// SHA-256 hex digest.
pub fn issue_token() -> (String, String) {
    let mut raw = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut raw);
    let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw);
    let hash = token_hash(&token);
    (token, hash)
}

pub fn token_hash(token: &str) -> String {
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    hex::encode(h.finalize())
}

/// Resolve the `Authorization: Bearer <token>` header to an [`AuthedDevice`],
/// rejecting revoked or unknown tokens. Constant-ish: lookup is by token hash.
pub async fn authenticate(pool: &PgPool, headers: &HeaderMap) -> Result<AuthedDevice, ApiError> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(ApiError::Unauthorized)?;

    let hash = token_hash(token);
    let row: Option<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT id, account_id FROM threefa.devices WHERE sync_token_hash = $1 AND revoked = FALSE",
    )
    .bind(&hash)
    .fetch_optional(pool)
    .await?;

    let (device_id, account_id) = row.ok_or(ApiError::Unauthorized)?;
    Ok(AuthedDevice {
        account_id,
        device_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_hash_round_trips() {
        let phc = hash_password(b"correct horse").unwrap();
        assert!(verify_password(b"correct horse", &phc));
        assert!(!verify_password(b"wrong horse", &phc));
    }

    #[test]
    fn token_hash_is_stable_and_distinct() {
        let (tok, hash) = issue_token();
        assert_eq!(token_hash(&tok), hash);
        let (tok2, _) = issue_token();
        assert_ne!(tok, tok2);
    }

    #[test]
    fn token_hash_matches_known_sha256_vectors() {
        // SHA-256("") and SHA-256("abc") — pins the digest algorithm so a stored
        // sync_token_hash never silently changes meaning.
        assert_eq!(
            token_hash(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            token_hash("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn issued_token_is_urlsafe_base64_of_32_bytes() {
        let (tok, hash) = issue_token();
        let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&tok)
            .expect("token must be url-safe base64 without padding");
        assert_eq!(raw.len(), 32);
        // Stored digest is lowercase hex of a SHA-256 (64 chars).
        assert_eq!(hash.len(), 64);
        assert!(hash.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(hash, hash.to_lowercase());
    }

    #[test]
    fn verify_password_rejects_malformed_phc() {
        // A corrupt or empty verifier string must fail closed, never panic.
        assert!(!verify_password(b"whatever", ""));
        assert!(!verify_password(b"whatever", "not-a-phc-string"));
        assert!(!verify_password(
            b"whatever",
            "$argon2id$v=19$m=65536,t=3,p=1$corrupt"
        ));
    }
}
