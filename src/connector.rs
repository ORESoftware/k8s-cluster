//! Pluggable transport backend behind the SOCKS proxy and web dashboard.
//!
//! Two backends expose the same "connect to host:port, get a byte stream"
//! interface:
//!
//! - **overlay** — this project's own onion overlay (build a telescoped circuit
//!   through the relays in the directory and tunnel through it).
//! - **arti** — the real Tor network via the official `arti-client` crate
//!   (compiled in only with `--features arti`).
//!
//! Selected at startup with `TOR_BACKEND` (default `overlay`).

use anyhow::{bail, Result};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::circuit::Circuit;
use crate::config::Directory;

/// Anything that can carry a bidirectional stream. Boxed so both backends can
/// return a uniform type.
pub trait Duplex: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Duplex for T {}

pub enum Connector {
    Overlay {
        directory: Directory,
        hops: usize,
    },
    #[cfg(feature = "arti")]
    Arti {
        client: std::sync::Arc<arti_client::TorClient<tor_rtcompat::PreferredRuntime>>,
        isolate_every_stream: bool,
    },
}

impl Connector {
    pub fn backend(&self) -> &'static str {
        return match self {
            Connector::Overlay { .. } => "overlay",
            #[cfg(feature = "arti")]
            Connector::Arti { .. } => "arti",
        };
    }

    /// Open a stream to `host:port` through the active backend.
    pub async fn connect(&self, host: &str, port: u16) -> Result<Box<dyn Duplex>> {
        return match self {
            Connector::Overlay { directory, hops } => {
                let path = directory.choose_path(*hops)?;
                let mut circuit = Circuit::build(&path).await?;
                circuit.begin(host, port).await?;
                // Bridge the onion circuit to a byte stream the caller can use.
                let (near, far) = tokio::io::duplex(64 * 1024);
                tokio::spawn(async move {
                    let _ = circuit.splice(far).await;
                });
                Ok(Box::new(near))
            }
            #[cfg(feature = "arti")]
            Connector::Arti {
                client,
                isolate_every_stream,
            } => {
                let stream = if *isolate_every_stream {
                    let mut prefs = arti_client::StreamPrefs::new();
                    prefs.isolate_every_stream();
                    client.connect_with_prefs((host, port), &prefs).await
                } else {
                    client.connect((host, port)).await
                }
                .map_err(|e| anyhow::anyhow!("tor connect to {host}:{port} failed: {e}"))?;
                Ok(Box::new(stream))
            }
        };
    }
}

/// Construct the connector chosen by `backend`.
pub async fn build(backend: &str, directory: Option<Directory>, hops: usize) -> Result<Connector> {
    return match backend {
        "overlay" => {
            let directory = directory
                .ok_or_else(|| anyhow::anyhow!("overlay backend requires TOR_DIRECTORY"))?;
            Ok(Connector::Overlay { directory, hops })
        }
        "arti" => build_arti().await,
        other => bail!("unknown TOR_BACKEND '{other}' (expected overlay|arti)"),
    };
}

#[cfg(feature = "arti")]
async fn build_arti() -> Result<Connector> {
    use arti_client::{TorClient, TorClientConfig};
    // rustls 0.23 requires a process-wide CryptoProvider to be chosen explicitly.
    let _ = rustls::crypto::ring::default_provider().install_default();
    tracing::info!("bootstrapping Tor via arti (fetching directory consensus, may take a while)…");
    let config = match std::env::var("TOR_ARTI_CONFIG") {
        Ok(path) => {
            let raw = std::fs::read_to_string(&path)
                .map_err(|e| anyhow::anyhow!("reading Arti client config {path}: {e}"))?;
            let config = parse_arti_client_config(&raw, &path)?;
            tracing::info!(config = %path, "using operator-supplied Arti client configuration");
            config
        }
        Err(_) => TorClientConfig::default(),
    };
    let client = TorClient::create_bootstrapped(config)
        .await
        .map_err(|e| anyhow::anyhow!("arti bootstrap failed: {e}"))?;
    tracing::info!("arti bootstrapped — connected to the real Tor network");
    let isolate_every_stream = matches!(
        std::env::var("TOR_ARTI_ISOLATE_STREAMS").ok().as_deref(),
        Some("1") | Some("true") | Some("TRUE") | Some("yes")
    );
    if isolate_every_stream {
        tracing::warn!("Arti per-stream circuit isolation enabled; use only at low volume because it increases Tor network load");
    }
    return Ok(Connector::Arti {
        client,
        isolate_every_stream,
    });
}

#[cfg(feature = "arti")]
fn parse_arti_client_config(raw: &str, source: &str) -> Result<arti_client::TorClientConfig> {
    use arti_client::config::TorClientConfigBuilder;
    let value: toml::Value = toml::from_str(raw)
        .map_err(|e| anyhow::anyhow!("parsing Arti client config {source}: {e}"))?;
    let builder: TorClientConfigBuilder = value
        .try_into()
        .map_err(|e| anyhow::anyhow!("decoding Arti client config {source}: {e}"))?;
    return builder
        .build()
        .map_err(|e| anyhow::anyhow!("building Arti client config {source}: {e}"));
}

#[cfg(all(test, feature = "arti"))]
mod tests {
    use super::*;

    #[test]
    fn example_arti_transport_config_parses() {
        parse_arti_client_config(
            include_str!("../arti-client.example.toml"),
            "arti-client.example.toml",
        )
        .unwrap();
    }
}

#[cfg(not(feature = "arti"))]
async fn build_arti() -> Result<Connector> {
    bail!("this binary was built without Tor support; rebuild with `--features arti` to use TOR_BACKEND=arti");
}
