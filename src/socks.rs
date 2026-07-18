//! SOCKS5 front-end (RFC 1928, CONNECT only) with optional RFC 1929
//! username/password authentication.
//!
//! This is the local interface applications point at — the same role Tor's
//! SOCKS port plays. Each accepted connection opens a stream to the requested
//! destination through the active backend (the onion overlay or real Tor via
//! arti) and splices bytes both ways.

use anyhow::{bail, Result};
use std::net::Ipv4Addr;
use std::sync::Arc;
use subtle::ConstantTimeEq;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
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
    pub auth: Option<SocksAuth>,
    pub max_connections: usize,
}

#[derive(Clone)]
pub struct SocksAuth {
    username: Vec<u8>,
    password: Vec<u8>,
}

impl SocksAuth {
    pub fn new(username: String, password: String) -> Result<Self> {
        if username.is_empty() || username.len() > u8::MAX as usize {
            bail!("TOR_SOCKS_USERNAME must contain 1..=255 bytes");
        }
        if password.len() < 16 || password.len() > u8::MAX as usize {
            bail!("TOR_SOCKS_PASSWORD must contain 16..=255 bytes");
        }
        return Ok(Self {
            username: username.into_bytes(),
            password: password.into_bytes(),
        });
    }

    /// Constant-time check of a presented username+password against the
    /// configured credentials. Shared by the SOCKS (RFC 1929) and HTTP CONNECT
    /// (Proxy-Authorization: Basic) front-ends.
    pub fn verify(&self, username: &[u8], password: &[u8]) -> bool {
        return bool::from(username.ct_eq(&self.username) & password.ct_eq(&self.password));
    }
}

pub async fn run(cfg: Arc<ClientConfig>) -> Result<()> {
    let listener = TcpListener::bind(&cfg.socks_listen).await?;
    let limit = Arc::new(Semaphore::new(cfg.max_connections));
    info!(listen = %cfg.socks_listen, backend = cfg.connector.backend(), "SOCKS5 proxy listening");

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
                debug!("at SOCKS connection capacity, dropping {peer}");
                continue;
            }
        };
        let cfg = cfg.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(stream, cfg).await {
                debug!("socks connection from {peer} ended: {e:#}");
            }
            drop(permit);
        });
    }
}

async fn handle(mut client: TcpStream, cfg: Arc<ClientConfig>) -> Result<()> {
    client.set_nodelay(true).ok();
    let (host, port) = timeout(
        SOCKS_HANDSHAKE_TIMEOUT,
        socks_handshake(&mut client, cfg.auth.as_ref()),
    )
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
async fn socks_handshake<S>(client: &mut S, auth: Option<&SocksAuth>) -> Result<(String, u16)>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    // Greeting: VER, NMETHODS, METHODS...
    let ver = client.read_u8().await?;
    if ver != 0x05 {
        bail!("unsupported SOCKS version {ver}");
    }
    let nmethods = client.read_u8().await? as usize;
    let mut methods = vec![0u8; nmethods];
    client.read_exact(&mut methods).await?;
    let selected = if auth.is_some() { 0x02 } else { 0x00 };
    if !methods.contains(&selected) {
        client.write_all(&[0x05, 0xff]).await?;
        client.flush().await?;
        bail!("client did not offer required SOCKS authentication method");
    }
    client.write_all(&[0x05, selected]).await?;
    client.flush().await?;
    if let Some(auth) = auth {
        authenticate_rfc1929(client, auth).await?;
    }

    // Request: VER, CMD, RSV, ATYP, DST.ADDR, DST.PORT
    let ver = client.read_u8().await?;
    if ver != 0x05 {
        bail!("unsupported SOCKS version {ver} in request");
    }
    let cmd = client.read_u8().await?;
    let rsv = client.read_u8().await?;
    let atyp = client.read_u8().await?;

    if rsv != 0x00 {
        bail!("invalid SOCKS reserved byte {rsv}");
    }

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
            if len == 0 {
                bail!("SOCKS domain name must not be empty");
            }
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
    if port == 0 {
        bail!("SOCKS destination port must not be zero");
    }
    return Ok((host, port));
}

async fn authenticate_rfc1929<S>(client: &mut S, auth: &SocksAuth) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let version = client.read_u8().await?;
    if version != 0x01 {
        bail!("unsupported RFC 1929 authentication version {version}");
    }
    let username_len = client.read_u8().await? as usize;
    let mut username = vec![0u8; username_len];
    client.read_exact(&mut username).await?;
    let password_len = client.read_u8().await? as usize;
    let mut password = vec![0u8; password_len];
    client.read_exact(&mut password).await?;

    let accepted = ct_bytes_eq(&username, &auth.username) & ct_bytes_eq(&password, &auth.password);
    client
        .write_all(&[0x01, if accepted { 0x00 } else { 0x01 }])
        .await?;
    client.flush().await?;
    if !accepted {
        bail!("SOCKS authentication failed");
    }
    return Ok(());
}

fn ct_bytes_eq(a: &[u8], b: &[u8]) -> bool {
    return bool::from(a.ct_eq(b));
}

/// Send a SOCKS5 reply with the given status and a null bound address.
async fn send_reply<S>(client: &mut S, status: u8) -> Result<()>
where
    S: AsyncWrite + Unpin,
{
    // VER, REP, RSV, ATYP=IPv4, BND.ADDR=0.0.0.0, BND.PORT=0
    let reply = [0x05, status, 0x00, 0x01, 0, 0, 0, 0, 0, 0];
    client.write_all(&reply).await?;
    client.flush().await?;
    return Ok(());
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn rejects_when_required_method_was_not_offered() {
        let (mut server, mut client) = duplex(64);
        let server_task = tokio::spawn(async move { socks_handshake(&mut server, None).await });
        client.write_all(&[0x05, 0x01, 0x02]).await.unwrap();
        let mut reply = [0u8; 2];
        client.read_exact(&mut reply).await.unwrap();
        assert_eq!(reply, [0x05, 0xff]);
        assert!(server_task.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn rfc1929_rejects_bad_credentials() {
        let auth = SocksAuth::new("alice".into(), "correct horse!!!".into()).unwrap();
        let (mut server, mut client) = duplex(128);
        let server_task =
            tokio::spawn(async move { socks_handshake(&mut server, Some(&auth)).await });
        client.write_all(&[0x05, 0x01, 0x02]).await.unwrap();
        let mut method = [0u8; 2];
        client.read_exact(&mut method).await.unwrap();
        assert_eq!(method, [0x05, 0x02]);
        client
            .write_all(&[
                0x01, 0x05, b'a', b'l', b'i', b'c', b'e', 0x05, b'w', b'r', b'o', b'n', b'g',
            ])
            .await
            .unwrap();
        let mut result = [0u8; 2];
        client.read_exact(&mut result).await.unwrap();
        assert_eq!(result, [0x01, 0x01]);
        assert!(server_task.await.unwrap().is_err());
    }

    #[test]
    fn constant_time_compare_handles_different_lengths() {
        assert!(ct_bytes_eq(b"same", b"same"));
        assert!(!ct_bytes_eq(b"same", b"same!"));
        assert!(!ct_bytes_eq(b"same", b"sane"));
    }
}
