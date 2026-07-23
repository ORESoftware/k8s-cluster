//! Argon2id password hashing on blocking worker threads.

use std::sync::OnceLock;

use argon2::password_hash::{
    rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString,
};
use argon2::{Algorithm, Argon2, Params, Version};

use crate::error::AuthError;

fn argon2() -> Argon2<'static> {
    // 32 MiB, 3 iterations, one lane. This is intentionally more expensive
    // than request parsing and always runs outside Tokio's async workers.
    let params = Params::new(32 * 1024, 3, 1, None).expect("valid Argon2 parameters");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

pub async fn hash(password: String) -> Result<String, AuthError> {
    tokio::task::spawn_blocking(move || {
        let salt = SaltString::generate(&mut OsRng);
        argon2()
            .hash_password(password.as_bytes(), &salt)
            .map(|hash| hash.to_string())
            .map_err(|error| {
                tracing::error!(%error, "Argon2 password hashing failed");
                AuthError::Internal
            })
    })
    .await
    .map_err(|error| {
        tracing::error!(%error, "password hashing worker failed");
        AuthError::Internal
    })?
}

pub async fn verify(password: String, encoded_hash: Option<String>) -> Result<bool, AuthError> {
    tokio::task::spawn_blocking(move || {
        let encoded_hash = encoded_hash.unwrap_or_else(dummy_hash);
        let parsed = PasswordHash::new(&encoded_hash).map_err(|error| {
            tracing::error!(%error, "stored password hash is invalid");
            AuthError::Internal
        })?;
        Ok(argon2()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok())
    })
    .await
    .map_err(|error| {
        tracing::error!(%error, "password verification worker failed");
        AuthError::Internal
    })?
}

/// A valid hash used when an email does not exist, keeping login timing close
/// to the existing-user path and reducing user-enumeration signal.
fn dummy_hash() -> String {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| {
        let salt = SaltString::encode_b64(b"shared-auth-dummy").expect("valid fixed salt");
        argon2()
            .hash_password(b"not-a-real-password", &salt)
            .expect("dummy password hash")
            .to_string()
    })
    .clone()
}
