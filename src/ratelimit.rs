//! Minimal in-process, per-IP rate limiting (fixed-window).
//!
//! This is the online brute-force / registration-flood speed bump for the auth
//! endpoints. It deliberately has **no external store** (no Redis): a single
//! process keeps a small map of `IP -> window`. Argon2's inherent cost already
//! bounds throughput per request; this caps *attempts per IP per minute* on top.
//!
//! ## Client IP resolution & trust
//! The server runs behind the cluster ingress (see `deploy/k8s`), so the TCP peer
//! is the ingress pod, not the user. We therefore prefer the **leftmost**
//! `X-Forwarded-For` entry (the original client, as appended by ingress-nginx),
//! then `X-Real-IP`, and only fall back to the TCP peer address. This is
//! best-effort: an ingress that blindly forwards a *client-supplied* XFF would
//! let an attacker rotate the key, which is why this is a layer on top of Argon2,
//! not the sole control. If the limiter is ever exposed directly (no trusted
//! proxy), switch `client_ip` to use only the peer address.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::http::HeaderMap;

/// Fixed-window request counter, keyed by client IP.
pub struct RateLimiter {
    window: Duration,
    max: u32,
    buckets: Mutex<HashMap<IpAddr, Window>>,
}

struct Window {
    count: u32,
    started: Instant,
}

impl RateLimiter {
    pub fn new(max: u32, window: Duration) -> Self {
        Self {
            window,
            max,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Record an attempt from `key`; return `true` if it is within the limit.
    pub fn check(&self, key: IpAddr) -> bool {
        let now = Instant::now();
        let mut map = self.buckets.lock().unwrap_or_else(|e| e.into_inner());

        // Opportunistically evict stale windows so the map can't grow without
        // bound under IP churn (e.g. a spoofed-XFF flood).
        if map.len() > 10_000 {
            map.retain(|_, w| now.duration_since(w.started) < self.window);
        }

        let w = map.entry(key).or_insert(Window {
            count: 0,
            started: now,
        });
        if now.duration_since(w.started) >= self.window {
            w.count = 0;
            w.started = now;
        }
        if w.count >= self.max {
            false
        } else {
            w.count += 1;
            true
        }
    }
}

/// Resolve the effective client IP for rate-limiting (see module docs for the
/// trust model). Prefers `X-Forwarded-For` (leftmost) / `X-Real-IP`, else peer.
pub fn client_ip(headers: &HeaderMap, peer: SocketAddr) -> IpAddr {
    if let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
        if let Some(first) = xff.split(',').next() {
            if let Ok(ip) = first.trim().parse::<IpAddr>() {
                return ip;
            }
        }
    }
    if let Some(real) = headers.get("x-real-ip").and_then(|v| v.to_str().ok()) {
        if let Ok(ip) = real.trim().parse::<IpAddr>() {
            return ip;
        }
    }
    peer.ip()
}

/// Read a per-minute limit from `env_var`, falling back to `default`.
pub fn limit_from_env(env_var: &str, default: u32) -> u32 {
    std::env::var(env_var)
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn ip(n: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, n))
    }

    #[test]
    fn allows_up_to_limit_then_blocks() {
        let rl = RateLimiter::new(3, Duration::from_secs(60));
        assert!(rl.check(ip(1)));
        assert!(rl.check(ip(1)));
        assert!(rl.check(ip(1)));
        assert!(!rl.check(ip(1)), "4th attempt in window must be blocked");
    }

    #[test]
    fn separate_ips_have_separate_budgets() {
        let rl = RateLimiter::new(1, Duration::from_secs(60));
        assert!(rl.check(ip(1)));
        assert!(!rl.check(ip(1)));
        // A different IP is unaffected.
        assert!(rl.check(ip(2)));
    }

    #[test]
    fn window_resets_after_elapse() {
        let rl = RateLimiter::new(1, Duration::from_millis(20));
        assert!(rl.check(ip(1)));
        assert!(!rl.check(ip(1)));
        std::thread::sleep(Duration::from_millis(30));
        assert!(rl.check(ip(1)), "window should reset after it elapses");
    }

    #[test]
    fn xff_leftmost_wins_over_peer() {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", "203.0.113.7, 10.0.0.1".parse().unwrap());
        let peer: SocketAddr = "10.0.0.1:5000".parse().unwrap();
        assert_eq!(client_ip(&h, peer), "203.0.113.7".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn falls_back_to_peer_without_headers() {
        let h = HeaderMap::new();
        let peer: SocketAddr = "198.51.100.9:443".parse().unwrap();
        assert_eq!(client_ip(&h, peer), "198.51.100.9".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn ignores_malformed_xff() {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", "not-an-ip".parse().unwrap());
        let peer: SocketAddr = "198.51.100.9:443".parse().unwrap();
        assert_eq!(client_ip(&h, peer), "198.51.100.9".parse::<IpAddr>().unwrap());
    }
}
