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

/// Structural bounds for an opaque [`SealedBlob`] and its version vector. These
/// constrain the *shape* of an envelope (never its plaintext) so a malformed or
/// hostile blob is rejected before it can drive an oversized allocation or a
/// multi-gigabyte KDF on a peer device. They are part of the protocol contract:
/// both ends enforce them identically (server on push, client on download).
pub const NONCE_LEN: usize = 24;
pub const MIN_KDF_SALT_LEN: usize = 8;
pub const MAX_KDF_SALT_LEN: usize = 64;
pub const MAX_CIPHERTEXT_LEN: usize = 512 * 1024;
/// A version vector should never name more devices than an account could plausibly
/// enroll; caps unbounded growth from spoofed `device_id`s.
pub const MAX_VERSION_ENTRIES: usize = 64;
/// Maximum length of a `device_id` label in the version vector.
pub const MAX_DEVICE_ID_LEN: usize = 64;

/// True if `id` is a plausible device identifier: non-empty, bounded, and made
/// only of characters a client legitimately emits (UUIDs, base64url tokens,
/// `issuer:label` style ids). Rejects control chars and unbounded junk.
pub fn device_id_is_valid(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_DEVICE_ID_LEN
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':' | b'.'))
}

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

impl KdfParams {
    /// Reject params outside a sane envelope. Enforced on *both* sides so a
    /// tampered or hostile blob can't ask a peer device to allocate gigabytes or
    /// spin forever in Argon2 — and so the parameters can't be weakened below a
    /// brute-force-resistant floor (≥ 8 MiB, ≥ 1 pass).
    pub fn is_sane(&self) -> bool {
        (8 * 1024..=1024 * 1024).contains(&self.mem_kib)
            && (1..=10).contains(&self.iterations)
            && (1..=4).contains(&self.parallelism)
    }
}

impl SealedBlob {
    /// Structural validation of an opaque envelope: enforces the crypto shape
    /// (24-byte nonce, bounded salt/ciphertext) and sane KDF params *without*
    /// ever decrypting. Cheap, side-effect-free, and identical on both ends.
    pub fn is_well_formed(&self) -> bool {
        self.nonce.len() == NONCE_LEN
            && (MIN_KDF_SALT_LEN..=MAX_KDF_SALT_LEN).contains(&self.kdf_salt.len())
            && (1..=MAX_CIPHERTEXT_LEN).contains(&self.ciphertext.len())
            && self.kdf_params.is_sane()
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

    #[test]
    fn default_params_are_sane_but_extremes_are_not() {
        assert!(KdfParams::default().is_sane());
        // Memory-exhaustion attempt (~4 GiB) is rejected.
        assert!(!KdfParams { mem_kib: 4_000_000, iterations: 3, parallelism: 1 }.is_sane());
        // Weakened-to-nothing params are rejected.
        assert!(!KdfParams { mem_kib: 8, iterations: 0, parallelism: 0 }.is_sane());
    }

    #[test]
    fn well_formed_blob_accepts_valid_and_rejects_malformed() {
        let ok = SealedBlob {
            ciphertext: vec![1, 2, 3, 4],
            nonce: vec![0u8; NONCE_LEN],
            kdf_salt: vec![9u8; 16],
            kdf_params: KdfParams::default(),
        };
        assert!(ok.is_well_formed());
        // Wrong nonce length.
        let mut bad = ok.clone();
        bad.nonce = vec![0u8; 12];
        assert!(!bad.is_well_formed());
        // Oversized ciphertext.
        let mut big = ok.clone();
        big.ciphertext = vec![0u8; MAX_CIPHERTEXT_LEN + 1];
        assert!(!big.is_well_formed());
        // Empty ciphertext.
        let mut empty = ok.clone();
        empty.ciphertext.clear();
        assert!(!empty.is_well_formed());
    }

    #[test]
    fn device_id_validation() {
        assert!(device_id_is_valid("a1b2c3d4-0000-1111-2222-333344445555"));
        assert!(device_id_is_valid("GitHub:octocat.1"));
        assert!(!device_id_is_valid(""));
        assert!(!device_id_is_valid("has space"));
        assert!(!device_id_is_valid("bad\nnewline"));
        assert!(!device_id_is_valid(&"x".repeat(MAX_DEVICE_ID_LEN + 1)));
    }
}
