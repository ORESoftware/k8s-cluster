//! Onion cryptography: per-hop key agreement and layered AEAD.
//!
//! Handshake (an ntor-like construction, simplified):
//!   * Each relay owns a long-term X25519 static keypair; its public key is
//!     published in the directory.
//!   * The client, per hop, generates an ephemeral X25519 key `x`.
//!   * Client sends protocol marker `TSR2` and `X = x·G` as the CREATE blob.
//!   * The relay generates its own ephemeral `y`, and both sides compute the
//!     same shared secret from two Diffie-Hellman results:
//!         s1 = DH(client_eph, relay_static)   (authenticates the relay)
//!         s2 = DH(client_eph, relay_eph)       (forward secrecy)
//!     keyed material = HKDF-SHA256(salt, s1 || s2).
//!   * The relay replies CREATED = `TSR2` || `Y = y·G` || auth-MAC,
//!     where the MAC is HMAC-SHA256 over the transcript keyed by a derived
//!     auth key. The client verifies the MAC, proving the relay holds the
//!     static secret matching the directory public key.
//!
//! This is a pragmatic, readable handshake, not the formally analyzed Tor ntor.
//! It gives relay authentication and forward secrecy, which is what the onion
//! layering relies on.

use anyhow::{bail, Result};
use chacha20poly1305::aead::Aead;
use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use rand::rngs::OsRng;
use sha2::Sha256;
use std::sync::OnceLock;
use subtle::ConstantTimeEq;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroize;

type HmacSha256 = Hmac<Sha256>;

const HANDSHAKE_MARKER: &[u8; 4] = b"TSR2";
const HKDF_SALT: &[u8] = b"tor-server.rs/v2/onion-handshake";
const HKDF_INFO: &[u8] = b"tor-server.rs/v2/directional-keys";
const AUTH_ROLE: &[u8] = b"tor-server.rs/v2/relay-auth";

/// Optional overlay-membership pre-shared key. When set (via `TOR_NETWORK_SECRET`),
/// it is folded into every hop's key derivation, so a client and relay only
/// agree on keys — and pass the handshake's auth MAC — if they share the same
/// secret. Empty (the default) means an open overlay. Set once at startup.
static NETWORK_SECRET: OnceLock<Vec<u8>> = OnceLock::new();

/// Install the overlay pre-shared key. Idempotent; only the first call wins.
pub fn set_network_secret(secret: Vec<u8>) {
    let _ = NETWORK_SECRET.set(secret);
}

fn network_secret() -> &'static [u8] {
    return NETWORK_SECRET.get().map(|v| v.as_slice()).unwrap_or(&[]);
}

/// The directional symmetric keys derived from one hop's handshake.
pub struct HopKeys {
    /// Client -> relay direction.
    pub forward: [u8; 32],
    /// Relay -> client direction.
    pub backward: [u8; 32],
}

fn derive_keys(
    s1: &[u8; 32],
    s2: &[u8; 32],
    relay_static_pub: &[u8; 32],
    client_pub: &[u8; 32],
    relay_eph_pub: &[u8; 32],
) -> (HopKeys, [u8; 32]) {
    let psk = network_secret();
    let mut ikm = Vec::with_capacity(64 + 4 + 96 + psk.len());
    ikm.extend_from_slice(s1);
    ikm.extend_from_slice(s2);
    ikm.extend_from_slice(HANDSHAKE_MARKER);
    ikm.extend_from_slice(relay_static_pub);
    ikm.extend_from_slice(client_pub);
    ikm.extend_from_slice(relay_eph_pub);
    ikm.extend_from_slice(psk);
    let hk = Hkdf::<Sha256>::new(Some(HKDF_SALT), &ikm);
    let mut okm = [0u8; 96];
    hk.expand(HKDF_INFO, &mut okm)
        .expect("96 bytes is a valid HKDF length");

    let mut forward = [0u8; 32];
    let mut backward = [0u8; 32];
    let mut auth_key = [0u8; 32];
    forward.copy_from_slice(&okm[0..32]);
    backward.copy_from_slice(&okm[32..64]);
    auth_key.copy_from_slice(&okm[64..96]);

    let mut mac =
        <HmacSha256 as Mac>::new_from_slice(&auth_key).expect("hmac accepts any key length");
    mac.update(HANDSHAKE_MARKER);
    mac.update(AUTH_ROLE);
    mac.update(relay_static_pub);
    mac.update(client_pub);
    mac.update(relay_eph_pub);
    let tag: [u8; 32] = mac.finalize().into_bytes().into();

    // Wipe the transient key material (the concatenated DH secrets and the full
    // 96-byte KDF output). `forward`/`backward` are moved into the returned
    // HopKeys; the returned auth `tag` is public.
    ikm.zeroize();
    okm.zeroize();
    auth_key.zeroize();
    return (HopKeys { forward, backward }, tag);
}

/// Client side of the handshake.
///
/// Returns the CREATE blob to send, plus a finalizer that consumes the CREATED
/// reply and yields the hop keys (after verifying the relay's auth MAC).
pub struct ClientHandshake {
    eph_secret: StaticSecret,
    eph_public: [u8; 32],
    relay_static_pub: PublicKey,
}

impl ClientHandshake {
    pub fn new(relay_static_pub: [u8; 32]) -> Self {
        let eph_secret = StaticSecret::random_from_rng(OsRng);
        let eph_public = PublicKey::from(&eph_secret).to_bytes();
        return ClientHandshake {
            eph_secret,
            eph_public,
            relay_static_pub: PublicKey::from(relay_static_pub),
        };
    }

    /// Versioned CREATE blob (`TSR2` plus our 32-byte ephemeral public key).
    pub fn create_blob(&self) -> Vec<u8> {
        let mut create = Vec::with_capacity(36);
        create.extend_from_slice(HANDSHAKE_MARKER);
        create.extend_from_slice(&self.eph_public);
        return create;
    }

    /// Consume the versioned CREATED reply, verify it, and derive hop keys.
    pub fn finish(self, created: &[u8]) -> Result<HopKeys> {
        if created.len() != 68 {
            bail!("CREATED must be 68 bytes, got {}", created.len());
        }
        if &created[0..4] != HANDSHAKE_MARKER {
            bail!("CREATED used an unsupported handshake version");
        }
        let mut relay_eph_pub = [0u8; 32];
        relay_eph_pub.copy_from_slice(&created[4..36]);
        let mut got_mac = [0u8; 32];
        got_mac.copy_from_slice(&created[36..68]);

        let s1 = self.eph_secret.diffie_hellman(&self.relay_static_pub);
        if !s1.was_contributory() {
            bail!("relay static X25519 key produced a non-contributory shared secret");
        }
        let s2 = self
            .eph_secret
            .diffie_hellman(&PublicKey::from(relay_eph_pub));
        if !s2.was_contributory() {
            bail!("relay ephemeral X25519 key produced a non-contributory shared secret");
        }
        let relay_static_pub = self.relay_static_pub.to_bytes();
        let (keys, want_mac) = derive_keys(
            &s1.to_bytes(),
            &s2.to_bytes(),
            &relay_static_pub,
            &self.eph_public,
            &relay_eph_pub,
        );

        if !bool::from(got_mac.ct_eq(&want_mac)) {
            bail!("relay authentication failed: CREATED MAC mismatch");
        }
        return Ok(keys);
    }
}

/// Relay side of the handshake: given our static secret and the client's CREATE
/// blob, produce the CREATED reply and our hop keys.
pub fn relay_handshake(static_secret: &StaticSecret, create: &[u8]) -> Result<(Vec<u8>, HopKeys)> {
    if create.len() != 36 {
        bail!("CREATE must be 36 bytes, got {}", create.len());
    }
    if &create[0..4] != HANDSHAKE_MARKER {
        bail!("CREATE used an unsupported handshake version");
    }
    let mut client_pub = [0u8; 32];
    client_pub.copy_from_slice(&create[4..36]);
    let client_pub_point = PublicKey::from(client_pub);

    let relay_eph_secret = StaticSecret::random_from_rng(OsRng);
    let relay_eph_pub = PublicKey::from(&relay_eph_secret).to_bytes();

    let s1 = static_secret.diffie_hellman(&client_pub_point);
    let s2 = relay_eph_secret.diffie_hellman(&client_pub_point);
    if !s1.was_contributory() || !s2.was_contributory() {
        bail!("client X25519 key produced a non-contributory shared secret");
    }
    let relay_static_pub = PublicKey::from(static_secret).to_bytes();
    let (keys, mac) = derive_keys(
        &s1.to_bytes(),
        &s2.to_bytes(),
        &relay_static_pub,
        &client_pub,
        &relay_eph_pub,
    );

    let mut created = Vec::with_capacity(68);
    created.extend_from_slice(HANDSHAKE_MARKER);
    created.extend_from_slice(&relay_eph_pub);
    created.extend_from_slice(&mac);
    return Ok((created, keys));
}

/// AEAD sealer with a monotonic per-stream nonce counter. One `Sealer` owns one
/// key used in exactly one direction, so its nonce space is never reused.
pub struct Sealer {
    cipher: ChaCha20Poly1305,
    counter: u64,
}

impl Sealer {
    pub fn new(key: [u8; 32]) -> Self {
        return Sealer {
            cipher: ChaCha20Poly1305::new((&key).into()),
            counter: 0,
        };
    }

    pub fn seal(&mut self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let next = self
            .counter
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("AEAD nonce space exhausted"))?;
        let nonce = nonce_from_counter(self.counter);
        let ct = self
            .cipher
            .encrypt(&nonce, plaintext)
            .map_err(|_| anyhow::anyhow!("AEAD seal failed"))?;
        self.counter = next;
        return Ok(ct);
    }
}

/// AEAD opener, mirror of [`Sealer`].
pub struct Opener {
    cipher: ChaCha20Poly1305,
    counter: u64,
}

impl Opener {
    pub fn new(key: [u8; 32]) -> Self {
        return Opener {
            cipher: ChaCha20Poly1305::new((&key).into()),
            counter: 0,
        };
    }

    pub fn open(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>> {
        let next = self
            .counter
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("AEAD nonce space exhausted"))?;
        let nonce = nonce_from_counter(self.counter);
        let pt = self
            .cipher
            .decrypt(&nonce, ciphertext)
            .map_err(|_| anyhow::anyhow!("AEAD open failed (bad tag or desync)"))?;
        self.counter = next;
        return Ok(pt);
    }
}

fn nonce_from_counter(counter: u64) -> Nonce {
    let mut nonce = [0u8; 12];
    nonce[4..12].copy_from_slice(&counter.to_be_bytes());
    return Nonce::from(nonce);
}

/// Generate a fresh static keypair, returning (secret, public).
pub fn generate_static_keypair() -> (StaticSecret, [u8; 32]) {
    let secret = StaticSecret::random_from_rng(OsRng);
    let public = PublicKey::from(&secret).to_bytes();
    return (secret, public);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_agrees_on_keys_and_authenticates() {
        let (relay_secret, relay_pub) = generate_static_keypair();

        let client = ClientHandshake::new(relay_pub);
        let create = client.create_blob();
        let (created, relay_keys) = relay_handshake(&relay_secret, &create).unwrap();
        let client_keys = client.finish(&created).unwrap();

        assert_eq!(client_keys.forward, relay_keys.forward);
        assert_eq!(client_keys.backward, relay_keys.backward);
    }

    #[test]
    fn handshake_rejects_wrong_static_key() {
        let (_relay_secret, relay_pub) = generate_static_keypair();
        let (other_secret, _other_pub) = generate_static_keypair();

        // Client believes it is talking to `relay_pub`, but the responder holds
        // a different static secret: the auth MAC must not verify.
        let client = ClientHandshake::new(relay_pub);
        let create = client.create_blob();
        let (created, _keys) = relay_handshake(&other_secret, &create).unwrap();
        assert!(client.finish(&created).is_err());
    }

    #[test]
    fn relay_rejects_non_contributory_client_key() {
        let (relay_secret, _relay_pub) = generate_static_keypair();
        let mut create = HANDSHAKE_MARKER.to_vec();
        create.extend_from_slice(&[0u8; 32]);
        assert!(relay_handshake(&relay_secret, &create).is_err());
    }

    #[test]
    fn handshake_rejects_unknown_protocol_version() {
        let (relay_secret, relay_pub) = generate_static_keypair();
        let client = ClientHandshake::new(relay_pub);
        let mut create = client.create_blob();
        create[3] ^= 1;
        assert!(relay_handshake(&relay_secret, &create).is_err());
    }

    #[test]
    fn seal_open_roundtrips_in_order() {
        let (relay_secret, relay_pub) = generate_static_keypair();
        let client = ClientHandshake::new(relay_pub);
        let create = client.create_blob();
        let (created, relay_keys) = relay_handshake(&relay_secret, &create).unwrap();
        let client_keys = client.finish(&created).unwrap();

        let mut sealer = Sealer::new(client_keys.forward);
        let mut opener = Opener::new(relay_keys.forward);
        for i in 0..8u8 {
            let msg = vec![i; 100 + i as usize];
            let ct = sealer.seal(&msg).unwrap();
            assert_ne!(ct, msg);
            assert_eq!(opener.open(&ct).unwrap(), msg);
        }
    }

    #[test]
    fn nonce_counter_exhaustion_fails_closed() {
        let key = [42u8; 32];
        let mut sealer = Sealer::new(key);
        sealer.counter = u64::MAX;
        assert!(sealer.seal(b"never encrypted").is_err());

        let mut opener = Opener::new(key);
        opener.counter = u64::MAX;
        assert!(opener.open(b"never decrypted").is_err());
    }

    #[test]
    fn open_fails_on_nonce_desync() {
        let key = [7u8; 32];
        let mut sealer = Sealer::new(key);
        let mut opener = Opener::new(key);
        let _first = sealer.seal(b"one").unwrap(); // advances sealer counter
        let second = sealer.seal(b"two").unwrap();
        // Opener is still at counter 0; decrypting the second ciphertext must fail.
        assert!(opener.open(&second).is_err());
    }
}
