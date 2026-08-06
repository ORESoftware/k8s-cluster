//! Configuration: the relay directory and on-disk key persistence.

use anyhow::{bail, Context, Result};
use base64::Engine;
use serde::Deserialize;
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::{Zeroize, Zeroizing};

/// One relay as seen by a client: where to reach it and its static public key.
#[derive(Debug, Clone, Deserialize)]
pub struct RelayInfo {
    pub name: String,
    pub addr: String,
    /// Base64 (standard, padded) of the 32-byte X25519 static public key.
    pub pubkey: String,
}

impl RelayInfo {
    pub fn pubkey_bytes(&self) -> Result<[u8; 32]> {
        return decode_pubkey(&self.pubkey).with_context(|| format!("relay '{}'", self.name));
    }
}

/// The directory file: the set of relays a client may route through.
#[derive(Debug, Clone, Deserialize)]
pub struct Directory {
    pub relays: Vec<RelayInfo>,
}

impl Directory {
    pub fn load(path: &Path) -> Result<Directory> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading directory file {}", path.display()))?;
        let dir: Directory = toml::from_str(&text)
            .with_context(|| format!("parsing directory file {}", path.display()))?;
        if dir.relays.is_empty() {
            bail!("directory {} lists no relays", path.display());
        }
        let mut names = HashSet::new();
        let mut addrs = HashSet::new();
        let mut pubkeys = HashSet::new();
        for relay in &dir.relays {
            if relay.name.trim().is_empty() || relay.addr.trim().is_empty() {
                bail!("directory relay names and addresses must not be empty");
            }
            if relay.addr.len() > 512
                || relay
                    .addr
                    .chars()
                    .any(|c| c.is_ascii_control() || c.is_ascii_whitespace())
            {
                bail!("relay '{}' has an invalid address", relay.name);
            }
            if !names.insert(relay.name.clone()) {
                bail!("directory contains duplicate relay name '{}'", relay.name);
            }
            if !addrs.insert(relay.addr.clone()) {
                bail!(
                    "directory contains duplicate relay address '{}'",
                    relay.addr
                );
            }
            let pubkey = relay.pubkey_bytes()?;
            let probe = StaticSecret::random_from_rng(rand::rngs::OsRng)
                .diffie_hellman(&PublicKey::from(pubkey));
            if !probe.was_contributory() {
                bail!(
                    "relay '{}' has a non-contributory X25519 public key",
                    relay.name
                );
            }
            if !pubkeys.insert(pubkey) {
                bail!("directory contains a duplicate relay public key");
            }
        }
        return Ok(dir);
    }

    /// Pick `hops` distinct relays at random; the last is the exit.
    pub fn choose_path(&self, hops: usize) -> Result<Vec<RelayInfo>> {
        use rand::seq::SliceRandom;
        if hops == 0 {
            bail!("hop count must be at least 1");
        }
        if self.relays.len() < hops {
            bail!(
                "directory has {} relays but {hops} hops were requested",
                self.relays.len()
            );
        }
        let mut relays = self.relays.clone();
        relays.shuffle(&mut rand::thread_rng());
        relays.truncate(hops);
        return Ok(relays);
    }
}

pub fn encode_pubkey(pubkey: &[u8; 32]) -> String {
    return base64::engine::general_purpose::STANDARD.encode(pubkey);
}

fn decode_pubkey(s: &str) -> Result<[u8; 32]> {
    let raw = base64::engine::general_purpose::STANDARD
        .decode(s.trim())
        .context("base64-decoding public key")?;
    if raw.len() != 32 {
        bail!("public key must be 32 bytes, got {}", raw.len());
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&raw);
    return Ok(out);
}

/// Load a relay's static secret from `path`, or generate + persist one if the
/// file does not exist. The file stores the 32-byte secret as base64.
pub fn load_or_create_static_secret(path: &Path) -> Result<(StaticSecret, [u8; 32])> {
    if path.exists() {
        // The write path is symlink-safe (O_EXCL); harden the read path too so an
        // attacker with write access to the key directory cannot pre-plant a
        // symlink that redirects the relay's "identity secret" to a file they
        // control.
        #[cfg(unix)]
        {
            let meta = std::fs::symlink_metadata(path)
                .with_context(|| format!("stat key file {}", path.display()))?;
            if meta.file_type().is_symlink() {
                bail!(
                    "key file {} is a symlink; refusing to follow it",
                    path.display()
                );
            }
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading key file {}", path.display()))?;
        let mut raw = Zeroizing::new(
            base64::engine::general_purpose::STANDARD
                .decode(text.trim())
                .with_context(|| format!("base64-decoding key file {}", path.display()))?,
        );
        if raw.len() != 32 {
            bail!(
                "key file {} must hold 32 bytes, got {}",
                path.display(),
                raw.len()
            );
        }
        let mut secret_bytes = Zeroizing::new([0u8; 32]);
        secret_bytes.copy_from_slice(&raw);
        raw.zeroize();
        let secret = StaticSecret::from(*secret_bytes);
        let public = PublicKey::from(&secret).to_bytes();
        return Ok((secret, public));
    }

    let (secret, public) = crate::crypto::generate_static_keypair();
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating key dir {}", parent.display()))?;
        }
    }
    let encoded =
        Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(secret.to_bytes()));
    // Create atomically and refuse to follow/overwrite an existing path. On
    // Unix the owner-only mode is applied at creation, eliminating the window
    // where a newly written identity key inherited a permissive umask.
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("creating key file {}", path.display()))?;
    file.write_all(encoded.as_bytes())
        .with_context(|| format!("writing key file {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("syncing key file {}", path.display()))?;
    return Ok((secret, public));
}
