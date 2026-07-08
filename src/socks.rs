//! SOCKS5 front-end (RFC 1928, CONNECT only, no authentication).
//!
//! This is the local interface applications point at — the same role Tor's
//! SOCKS port plays. Each accepted connection opens a stream to the requested
//! destination through the active backend (the onion overlay or real Tor via
//! arti) and splices bytes both ways.

use anyhow::{bail, Result};
use std::net::Ipv4Addr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{timeout, Duration};
use tracing::{debug, info, warn};

use crate::connector::Connector;
use crate::stats::Stats;

/// A client must finish the SOCKS negotiation within this window (anti-slowloris).
const SOCKS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
/// Bound on establishing the upstream circuit/connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(60);

pub struct ClientConfig {
    pub socks_listen: String,
    pub connector: Arc<Connector>,
    pub stats: Arc<Stats>,
}

pub async fn run(cfg: Arc<ClientConfig>) -> Result<()> {
    let listener = TcpListener::bind(&cfg.socks_listen).await?;
    info!(listen = %cfg.socks_listen, backend = cfg.connector.backend(), "SOCKS5 proxy listening");

    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                warn!("accept failed: {e}");
                continue;
            }
        };
        let cfg = cfg.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(stream, cfg).await {
                debug!("socks connection from {peer} ended: {e:#}");
            }
        });
    }
}

async fn handle(mut client: TcpStream, cfg: Arc<ClientConfig>) -> Result<()> {
    client.set_nodelay(true).ok();
    let (host, port) = timeout(SOCKS_HANDSHAKE_TIMEOUT, socks_handshake(&mut client))
        .await
        .map_err(|_| anyhow::anyhow!("SOCKS handshake timed out"))??;
    debug!(target = %format!("{host}:{port}"), backend = cfg.connector.backend(), "connecting");

    let connect = timeout(CONNECT_TIMEOUT, cfg.connector.connect(&host, port)).await;
    let mut upstream = match connect {
        Ok(Ok(s)) => {
            cfg.stats.built();
            s
        }
        Ok(Err(e)) => {
            cfg.stats.failed();
            warn!("connect to {host}:{port} failed: {e:#}");
            send_reply(&mut client, 0x01).await?; // general failure
            return Ok(());
        }
        Err(_) => {
            cfg.stats.failed();
            warn!("connect to {host}:{port} timed out");
            send_reply(&mut client, 0x01).await?;
            return Ok(());
        }
    };

    send_reply(&mut client, 0x00).await?; // succeeded
    cfg.stats.active_inc();
    let result = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
    cfg.stats.active_dec();
    result?;
    return Ok(());
}

/// Perform the SOCKS5 greeting + CONNECT request, returning the target
/// host and port. Leaves the stream positioned to send the reply.
async fn socks_handshake(client: &mut TcpStream) -> Result<(String, u16)> {
    // Greeting: VER, NMETHODS, METHODS...
    let ver = client.read_u8().await?;
    if ver != 0x05 {
        bail!("unsupported SOCKS version {ver}");
    }
    let nmethods = client.read_u8().await? as usize;
    let mut methods = vec![0u8; nmethods];
    client.read_exact(&mut methods).await?;
    // We only support "no authentication required" (0x00).
    client.write_all(&[0x05, 0x00]).await?;
    client.flush().await?;

    // Request: VER, CMD, RSV, ATYP, DST.ADDR, DST.PORT
    let ver = client.read_u8().await?;
    if ver != 0x05 {
        bail!("unsupported SOCKS version {ver} in request");
    }
    let cmd = client.read_u8().await?;
    let _rsv = client.read_u8().await?;
    let atyp = client.read_u8().await?;

    if cmd != 0x01 {
        // Only CONNECT is supported.
        send_reply(client, 0x07).await?; // command not supported
        bail!("unsupported SOCKS command {cmd}");
    }

    let host = match atyp {
        0x01 => {
            let mut octets = [0u8; 4];
            client.read_exact(&mut octets).await?;
            Ipv4Addr::from(octets).to_string()
        }
        0x03 => {
            let len = client.read_u8().await? as usize;
            let mut name = vec![0u8; len];
            client.read_exact(&mut name).await?;
            String::from_utf8(name).map_err(|_| anyhow::anyhow!("invalid domain name"))?
        }
        0x04 => {
            let mut octets = [0u8; 16];
            client.read_exact(&mut octets).await?;
            std::net::Ipv6Addr::from(octets).to_string()
        }
        other => {
            send_reply(client, 0x08).await?; // address type not supported
            bail!("unsupported SOCKS address type {other}");
        }
    };
    let port = client.read_u16().await?;
    return Ok((host, port));
}

/// Send a SOCKS5 reply with the given status and a null bound address.
async fn send_reply(client: &mut TcpStream, status: u8) -> Result<()> {
    // VER, REP, RSV, ATYP=IPv4, BND.ADDR=0.0.0.0, BND.PORT=0
    let reply = [0x05, status, 0x00, 0x01, 0, 0, 0, 0, 0, 0];
    client.write_all(&reply).await?;
    client.flush().await?;
    return Ok(());
}
