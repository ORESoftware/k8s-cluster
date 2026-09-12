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
mod connector;
mod crypto;
mod forward;
mod http_connect;
mod policy;
mod relay;
mod server;
mod socks;
mod stats;
mod web;
mod wire;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    server::run().await
}
