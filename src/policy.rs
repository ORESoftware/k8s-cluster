//! Exit and extend policy.
//!
//! An exit relay makes TCP connections on a client's behalf, so without a
//! policy it is an open proxy into whatever the exit can reach — including
//! loopback, RFC1918/ULA private ranges, link-local, and the cloud metadata
//! endpoint (169.254.169.254). By default we resolve the destination and refuse
//! any address in those ranges, which is the single most important hardening
//! against SSRF. Set `TOR_EXIT_ALLOW_PRIVATE=1` to permit them (e.g. for a
//! fully local test overlay where the origin is on 127.0.0.1).
//!
//! `Extend` targets (relay-to-relay hops) are usually private cluster IPs, so
//! they are not subject to the exit range filter. They may instead be pinned to
//! an explicit allowlist via `TOR_RELAY_PEERS` (comma-separated host:port).

use anyhow::{bail, Result};
use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use tokio::net::lookup_host;

#[derive(Clone)]
pub struct Policy {
    allow_private_exit: bool,
    /// If `Some`, `Extend` targets must appear in this set (exact string match
    /// on the `host:port` the client requested).
    relay_peers: Option<HashSet<String>>,
}

impl Policy {
    pub fn from_env() -> Policy {
        let allow_private_exit = env_flag("TOR_EXIT_ALLOW_PRIVATE");
        let relay_peers = std::env::var("TOR_RELAY_PEERS").ok().and_then(|raw| {
            let set: HashSet<String> = raw
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if set.is_empty() {
                None
            } else {
                Some(set)
            }
        });
        return Policy {
            allow_private_exit,
            relay_peers,
        };
    }

    pub fn allow_private_exit(&self) -> bool {
        return self.allow_private_exit;
    }

    /// Resolve `host:port` and return the first address the exit policy permits.
    pub async fn resolve_exit(&self, host: &str, port: u16) -> Result<SocketAddr> {
        let addrs: Vec<SocketAddr> = lookup_host((host, port))
            .await
            .map_err(|e| anyhow::anyhow!("DNS resolution for {host}:{port} failed: {e}"))?
            .collect();
        if addrs.is_empty() {
            bail!("no addresses resolved for {host}:{port}");
        }
        if self.allow_private_exit {
            return Ok(addrs[0]);
        }
        for addr in &addrs {
            if !is_blocked(addr.ip()) {
                return Ok(*addr);
            }
        }
        bail!("exit policy blocked all resolved addresses for {host}:{port} (private/loopback/link-local)");
    }

    /// Validate an `Extend` target against the optional relay-peer allowlist.
    pub fn check_extend(&self, addr: &str) -> Result<()> {
        if let Some(peers) = &self.relay_peers {
            if !peers.contains(addr) {
                bail!("extend target {addr} not in TOR_RELAY_PEERS allowlist");
            }
        }
        return Ok(());
    }
}

fn env_flag(key: &str) -> bool {
    return matches!(
        std::env::var(key).ok().as_deref(),
        Some("1") | Some("true") | Some("TRUE") | Some("yes")
    );
}

/// True if an IP is in a range an exit must not reach by default.
fn is_blocked(ip: IpAddr) -> bool {
    return match ip {
        IpAddr::V4(v4) => is_blocked_v4(v4),
        IpAddr::V6(v6) => is_blocked_v6(v6),
    };
}

fn is_blocked_v4(v4: Ipv4Addr) -> bool {
    let o = v4.octets();
    // 100.64.0.0/10 carrier-grade NAT (not covered by is_private).
    let is_cgnat = o[0] == 100 && (o[1] & 0xc0) == 0x40;
    return v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local()
        || v4.is_broadcast()
        || v4.is_documentation()
        || v4.is_unspecified()
        || v4.is_multicast()
        || is_cgnat;
}

fn is_blocked_v6(v6: Ipv6Addr) -> bool {
    let seg0 = v6.segments()[0];
    let is_ula = (seg0 & 0xfe00) == 0xfc00; // fc00::/7 unique local
    let is_link_local = (seg0 & 0xffc0) == 0xfe80; // fe80::/10
    return v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() || is_ula || is_link_local;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_loopback_private_and_metadata() {
        assert!(is_blocked("127.0.0.1".parse().unwrap()));
        assert!(is_blocked("10.0.0.5".parse().unwrap()));
        assert!(is_blocked("192.168.1.1".parse().unwrap()));
        assert!(is_blocked("172.16.4.9".parse().unwrap()));
        assert!(is_blocked("169.254.169.254".parse().unwrap())); // cloud metadata
        assert!(is_blocked("100.100.0.1".parse().unwrap())); // CGNAT
        assert!(is_blocked("::1".parse().unwrap()));
        assert!(is_blocked("fd00::1".parse().unwrap()));
        assert!(is_blocked("fe80::1".parse().unwrap()));
    }

    #[test]
    fn allows_public_addresses() {
        assert!(!is_blocked("1.1.1.1".parse().unwrap()));
        assert!(!is_blocked("93.184.216.34".parse().unwrap())); // example.com
        assert!(!is_blocked("2606:4700:4700::1111".parse().unwrap()));
    }
}
