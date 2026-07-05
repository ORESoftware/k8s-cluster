//! tor-server: an onion-routing anonymizing proxy.
//!
//! One binary, three modes (argv[1] or the `TOR_ROLE` env var):
//!
//!   relay   Run an onion relay/exit node. Loads (or creates) a static X25519
//!           keypair and forwards cells. Prints its public key on startup so it
//!           can be added to a client directory.
//!             env: TOR_LISTEN (default 0.0.0.0:9001)
//!                  TOR_KEY_FILE (default ./relay.key)
//!
//!   client  Run the local SOCKS5 proxy. Builds a fresh multi-hop circuit per
//!           connection from the relays in the directory file.
//!             env: TOR_SOCKS_LISTEN (default 127.0.0.1:9050)
//!                  TOR_DIRECTORY (required; path to directory.toml)
//!                  TOR_HOPS (default 3)
//!
//!   keygen  Generate/persist a relay keypair and print its public key, then exit.
//!             env: TOR_KEY_FILE (default ./relay.key)

mod cell;
mod circuit;
mod config;
mod crypto;
mod policy;
mod relay;
mod socks;
mod stats;
mod web;
mod wire;

use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::info;
use tracing_subscriber::EnvFilter;

fn env_or(key: &str, default: &str) -> String {
    return std::env::var(key).unwrap_or_else(|_| default.to_string());
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_target(false)
        .init();

    // Optional overlay membership secret, folded into every handshake.
    if let Ok(secret) = std::env::var("TOR_NETWORK_SECRET") {
        if !secret.is_empty() {
            crypto::set_network_secret(secret.into_bytes());
            info!("overlay pre-shared key active (TOR_NETWORK_SECRET set)");
        }
    }

    let role = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("TOR_ROLE").ok())
        .unwrap_or_default();

    match role.as_str() {
        "relay" => run_relay().await,
        "client" => run_client().await,
        "keygen" => run_keygen(),
        "" => {
            bail!("no role given; usage: tor-server <relay|client|keygen> (or set TOR_ROLE)");
        }
        other => bail!("unknown role '{other}'; expected relay|client|keygen"),
    }
}

async fn run_relay() -> Result<()> {
    let listen = env_or("TOR_LISTEN", "0.0.0.0:9001");
    let key_file = PathBuf::from(env_or("TOR_KEY_FILE", "./relay.key"));
    let (secret, public) = config::load_or_create_static_secret(&key_file)
        .context("loading relay static key")?;
    info!(
        key_file = %key_file.display(),
        pubkey = %config::encode_pubkey(&public),
        "relay static identity"
    );
    return relay::run(&listen, secret).await;
}

async fn run_client() -> Result<()> {
    let socks_listen = env_or("TOR_SOCKS_LISTEN", "127.0.0.1:9050");
    let dir_path = std::env::var("TOR_DIRECTORY")
        .context("TOR_DIRECTORY is required in client mode (path to directory.toml)")?;
    let hops: usize = env_or("TOR_HOPS", "3")
        .parse()
        .context("TOR_HOPS must be a positive integer")?;
    let directory = config::Directory::load(&PathBuf::from(&dir_path))?;
    let cfg = socks::ClientConfig {
        socks_listen,
        directory,
        hops,
    };
    return socks::run(cfg).await;
}

fn run_keygen() -> Result<()> {
    let key_file = PathBuf::from(env_or("TOR_KEY_FILE", "./relay.key"));
    let (_secret, public) = config::load_or_create_static_secret(&key_file)
        .context("generating relay static key")?;
    println!("key_file: {}", key_file.display());
    println!("pubkey:   {}", config::encode_pubkey(&public));
    return Ok(());
}
