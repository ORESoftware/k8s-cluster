//! HTTP `CONNECT` forward-proxy front-end.
//!
//! A second local proxy interface that runs alongside the SOCKS5 listener.
//! Applications and OS "HTTP proxy" settings that speak HTTP `CONNECT`
//! (browsers, `curl -x http://…`, Docker, most corporate-proxy fields) tunnel
//! TCP through the active backend — the same onion overlay or real-Tor path the
//! SOCKS front-end uses.
//!
//! Only the `CONNECT` method is supported. It covers HTTPS and any TCP
//! destination and carries TLS end-to-end (the proxy never sees plaintext). For
//! plaintext `http://` forwarding, point the app at the SOCKS interface instead;
//! absolute-URI forwarding is deliberately not implemented (smaller surface, no
//! request rewriting).
//!
//! Security posture mirrors the SOCKS front-end exactly: it binds loopback by
//! default, a non-loopback bind must be explicitly opted into and requires proxy
//! credentials (checked in constant time), and it never acts as an open proxy —
//! the exit policy of the backend still applies to every destination.

use anyhow::{anyhow, bail, Result};
use base64::Engine;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio::time::{timeout, Duration};
use tracing::{debug, info, warn};

use crate::connector::Connector;
use crate::socks::SocksAuth;
use crate::stats::Stats;

/// A client must send a complete request head within this window (anti-slowloris).
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
/// Bound on establishing the upstream circuit/connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(60);
/// Cap on the request head we buffer before the blank line, so a peer cannot
/// make us allocate unbounded memory by never terminating the headers.
const MAX_HEAD_BYTES: usize = 16 * 1024;

pub struct HttpProxyConfig {
    pub listen: String,
    pub connector: Arc<Connector>,
    pub stats: Arc<Stats>,
    /// Shared with the SOCKS front-end: the same proxy credential guards both.
    pub auth: Option<SocksAuth>,
    pub max_connections: usize,
}

pub async fn run(cfg: Arc<HttpProxyConfig>) -> Result<()> {
    let listener = TcpListener::bind(&cfg.listen).await?;
    let limit = Arc::new(Semaphore::new(cfg.max_connections));
    info!(listen = %cfg.listen, backend = cfg.connector.backend(), "HTTP CONNECT proxy listening");

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
                debug!("at HTTP proxy capacity, dropping {peer}");
                continue;
            }
        };
        let cfg = cfg.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(stream, cfg).await {
                debug!("http proxy connection from {peer} ended: {e:#}");
            }
            drop(permit);
        });
    }
}

async fn handle(mut client: TcpStream, cfg: Arc<HttpProxyConfig>) -> Result<()> {
    client.set_nodelay(true).ok();

    let (head_bytes, early) = timeout(HANDSHAKE_TIMEOUT, read_request_head(&mut client))
        .await
        .map_err(|_| anyhow!("HTTP proxy request timed out"))??;
    let head = parse_head(&head_bytes)?;

    let (host, port) = match connect_target(&head) {
        Ok(target) => target,
        Err(e) => {
            let _ = write_status(&mut client, "400 Bad Request").await;
            return Err(e);
        }
    };

    // Authenticate before dialing so an unauthorized peer cannot drive circuits.
    if let Some(auth) = &cfg.auth {
        if !proxy_auth_ok(&head, auth) {
            let _ = client
                .write_all(
                    b"HTTP/1.1 407 Proxy Authentication Required\r\n\
                      Proxy-Authenticate: Basic realm=\"tor-server\"\r\n\
                      Content-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await;
            let _ = client.flush().await;
            bail!("HTTP proxy authentication failed or missing");
        }
    }

    debug!(target = %format!("{host}:{port}"), backend = cfg.connector.backend(), "http proxy connecting");
    let connect = timeout(CONNECT_TIMEOUT, cfg.connector.connect(&host, port)).await;
    let mut upstream = match connect {
        Ok(Ok(s)) => {
            cfg.stats.built();
            s
        }
        Ok(Err(e)) => {
            cfg.stats.failed();
            warn!("http proxy connect to {host}:{port} failed: {e:#}");
            let _ = write_status(&mut client, "502 Bad Gateway").await;
            return Ok(());
        }
        Err(_) => {
            cfg.stats.failed();
            warn!("http proxy connect to {host}:{port} timed out");
            let _ = write_status(&mut client, "504 Gateway Timeout").await;
            return Ok(());
        }
    };

    client
        .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
        .await?;
    client.flush().await?;

    // A well-behaved client waits for the 200 before sending, but if it
    // pipelined bytes after the blank line (e.g. an early TLS ClientHello),
    // forward them before splicing so nothing is dropped.
    if !early.is_empty() {
        upstream.write_all(&early).await?;
        upstream.flush().await?;
    }

    cfg.stats.active_inc();
    let result = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
    cfg.stats.active_dec();
    result?;
    return Ok(());
}

/// Read the request head (up to and including the blank `\r\n\r\n` line),
/// returning the head bytes and any bytes already read past it.
async fn read_request_head(client: &mut TcpStream) -> Result<(Vec<u8>, Vec<u8>)> {
    let mut buf = Vec::with_capacity(1024);
    let mut tmp = [0u8; 1024];
    loop {
        let n = client.read(&mut tmp).await?;
        if n == 0 {
            bail!("client closed before completing the HTTP request");
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = find_subsequence(&buf, b"\r\n\r\n") {
            let early = buf[pos + 4..].to_vec();
            buf.truncate(pos + 4);
            return Ok((buf, early));
        }
        if buf.len() > MAX_HEAD_BYTES {
            bail!("HTTP proxy request head exceeds {MAX_HEAD_BYTES} bytes");
        }
    }
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    return haystack.windows(needle.len()).position(|w| w == needle);
}

struct RequestHead {
    method: String,
    target: String,
    /// Header names lowercased; values trimmed.
    headers: Vec<(String, String)>,
}

fn parse_head(bytes: &[u8]) -> Result<RequestHead> {
    let text =
        std::str::from_utf8(bytes).map_err(|_| anyhow!("request head is not valid UTF-8"))?;
    let mut lines = text.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("").to_string();
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    return Ok(RequestHead {
        method,
        target,
        headers,
    });
}

/// Extract and validate the `CONNECT host:port` target.
fn connect_target(head: &RequestHead) -> Result<(String, u16)> {
    if !head.method.eq_ignore_ascii_case("CONNECT") {
        bail!(
            "only the CONNECT method is supported (got '{}'); use the SOCKS proxy for plaintext HTTP",
            head.method
        );
    }
    let (host, port) = head
        .target
        .rsplit_once(':')
        .ok_or_else(|| anyhow!("CONNECT target must be host:port"))?;
    // Strip brackets around an IPv6 literal so the backend resolver sees a bare
    // address (e.g. `[2606:4700::1111]:443` -> `2606:4700::1111`).
    let host = if host.starts_with('[') && host.ends_with(']') && host.len() >= 2 {
        &host[1..host.len() - 1]
    } else {
        host
    };
    if host.is_empty()
        || host.len() > 253
        || host.bytes().any(|b| b.is_ascii_control() || b == b' ')
    {
        bail!("invalid CONNECT host");
    }
    let port: u16 = port.parse().map_err(|_| anyhow!("invalid CONNECT port"))?;
    if port == 0 {
        bail!("CONNECT port must not be zero");
    }
    return Ok((host.to_string(), port));
}

/// Verify `Proxy-Authorization: Basic base64(user:pass)` against the configured
/// credential (constant-time comparison of the decoded fields).
fn proxy_auth_ok(head: &RequestHead, auth: &SocksAuth) -> bool {
    let value = head
        .headers
        .iter()
        .find(|(k, _)| k == "proxy-authorization")
        .map(|(_, v)| v.as_str());
    let b64 = match value.and_then(|v| {
        v.strip_prefix("Basic ")
            .or_else(|| v.strip_prefix("basic "))
    }) {
        Some(b) => b.trim(),
        None => return false,
    };
    let decoded = match base64::engine::general_purpose::STANDARD.decode(b64) {
        Ok(d) => d,
        Err(_) => return false,
    };
    return match decoded.iter().position(|&b| b == b':') {
        Some(i) => auth.verify(&decoded[..i], &decoded[i + 1..]),
        None => false,
    };
}

async fn write_status(client: &mut TcpStream, status: &str) -> Result<()> {
    let msg = format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    client.write_all(msg.as_bytes()).await?;
    client.flush().await?;
    return Ok(());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(raw: &str) -> RequestHead {
        parse_head(raw.as_bytes()).unwrap()
    }

    #[test]
    fn parses_connect_target() {
        let h = head("CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n");
        assert_eq!(
            connect_target(&h).unwrap(),
            ("example.com".to_string(), 443)
        );
    }

    #[test]
    fn strips_ipv6_brackets() {
        let h = head("CONNECT [2606:4700::1111]:443 HTTP/1.1\r\n\r\n");
        assert_eq!(
            connect_target(&h).unwrap(),
            ("2606:4700::1111".to_string(), 443)
        );
    }

    #[test]
    fn rejects_non_connect_and_bad_targets() {
        assert!(connect_target(&head("GET http://example.com/ HTTP/1.1\r\n\r\n")).is_err());
        assert!(connect_target(&head("CONNECT example.com HTTP/1.1\r\n\r\n")).is_err()); // no port
        assert!(connect_target(&head("CONNECT example.com:0 HTTP/1.1\r\n\r\n")).is_err()); // port 0
        assert!(connect_target(&head("CONNECT example.com:notaport HTTP/1.1\r\n\r\n")).is_err());
    }

    #[test]
    fn proxy_auth_matches_and_rejects() {
        let auth = SocksAuth::new("alice".into(), "correct horse!!!".into()).unwrap();
        // base64("alice:correct horse!!!")
        let good = base64::engine::general_purpose::STANDARD.encode("alice:correct horse!!!");
        let h = head(&format!(
            "CONNECT example.com:443 HTTP/1.1\r\nProxy-Authorization: Basic {good}\r\n\r\n"
        ));
        assert!(proxy_auth_ok(&h, &auth));

        let bad = base64::engine::general_purpose::STANDARD.encode("alice:wrong");
        let h = head(&format!(
            "CONNECT example.com:443 HTTP/1.1\r\nProxy-Authorization: Basic {bad}\r\n\r\n"
        ));
        assert!(!proxy_auth_ok(&h, &auth));

        // Missing header.
        let h = head("CONNECT example.com:443 HTTP/1.1\r\n\r\n");
        assert!(!proxy_auth_ok(&h, &auth));
    }

    #[test]
    fn finds_head_terminator() {
        assert_eq!(find_subsequence(b"abc\r\n\r\nxy", b"\r\n\r\n"), Some(3));
        assert_eq!(find_subsequence(b"no terminator", b"\r\n\r\n"), None);
    }
}
