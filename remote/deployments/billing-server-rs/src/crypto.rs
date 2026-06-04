//! Per-tenant credential sealing.
//!
//! Plaintext provider credentials (OAuth refresh tokens, API keys, bank
//! coordinates) NEVER live in env vars, logs, or unencrypted DB columns. We
//! seal them with AES-256-GCM. The key passed in via `BILLING_MASTER_SEAL_KEY`
//! is the platform's data key (in production, supplied by KMS / SealedSecrets
//! and rotated on a schedule).
//!
//! For real production use this module should be swapped for envelope
//! encryption with AWS KMS or Vault transit, with the data key per-tenant.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use rand::{RngExt, rng};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{AppError, AppResult};

pub struct Sealer {
    cipher: Aes256Gcm,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SealedEnvelope {
    pub ciphertext_b64: String,
    pub nonce_b64: String,
    pub aad_tag: String,
    pub version: u32,
}

impl Sealer {
    pub fn from_b64_key(b64_key: &str) -> anyhow::Result<Self> {
        let raw = B64
            .decode(b64_key.trim())
            .map_err(|e| anyhow::anyhow!("BILLING_MASTER_SEAL_KEY is not valid base64: {e}"))?;
        if raw.len() != 32 {
            anyhow::bail!(
                "BILLING_MASTER_SEAL_KEY must decode to 32 bytes, got {}",
                raw.len()
            );
        }
        let cipher = Aes256Gcm::new_from_slice(&raw)
            .map_err(|e| anyhow::anyhow!("invalid seal key: {e}"))?;
        Ok(Self { cipher })
    }

    pub fn seal(
        &self,
        tenant_id: Uuid,
        provider_tag: &str,
        plaintext: &[u8],
    ) -> AppResult<SealedEnvelope> {
        let mut nonce_bytes = [0u8; 12];
        rng().fill(&mut nonce_bytes[..]);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let aad = format!("billing-server-rs/v1|tenant={tenant_id}|provider={provider_tag}");

        let ciphertext = self
            .cipher
            .encrypt(
                nonce,
                Payload {
                    msg: plaintext,
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|e| AppError::Crypto(format!("seal failed: {e}")))?;

        Ok(SealedEnvelope {
            ciphertext_b64: B64.encode(&ciphertext),
            nonce_b64: B64.encode(nonce_bytes),
            aad_tag: aad,
            version: 1,
        })
    }

    pub fn unseal(
        &self,
        tenant_id: Uuid,
        provider_tag: &str,
        env: &SealedEnvelope,
    ) -> AppResult<Vec<u8>> {
        let expected_aad =
            format!("billing-server-rs/v1|tenant={tenant_id}|provider={provider_tag}");
        if env.aad_tag != expected_aad {
            return Err(AppError::Crypto(
                "aad tag mismatch (sealed for a different tenant/provider)".into(),
            ));
        }

        let nonce_bytes = B64
            .decode(&env.nonce_b64)
            .map_err(|e| AppError::Crypto(format!("nonce b64: {e}")))?;
        if nonce_bytes.len() != 12 {
            return Err(AppError::Crypto("nonce length must be 12".into()));
        }
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = B64
            .decode(&env.ciphertext_b64)
            .map_err(|e| AppError::Crypto(format!("ciphertext b64: {e}")))?;

        self.cipher
            .decrypt(
                nonce,
                Payload {
                    msg: &ciphertext,
                    aad: env.aad_tag.as_bytes(),
                },
            )
            .map_err(|e| AppError::Crypto(format!("unseal failed: {e}")))
    }
}
