//! Static forward-tunnel (local port-forward through the overlay).
//!
//! Each `TOR_FORWARD` route binds a local TCP listener and tunnels every
//! accepted connection through the active backend to one fixed upstream
//! `host:port` — `ssh -L` semantics over the onion overlay. Unlike SOCKS or
//! CONNECT the connecting app does not choose the destination; the operator pins
//! it. This exposes a remote/public service locally (or to a trusted network)
//! with the overlay's anonymity in front of it, for apps that speak no proxy at
//! all.
//!
//! The pinned upstream still travels through the backend's exit policy, so a
//! private/internal target requires `TOR_EXIT_ALLOW_PRIVATE` at the exit just
//! like any other destination. The listener binds loopback by default; a
//! non-loopback bind requires `TOR_FORWARD_ALLOW_REMOTE=1` because it is an
//! unauthenticated tunnel to its fixed target.

use anyhow::{anyhow, bail, Result};
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio::time::{timeout, Duration};
use tracing::{debug, info, warn};

use crate::connector::Connector;
use crate::stats::Stats;

/// Bound on establishing the upstream circuit/connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForwardRoute {
    pub listen: String,
    pub target_host: String,
    pub target_port: u16,
}

pub struct ForwardConfig {
    pub route: ForwardRoute,
    pub connector: Arc<Connector>,
    pub stats: Arc<Stats>,
    pub max_connections: usize,
}

/// Parse `listen=host:port[,listen=host:port...]` into routes.
pub fn parse_routes(raw: &str) -> Result<Vec<ForwardRoute>> {
    let mut routes = Vec::new();
    for spec in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let (listen, target) = spec
            .split_once('=')
            .ok_or_else(|| anyhow!("TOR_FORWARD route '{spec}' must be listen=host:port"))?;
        let listen = listen.trim();
        let target = target.trim();
        // The listen side must be a concrete numeric bind address, so a hostname
        // cannot later resolve to a public interface and silently expose it.
        if listen.parse::<std::net::SocketAddr>().is_err() {
            bail!("TOR_FORWARD listen '{listen}' must be a numeric address:port");
        }
        let (host, port) = target
            .rsplit_once(':')
            .ok_or_else(|| anyhow!("TOR_FORWARD target '{target}' must be host:port"))?;
        let host = if host.starts_with('[') && host.ends_with(']') && host.len() >= 2 {
            &host[1..host.len() - 1]
        } else {
            host
        };
        if host.is_empty()
            || host.len() > 253
            || host.bytes().any(|b| b.is_ascii_control() || b == b' ')
        {
            bail!("TOR_FORWARD target host '{host}' is invalid");
        }
        let port: u16 = port
            .parse()
            .map_err(|_| anyhow!("TOR_FORWARD target port in '{target}' is invalid"))?;
        if port == 0 {
            bail!("TOR_FORWARD target port must not be zero");
        }
        routes.push(ForwardRoute {
            listen: listen.to_string(),
            target_host: host.to_string(),
            target_port: port,
        });
    }
    if routes.is_empty() {
        bail!("TOR_FORWARD was set but contained no routes");
    }
    return Ok(routes);
}

pub async fn run(cfg: Arc<ForwardConfig>) -> Result<()> {
    let listener = TcpListener::bind(&cfg.route.listen).await?;
    let limit = Arc::new(Semaphore::new(cfg.max_connections));
    info!(
        listen = %cfg.route.listen,
        target = %format!("{}:{}", cfg.route.target_host, cfg.route.target_port),
        backend = cfg.connector.backend(),
        "forward tunnel listening"
    );
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                warn!("accept failed: {e}");
                continue;
            }
        };
        let permit = match limit.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                debug!("at forward-tunnel capacity, dropping {peer}");
                continue;
            }
        };
        let cfg = cfg.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(stream, cfg).await {
                debug!("forward connection from {peer} ended: {e:#}");
            }
            drop(permit);
        });
    }
}

async fn handle(mut app: TcpStream, cfg: Arc<ForwardConfig>) -> Result<()> {
    app.set_nodelay(true).ok();
    let host = &cfg.route.target_host;
    let port = cfg.route.target_port;
    let connect = timeout(CONNECT_TIMEOUT, cfg.connector.connect(host, port)).await;
    let mut upstream = match connect {
        Ok(Ok(s)) => {
            cfg.stats.built();
            s
        }
        Ok(Err(e)) => {
            cfg.stats.failed();
            warn!("forward connect to {host}:{port} failed: {e:#}");
            return Ok(());
        }
        Err(_) => {
            cfg.stats.failed();
            warn!("forward connect to {host}:{port} timed out");
            return Ok(());
        }
    };
    cfg.stats.active_inc();
    let result = tokio::io::copy_bidirectional(&mut app, &mut upstream).await;
    cfg.stats.active_dec();
    result?;
    return Ok(());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_and_multiple_routes() {
        let r = parse_routes("127.0.0.1:8443=example.com:443").unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].listen, "127.0.0.1:8443");
        assert_eq!(r[0].target_host, "example.com");
        assert_eq!(r[0].target_port, 443);

        let r = parse_routes("127.0.0.1:8443=example.com:443, 127.0.0.1:9200=svc:9200").unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[1].target_host, "svc");
        assert_eq!(r[1].target_port, 9200);
    }

    #[test]
    fn strips_ipv6_target_brackets() {
        let r = parse_routes("127.0.0.1:8443=[2606:4700::1111]:443").unwrap();
        assert_eq!(r[0].target_host, "2606:4700::1111");
        assert_eq!(r[0].target_port, 443);
    }

    #[test]
    fn rejects_bad_specs() {
        assert!(parse_routes("").is_err()); // empty
        assert!(parse_routes("127.0.0.1:8443").is_err()); // no '='
        assert!(parse_routes("example.com:8443=host:443").is_err()); // non-numeric listen
        assert!(parse_routes("127.0.0.1:8443=host").is_err()); // target missing port
        assert!(parse_routes("127.0.0.1:8443=host:0").is_err()); // port 0
        assert!(parse_routes("127.0.0.1:8443=host:notaport").is_err());
    }
}
