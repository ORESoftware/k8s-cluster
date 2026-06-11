//! Sync-protocol DTOs + crypto envelope types.
//!
//! ⚠️ DUPLICATED across the frontend (`3fa-desktop.rs`) and backend
//! (`3fa-backend.rs`) repos by design — they are separate repos with no shared
//! crate. Keep the two copies byte-for-byte in sync; any divergence MUST bump
//! [`PROTOCOL_VERSION`] so a mismatch is detected at the boundary rather than
//! silently corrupting a sync.
//!
//! The cardinal rule of the zero-knowledge design is encoded in the types: the
//! server only ever handles an opaque [`SealedBlob`] of ciphertext plus version
//! metadata. There is deliberately **no** field that could hold a plaintext OTP
//! seed, password, or vault key.

use serde::{Deserialize, Serialize};

/// Protocol version negotiated between client and server. Bump on ANY breaking
/// change to the DTOs below (and to flag a copy drift between the two repos).
pub const PROTOCOL_VERSION: u32 = 1;

/// Opaque, client-encrypted vault payload. The server stores and returns this
/// verbatim and can never decrypt it: the AEAD key is derived client-side from
/// the account password and never leaves the device.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SealedBlob {
    /// AEAD ciphertext of the serialized vault (XChaCha20-Poly1305).
    pub ciphertext: Vec<u8>,
    /// 24-byte XChaCha20 nonce.
    pub nonce: Vec<u8>,
    /// KDF salt used to derive the blob key from the account password.
    pub kdf_salt: Vec<u8>,
    /// Argon2id parameters, so a client on another device can reproduce the key.
    pub kdf_params: KdfParams,
}

/// Argon2id parameters carried alongside ciphertext so any enrolled device can
/// reproduce the derivation. These are not secret.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct KdfParams {
    pub mem_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
}

impl Default for KdfParams {
    /// Interactive-but-strong defaults (~256 MiB, 3 passes).
    fn default() -> Self {
        Self {
            mem_kib: 256 * 1024,
            iterations: 3,
            parallelism: 1,
        }
    }
}

/// Per-device logical clock entry. Sync uses a version vector for
/// last-writer-wins-with-merge across devices.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VersionEntry {
    pub device_id: String,
    pub counter: u64,
}

/// A complete version vector: the causal history a blob has observed.
pub type VersionVector = Vec<VersionEntry>;

/// Request to push a new sealed vault version up to the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushRequest {
    pub device_id: String,
    pub blob: SealedBlob,
    /// The version vector the client believes is current; server rejects with
    /// [`PushResponse::Conflict`] if it has advanced past this.
    pub base_version: VersionVector,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PushResponse {
    /// Accepted; new authoritative version vector returned.
    Ok { version: VersionVector },
    /// Server is ahead. Client must pull, merge locally, and retry.
    Conflict { server_version: VersionVector },
}

/// Server's answer to a pull: the current sealed blob and its version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullResponse {
    pub blob: Option<SealedBlob>,
    pub version: VersionVector,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_blob_round_trips_json() {
        let blob = SealedBlob {
            ciphertext: vec![1, 2, 3, 4],
            nonce: vec![0u8; 24],
            kdf_salt: vec![9u8; 16],
            kdf_params: KdfParams::default(),
        };
        let json = serde_json::to_string(&blob).unwrap();
        let back: SealedBlob = serde_json::from_str(&json).unwrap();
        assert_eq!(blob, back);
    }

    #[test]
    fn default_kdf_params_are_strong() {
        let p = KdfParams::default();
        assert!(p.mem_kib >= 64 * 1024, "argon2 memory must be >= 64 MiB");
        assert!(p.iterations >= 2);
    }
}
