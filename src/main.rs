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
mod socks;
mod stats;
mod web;
mod wire;

use anyhow::{bail, Context, Result};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::info;

fn env_or(key: &str, default: &str) -> String {
    return std::env::var(key).unwrap_or_else(|_| default.to_string());
}

fn env_flag(key: &str) -> bool {
    return matches!(
        std::env::var(key).ok().as_deref(),
        Some("1") | Some("true") | Some("TRUE") | Some("yes")
    );
}

fn env_positive_usize(key: &str, default: usize) -> Result<usize> {
    let value = match std::env::var(key) {
        Ok(raw) => raw
            .parse::<usize>()
            .with_context(|| format!("{key} must be a positive integer"))?,
        Err(_) => default,
    };
    if value == 0 {
        bail!("{key} must be greater than zero");
    }
    return Ok(value);
}

/// Conservative check used for fail-closed listener defaults. Only numeric
/// loopback socket addresses are accepted; hostnames must opt in as remote so
/// DNS or hosts-file changes cannot turn a prior safety check into a public bind.
fn is_loopback_listener(listen: &str) -> bool {
    if let Ok(addr) = listen.parse::<SocketAddr>() {
        return addr.ip().is_loopback();
    }
    return false;
}

/// Read a secret from `env_key`, or from the file named by `file_key`, trimming
/// trailing whitespace/newline. Returns None if neither is set.
fn read_env_or_file(env_key: &str, file_key: &str) -> Result<Option<String>> {
    if let Ok(v) = std::env::var(env_key) {
        if !v.is_empty() {
            tracing::warn!(
                var = env_key,
                file_var = file_key,
                "secret read from the process environment; prefer the _FILE variant so it is not exposed via /proc/<pid>/environ, container inspect, or crash dumps"
            );
        }
        return Ok(Some(v));
    }
    if let Ok(path) = std::env::var(file_key) {
        let contents = std::fs::read_to_string(&path)
            .with_context(|| format!("reading secret file {path}"))?;
        return Ok(Some(contents.trim_end_matches(['\n', '\r']).to_string()));
    }
    return Ok(None);
}

#[tokio::main]
async fn main() -> Result<()> {
    let role = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("TOR_ROLE").ok())
        .unwrap_or_default();
    if let Some(scope) = role.strip_prefix("--export-openapi=") {
        print!("{}", web::export_openapi(scope)?);
        return Ok(());
    }
    let telemetry_service = match role.as_str() {
        "client" => "tor-client",
        "relay" => "tor-relay",
        "keygen" => "tor-keygen",
        _ => "tor-server",
    };
    // Retain the guard until every mode exits so buffered logs, traces, and
    // metrics are flushed during both graceful shutdown and startup failure.
    let _telemetry = fiducia_telemetry::init(telemetry_service);

    // Optional overlay membership secret, folded into every handshake. Prefer a
    // file (TOR_NETWORK_SECRET_FILE) so the secret is not exposed in the
    // process environment; fall back to TOR_NETWORK_SECRET.
    let mut network_secret_active = false;
    if let Some(secret) = read_env_or_file("TOR_NETWORK_SECRET", "TOR_NETWORK_SECRET_FILE")? {
        if !secret.is_empty() {
            if secret.len() < 32 {
                bail!("TOR_NETWORK_SECRET must contain at least 32 bytes of high-entropy data");
            }
            crypto::set_network_secret(secret.into_bytes());
            network_secret_active = true;
            info!("overlay pre-shared key active");
        }
    }

    match role.as_str() {
        "relay" => run_relay(network_secret_active).await,
        "client" => run_client().await,
        "keygen" => run_keygen(),
        "" => {
            bail!("no role given; usage: tor-server <relay|client|keygen> (or set TOR_ROLE)");
        }
        other => bail!("unknown role '{other}'; expected relay|client|keygen"),
    }
}

async fn run_relay(network_secret_active: bool) -> Result<()> {
    let listen = env_or("TOR_LISTEN", "0.0.0.0:9001");
    if !is_loopback_listener(&listen) && !network_secret_active && !env_flag("TOR_ALLOW_OPEN_RELAY")
    {
        bail!(
            "refusing non-loopback relay {listen} without TOR_NETWORK_SECRET; set a strong secret or explicitly acknowledge an open overlay with TOR_ALLOW_OPEN_RELAY=1"
        );
    }
    let key_file = PathBuf::from(env_or("TOR_KEY_FILE", "./relay.key"));
    let (secret, public) =
        config::load_or_create_static_secret(&key_file).context("loading relay static key")?;
    info!(
        key_file = %key_file.display(),
        pubkey = %config::encode_pubkey(&public),
        "relay static identity"
    );
    return relay::run(&listen, secret).await;
}

async fn run_client() -> Result<()> {
    let socks_listen = env_or("TOR_SOCKS_LISTEN", "127.0.0.1:9050");
    let ui_listen = env_or("TOR_UI_LISTEN", "127.0.0.1:9060");
    let backend = env_or("TOR_BACKEND", "overlay");
    let hops = env_positive_usize("TOR_HOPS", 3)?;
    let max_socks_connections = env_positive_usize("TOR_MAX_SOCKS_CONNECTIONS", 256)?;
    if max_socks_connections > 1_000_000 {
        bail!("TOR_MAX_SOCKS_CONNECTIONS must not exceed 1000000");
    }
    let docs_dir = PathBuf::from(env_or("TOR_DOCS_DIR", "./docs"));
    let ui_token = read_env_or_file("TOR_UI_TOKEN", "TOR_UI_TOKEN_FILE")?.filter(|t| !t.is_empty());
    let socks_password = read_env_or_file("TOR_SOCKS_PASSWORD", "TOR_SOCKS_PASSWORD_FILE")?
        .filter(|t| !t.is_empty());
    let socks_auth = match socks_password {
        Some(password) => Some(socks::SocksAuth::new(
            env_or("TOR_SOCKS_USERNAME", "tor"),
            password,
        )?),
        None => None,
    };
    let socks_is_remote = !is_loopback_listener(&socks_listen);
    if socks_is_remote && !env_flag("TOR_SOCKS_ALLOW_REMOTE") {
        bail!(
            "refusing non-loopback SOCKS listener {socks_listen}; set TOR_SOCKS_ALLOW_REMOTE=1 only behind a trusted network or encrypted tunnel"
        );
    }
    if socks_is_remote && socks_auth.is_none() {
        bail!(
            "non-loopback SOCKS listener {socks_listen} requires TOR_SOCKS_PASSWORD or TOR_SOCKS_PASSWORD_FILE"
        );
    }
    if socks_is_remote {
        tracing::warn!(
            listen = %socks_listen,
            "remote SOCKS enabled; RFC 1929 authenticates but does not encrypt the client-to-proxy link, so use a VPN, SSH tunnel, or private network"
        );
    }
    if ui_token.is_none()
        && !is_loopback_listener(&ui_listen)
        && !env_flag("TOR_UI_ALLOW_REMOTE_UNAUTHENTICATED")
    {
        bail!(
            "refusing non-loopback dashboard {ui_listen} without TOR_UI_TOKEN; set a token or explicitly acknowledge the open proxy with TOR_UI_ALLOW_REMOTE_UNAUTHENTICATED=1"
        );
    }
    if !is_loopback_listener(&ui_listen) && ui_token.as_ref().is_some_and(|token| token.len() < 32)
    {
        bail!("TOR_UI_TOKEN must contain at least 32 bytes when the dashboard is non-loopback");
    }

    // Optional HTTP CONNECT front-end (runs alongside SOCKS). Same fail-closed
    // posture: loopback by default; a non-loopback bind needs an explicit opt-in
    // and the shared proxy credential, and is never an open proxy.
    let http_listen = std::env::var("TOR_HTTP_LISTEN")
        .ok()
        .filter(|s| !s.is_empty());
    if let Some(listen) = &http_listen {
        let http_is_remote = !is_loopback_listener(listen);
        if http_is_remote && !env_flag("TOR_HTTP_ALLOW_REMOTE") {
            bail!(
                "refusing non-loopback HTTP proxy {listen}; set TOR_HTTP_ALLOW_REMOTE=1 only behind a trusted network or encrypted tunnel"
            );
        }
        if http_is_remote && socks_auth.is_none() {
            bail!(
                "non-loopback HTTP proxy {listen} requires proxy credentials (TOR_SOCKS_PASSWORD or TOR_SOCKS_PASSWORD_FILE)"
            );
        }
        if http_is_remote {
            tracing::warn!(
                listen = %listen,
                "remote HTTP CONNECT proxy enabled; Basic auth does not encrypt the client-to-proxy link, so use a VPN, SSH tunnel, or private network"
            );
        }
    }

    // Optional static forward tunnels: local listeners that carry every
    // connection through the overlay to a fixed upstream (ssh -L semantics).
    let forward_routes = match std::env::var("TOR_FORWARD").ok().filter(|s| !s.is_empty()) {
        Some(raw) => forward::parse_routes(&raw)?,
        None => Vec::new(),
    };
    for route in &forward_routes {
        let route_is_remote = !is_loopback_listener(&route.listen);
        if route_is_remote && !env_flag("TOR_FORWARD_ALLOW_REMOTE") {
            bail!(
                "refusing non-loopback forward tunnel {}; set TOR_FORWARD_ALLOW_REMOTE=1 only behind a trusted network (the tunnel is unauthenticated to its fixed target)",
                route.listen
            );
        }
        if route_is_remote {
            tracing::warn!(
                listen = %route.listen,
                target = %format!("{}:{}", route.target_host, route.target_port),
                "remote forward tunnel enabled; it is an unauthenticated path to its fixed upstream — restrict to a trusted network"
            );
        }
    }

    // The overlay backend needs a relay directory; arti does not.
    let directory = match std::env::var("TOR_DIRECTORY") {
        Ok(p) => Some(config::Directory::load(&PathBuf::from(&p))?),
        Err(_) => None,
    };
    let connector = Arc::new(connector::build(&backend, directory.clone(), hops).await?);
    let stats = Arc::new(stats::Stats::default());

    let client_cfg = Arc::new(socks::ClientConfig {
        socks_listen: socks_listen.clone(),
        connector: connector.clone(),
        stats: stats.clone(),
        auth: socks_auth.clone(),
        max_connections: max_socks_connections,
    });
    // Built before web_cfg consumes `connector`/`stats` so the HTTP front-end
    // shares the same backend, counters, and proxy credential.
    let http_cfg = http_listen.map(|listen| {
        Arc::new(http_connect::HttpProxyConfig {
            listen,
            connector: connector.clone(),
            stats: stats.clone(),
            auth: socks_auth,
            max_connections: max_socks_connections,
        })
    });
    let forward_cfgs: Vec<Arc<forward::ForwardConfig>> = forward_routes
        .into_iter()
        .map(|route| {
            Arc::new(forward::ForwardConfig {
                route,
                connector: connector.clone(),
                stats: stats.clone(),
                max_connections: max_socks_connections,
            })
        })
        .collect();
    let web_cfg = Arc::new(web::WebConfig {
        ui_listen: ui_listen.clone(),
        socks_listen,
        connector,
        directory,
        hops,
        docs_dir,
        stats,
        ui_token,
    });

    // Run the front-ends and dashboard concurrently; if any exits, the process
    // exits with its error.
    let mut tasks = tokio::task::JoinSet::new();
    tasks.spawn(async move { socks::run(client_cfg).await });
    tasks.spawn(async move { web::run(web_cfg).await });
    if let Some(http_cfg) = http_cfg {
        tasks.spawn(async move { http_connect::run(http_cfg).await });
    }
    for fwd_cfg in forward_cfgs {
        tasks.spawn(async move { forward::run(fwd_cfg).await });
    }
    if let Some(res) = tasks.join_next().await {
        return res.context("client task panicked")?;
    }
    return Ok(());
}

fn run_keygen() -> Result<()> {
    let key_file = PathBuf::from(env_or("TOR_KEY_FILE", "./relay.key"));
    let (_secret, public) =
        config::load_or_create_static_secret(&key_file).context("generating relay static key")?;
    println!("key_file: {}", key_file.display());
    println!("pubkey:   {}", config::encode_pubkey(&public));
    return Ok(());
}

#[cfg(test)]
mod tests {
    use super::is_loopback_listener;

    #[test]
    fn listener_safety_requires_numeric_loopback() {
        assert!(is_loopback_listener("127.0.0.1:9050"));
        assert!(is_loopback_listener("[::1]:9050"));
        assert!(!is_loopback_listener("0.0.0.0:9050"));
        assert!(!is_loopback_listener("localhost:9050"));
        assert!(!is_loopback_listener("proxy.example:9050"));
    }
}
