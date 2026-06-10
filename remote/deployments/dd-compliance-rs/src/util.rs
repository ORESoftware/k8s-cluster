use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use reqwest::Url;

pub fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

pub fn now_unix_nano_string() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .to_string()
}

pub fn next_id(prefix: &str, counter: &AtomicU64) -> String {
    let seq = counter.fetch_add(1, Ordering::Relaxed) + 1;
    format!("{prefix}-{}-{seq}", now_ms())
}

pub fn normalize_key(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

pub fn clean_text(value: &str, max_chars: usize) -> String {
    value
        .trim()
        .chars()
        .filter(|ch| !ch.is_control() || *ch == '\n' || *ch == '\t')
        .take(max_chars)
        .collect()
}

pub fn is_private_or_local_url(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return true;
    };
    let host = host.trim().to_ascii_lowercase();
    if matches!(
        host.as_str(),
        "localhost" | "ip6-localhost" | "ip6-loopback"
    ) {
        return true;
    }
    if host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".svc")
        || host.ends_with(".cluster.local")
        || host.contains("169.254.")
    {
        return true;
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return match ip {
            IpAddr::V4(ip) => is_private_ipv4(ip),
            IpAddr::V6(ip) => is_private_ipv6(ip),
        };
    }
    false
}

fn is_private_ipv4(ip: Ipv4Addr) -> bool {
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.octets()[0] == 0
}

fn is_private_ipv6(ip: Ipv6Addr) -> bool {
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_unique_local()
        || ip.is_unicast_link_local()
        || ip.segments()[0] & 0xffc0 == 0xfe80
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_key_is_stable_for_standard_aliases() {
        assert_eq!(normalize_key("SOC 2"), "soc-2");
        assert_eq!(normalize_key("ISO/IEC 27001"), "iso-iec-27001");
    }

    #[test]
    fn private_urls_are_blocked() {
        assert!(is_private_or_local_url(
            &Url::parse("http://127.0.0.1:8080").unwrap()
        ));
        assert!(is_private_or_local_url(
            &Url::parse("http://service.default.svc.cluster.local").unwrap()
        ));
        assert!(!is_private_or_local_url(
            &Url::parse("https://example.com/report.json").unwrap()
        ));
    }
}
