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
use tokio::time::{timeout, Duration};

/// Bound on exit-side name resolution. `lookup_host` uses the blocking OS
/// resolver; without this, a domain whose nameserver is black-holed pins a
/// circuit slot and a blocking-pool thread for the full OS resolver timeout.
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct Policy {
    allow_private_exit: bool,
    /// When false (`TOR_DISABLE_EXIT`), this relay refuses `Begin`: it will only
    /// ever act as an entry/middle hop and never open a connection to a real
    /// destination on a client's behalf.
    exit_enabled: bool,
    denied_exit_ports: HashSet<u16>,
    /// If `Some`, `Extend` targets must appear in this set (exact string match
    /// on the `host:port` the client requested).
    relay_peers: Option<HashSet<String>>,
}

impl Policy {
    pub fn from_env() -> Result<Policy> {
        let allow_private_exit = env_flag("TOR_EXIT_ALLOW_PRIVATE");
        let exit_enabled = !env_flag("TOR_DISABLE_EXIT");
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
        let denied_exit_ports = parse_ports(
            &std::env::var("TOR_EXIT_DENY_PORTS").unwrap_or_else(|_| "25".to_string()),
        )?;
        return Ok(Policy {
            allow_private_exit,
            exit_enabled,
            denied_exit_ports,
            relay_peers,
        });
    }

    pub fn allow_private_exit(&self) -> bool {
        return self.allow_private_exit;
    }

    /// Whether this relay is permitted to serve as an exit (open connections to
    /// real destinations). False makes it a middle-only relay.
    pub fn exit_enabled(&self) -> bool {
        return self.exit_enabled;
    }

    /// Whether an `Extend` allowlist (`TOR_RELAY_PEERS`) is configured.
    pub fn extend_allowlisted(&self) -> bool {
        return self.relay_peers.is_some();
    }

    /// Resolve `host:port` and return every address the exit policy permits.
    ///
    /// Keeping all permitted results lets the caller fall back between IPv6 and
    /// IPv4 when the resolver's first answer is unreachable.
    pub async fn resolve_exit(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>> {
        if !self.exit_enabled {
            bail!(
                "exit disabled on this relay (TOR_DISABLE_EXIT); it serves as a middle relay only"
            );
        }
        if host.is_empty()
            || host.len() > 253
            || host
                .chars()
                .any(|c| c.is_ascii_control() || c.is_ascii_whitespace())
        {
            bail!("exit destination host is invalid");
        }
        if port == 0 || self.denied_exit_ports.contains(&port) {
            bail!("exit policy blocks destination port {port}");
        }
        let addrs: Vec<SocketAddr> = timeout(RESOLVE_TIMEOUT, lookup_host((host, port)))
            .await
            .map_err(|_| anyhow::anyhow!("DNS resolution for {host}:{port} timed out"))?
            .map_err(|e| anyhow::anyhow!("DNS resolution for {host}:{port} failed: {e}"))?
            .collect();
        if addrs.is_empty() {
            bail!("no addresses resolved for {host}:{port}");
        }
        if self.allow_private_exit {
            return Ok(addrs);
        }
        let permitted: Vec<SocketAddr> = addrs
            .into_iter()
            .filter(|addr| !is_blocked(addr.ip()))
            .collect();
        if permitted.is_empty() {
            bail!("exit policy blocked all resolved addresses for {host}:{port} (private/loopback/link-local)");
        }
        return Ok(permitted);
    }

    /// Validate an `Extend` target against the optional relay-peer allowlist.
    pub fn check_extend(&self, addr: &str) -> Result<()> {
        if addr.is_empty()
            || addr.len() > 512
            || addr
                .chars()
                .any(|c| c.is_ascii_control() || c.is_ascii_whitespace())
        {
            bail!("extend target is invalid");
        }
        if let Some(peers) = &self.relay_peers {
            if !peers.contains(addr) {
                bail!("extend target {addr} not in TOR_RELAY_PEERS allowlist");
            }
        }
        return Ok(());
    }
}

fn parse_ports(raw: &str) -> Result<HashSet<u16>> {
    let mut ports = HashSet::new();
    for part in raw
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        let port = part
            .parse::<u16>()
            .map_err(|_| anyhow::anyhow!("TOR_EXIT_DENY_PORTS contains invalid port '{part}'"))?;
        if port == 0 {
            bail!("TOR_EXIT_DENY_PORTS must not contain port 0");
        }
        ports.insert(port);
    }
    return Ok(ports);
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
    let is_this_network = o[0] == 0; // 0.0.0.0/8
    let is_benchmark = o[0] == 198 && (o[1] == 18 || o[1] == 19); // 198.18.0.0/15
    let is_reserved = o[0] >= 240; // 240.0.0.0/4
                                   // 192.0.0.0/24 IETF protocol assignments (incl. 192.0.0.170/171 NAT64/DS-Lite).
    let is_ietf_proto = o[0] == 192 && o[1] == 0 && o[2] == 0;
    // 192.88.99.0/24 deprecated 6to4 anycast relay.
    let is_6to4_relay = o[0] == 192 && o[1] == 88 && o[2] == 99;
    return v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local()
        || v4.is_broadcast()
        || v4.is_documentation()
        || v4.is_unspecified()
        || v4.is_multicast()
        || is_cgnat
        || is_this_network
        || is_benchmark
        || is_reserved
        || is_ietf_proto
        || is_6to4_relay;
}

fn is_blocked_v6(v6: Ipv6Addr) -> bool {
    // IPv4-mapped (::ffff:a.b.c.d) embeds a v4 address that is reachable as v4;
    // without this, `::ffff:127.0.0.1` would bypass the v4 loopback/private
    // checks entirely. Apply the v4 rules to the embedded address.
    if let Some(v4) = v6.to_ipv4() {
        return is_blocked_v4(v4);
    }
    // 6to4 (2002:V4::/16) and NAT64 well-known prefix (64:ff9b::/96) likewise
    // embed a v4 destination; block if that embedded v4 is private/loopback.
    let seg = v6.segments();
    if seg[0] == 0x2002 {
        let v4 = Ipv4Addr::new(
            (seg[1] >> 8) as u8,
            seg[1] as u8,
            (seg[2] >> 8) as u8,
            seg[2] as u8,
        );
        return is_blocked_v4(v4);
    }
    if seg[0] == 0x0064
        && seg[1] == 0xff9b
        && seg[2] == 0
        && seg[3] == 0
        && seg[4] == 0
        && seg[5] == 0
    {
        let v4 = Ipv4Addr::new(
            (seg[6] >> 8) as u8,
            seg[6] as u8,
            (seg[7] >> 8) as u8,
            seg[7] as u8,
        );
        return is_blocked_v4(v4);
    }
    // Teredo (2001:0000::/32) embeds the client's IPv4 in the last 32 bits,
    // bit-inverted; block if that embedded v4 is private/loopback.
    if seg[0] == 0x2001 && seg[1] == 0x0000 {
        let v4 = Ipv4Addr::new(
            ((seg[6] >> 8) as u8) ^ 0xff,
            (seg[6] as u8) ^ 0xff,
            ((seg[7] >> 8) as u8) ^ 0xff,
            (seg[7] as u8) ^ 0xff,
        );
        return is_blocked_v4(v4);
    }
    // IPv4-translated (::ffff:0:0/96): seg[4]==0xffff, seg[5]==0, v4 in seg[6..8].
    // `to_ipv4()` does not decode this SIIT form, so handle it explicitly.
    if seg[0] == 0 && seg[1] == 0 && seg[2] == 0 && seg[3] == 0 && seg[4] == 0xffff && seg[5] == 0 {
        let v4 = Ipv4Addr::new(
            (seg[6] >> 8) as u8,
            seg[6] as u8,
            (seg[7] >> 8) as u8,
            seg[7] as u8,
        );
        return is_blocked_v4(v4);
    }
    let seg0 = seg[0];
    let is_ula = (seg0 & 0xfe00) == 0xfc00; // fc00::/7 unique local
    let is_link_local = (seg0 & 0xffc0) == 0xfe80; // fe80::/10
    let is_site_local = (seg0 & 0xffc0) == 0xfec0; // fec0::/10 (deprecated but routable)
    return v6.is_loopback()
        || v6.is_unspecified()
        || v6.is_multicast()
        || is_ula
        || is_link_local
        || is_site_local;
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
        assert!(is_blocked("0.1.2.3".parse().unwrap()));
        assert!(is_blocked("198.18.0.1".parse().unwrap()));
        assert!(is_blocked("240.0.0.1".parse().unwrap()));
        assert!(is_blocked("::1".parse().unwrap()));
        assert!(is_blocked("fd00::1".parse().unwrap()));
        assert!(is_blocked("fe80::1".parse().unwrap()));
        assert!(is_blocked("fec0::1".parse().unwrap()));
    }

    #[test]
    fn allows_public_addresses() {
        assert!(!is_blocked("1.1.1.1".parse().unwrap()));
        assert!(!is_blocked("93.184.216.34".parse().unwrap())); // example.com
        assert!(!is_blocked("2606:4700:4700::1111".parse().unwrap()));
    }

    #[test]
    fn blocks_v6_embedded_private_v4() {
        // IPv4-mapped forms of loopback/private must be blocked.
        assert!(is_blocked("::ffff:127.0.0.1".parse().unwrap()));
        assert!(is_blocked("::ffff:169.254.169.254".parse().unwrap())); // metadata
        assert!(is_blocked("::ffff:10.0.0.1".parse().unwrap()));
        assert!(is_blocked("::127.0.0.1".parse().unwrap()));
        // 6to4 and NAT64 wrapping a private v4.
        assert!(is_blocked("2002:0a00:0001::1".parse().unwrap())); // 6to4 of 10.0.0.1
        assert!(is_blocked("64:ff9b::7f00:1".parse().unwrap())); // NAT64 of 127.0.0.1
                                                                 // A mapped public address is still allowed.
        assert!(!is_blocked("::ffff:1.1.1.1".parse().unwrap()));
    }

    #[test]
    fn blocks_additional_reserved_and_embedded_ranges() {
        // 192.0.0.0/24 (incl. NAT64/DS-Lite) and 192.88.99.0/24 (6to4 relay).
        assert!(is_blocked("192.0.0.170".parse().unwrap()));
        assert!(is_blocked("192.0.0.1".parse().unwrap()));
        assert!(is_blocked("192.88.99.1".parse().unwrap()));
        // Neighbouring /24s stay public.
        assert!(!is_blocked("192.0.1.1".parse().unwrap()));
        assert!(!is_blocked("192.88.98.1".parse().unwrap()));
        // IPv4-translated (::ffff:0:0/96) wrapping loopback.
        assert!(is_blocked("::ffff:0:7f00:1".parse().unwrap())); // 127.0.0.1
                                                                 // Teredo (2001:0000::/32) with a bit-inverted private client v4 (10.0.0.1).
        assert!(is_blocked("2001:0:0:0:0:0:f5ff:fffe".parse().unwrap()));
        // Teredo wrapping a public client v4 (1.1.1.1) stays allowed.
        assert!(!is_blocked("2001:0:0:0:0:0:fefe:fefe".parse().unwrap()));
    }

    #[test]
    fn parses_default_exit_port_denylist() {
        let ports = parse_ports("25, 2525,25").unwrap();
        assert_eq!(ports.len(), 2);
        assert!(ports.contains(&25));
        assert!(parse_ports("25,nope").is_err());
    }

    fn policy(exit_enabled: bool, relay_peers: Option<HashSet<String>>) -> Policy {
        Policy {
            allow_private_exit: false,
            exit_enabled,
            denied_exit_ports: HashSet::new(),
            relay_peers,
        }
    }

    #[tokio::test]
    async fn middle_only_relay_refuses_exit() {
        let p = policy(false, None);
        assert!(!p.exit_enabled());
        // Rejected before any DNS resolution is attempted.
        assert!(p.resolve_exit("example.com", 443).await.is_err());
    }

    #[test]
    fn extend_allowlist_presence_is_reported() {
        assert!(!policy(true, None).extend_allowlisted());
        let peers = HashSet::from(["relay-b:9001".to_string()]);
        assert!(policy(true, Some(peers)).extend_allowlisted());
    }
}
