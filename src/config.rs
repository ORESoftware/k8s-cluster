//! Process configuration loaded once at startup.

use rand::RngCore;
use std::net::{AddrParseError, SocketAddr};

#[derive(Clone)]
pub(crate) struct SupabaseConfig {
    pub url: String,
    pub anon_key: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SharedAuthConfig {
    pub base_url: String,
}

#[derive(Clone)]
pub(crate) struct Config {
    pub bind_addr: SocketAddr,
    pub supabase: Option<SupabaseConfig>,
    pub shared_auth: Option<SharedAuthConfig>,
    pub server_secret: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ConfigError {
    #[error("BIND_ADDR/PORT must resolve to a valid socket address")]
    InvalidBindAddress(#[source] AddrParseError),
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_values(
            std::env::var("BIND_ADDR").ok(),
            std::env::var("PORT").ok(),
            std::env::var("SUPABASE_URL").ok(),
            std::env::var("SUPABASE_ANON_KEY").ok(),
            std::env::var("SHARED_AUTH_BASE_URL").ok(),
            std::env::var("SERVER_SECRET").ok(),
        )
    }

    fn from_values(
        bind_addr: Option<String>,
        port: Option<String>,
        supabase_url: Option<String>,
        supabase_anon_key: Option<String>,
        shared_auth_base_url: Option<String>,
        server_secret: Option<String>,
    ) -> Result<Self, ConfigError> {
        let bind_addr = bind_addr
            .unwrap_or_else(|| format!("0.0.0.0:{}", port.unwrap_or_else(|| "8080".to_owned())))
            .parse()
            .map_err(ConfigError::InvalidBindAddress)?;
        let supabase = supabase_config(supabase_url, supabase_anon_key);
        let shared_auth = shared_auth_config(shared_auth_base_url);
        let server_secret = match server_secret {
            Some(value) if !value.trim().is_empty() => value.into_bytes(),
            _ => {
                let mut value = vec![0u8; 32];
                rand::thread_rng().fill_bytes(&mut value);
                tracing::warn!(
                    "SERVER_SECRET is unset; enrollment cookies will not survive a restart"
                );
                value
            }
        };
        Ok(Self {
            bind_addr,
            supabase,
            shared_auth,
            server_secret,
        })
    }
}

/// Build a Supabase config from two non-empty values.
pub(crate) fn supabase_config(
    url: Option<String>,
    anon_key: Option<String>,
) -> Option<SupabaseConfig> {
    let url = url.map(|value| value.trim().trim_end_matches('/').to_owned())?;
    let anon_key = anon_key.map(|value| value.trim().to_owned())?;
    if url.is_empty() || anon_key.is_empty() {
        return None;
    }
    Some(SupabaseConfig { url, anon_key })
}

/// Accept a shared-auth base URL, requiring TLS off-cluster.
///
/// Plaintext `http://` is permitted only for loopback (local development) and
/// for in-cluster hops — any `*.svc.cluster.local` host, not one specific
/// service name. The hop has moved before (direct `dd-shared-auth` → the
/// `dd-remote-gateway` front door) and a per-hostname allow-list silently
/// disabled authentication when it did; matching the cluster DNS suffix keeps
/// the "no plaintext to the public internet" property without re-breaking on
/// the next reroute.
pub(crate) fn shared_auth_config(base_url: Option<String>) -> Option<SharedAuthConfig> {
    let base_url = base_url?.trim().trim_end_matches('/').to_owned();
    if base_url.is_empty() {
        return None;
    }
    let accepted = base_url.starts_with("https://")
        || base_url
            .strip_prefix("http://")
            .is_some_and(in_cluster_or_loopback);
    if !accepted {
        // A rejected URL leaves `shared_auth` unset, which turns every login
        // into a 503. Say so at startup instead of failing silently at request
        // time. The value is an internal service address, not a credential.
        tracing::error!(
            shared_auth.base_url = %base_url,
            "SHARED_AUTH_BASE_URL rejected: plaintext http:// is only allowed for \
             loopback or *.svc.cluster.local hosts — authentication is DISABLED"
        );
        return None;
    }
    Some(SharedAuthConfig { base_url })
}

/// True for `host[:port][/path]` authorities that stay inside the cluster.
fn in_cluster_or_loopback(after_scheme: &str) -> bool {
    let authority = after_scheme.split('/').next().unwrap_or_default();
    let host = authority.rsplit_once(':').map_or(authority, |(host, _)| host);
    host == "127.0.0.1" || host == "localhost" || host.ends_with(".svc.cluster.local")
}

#[cfg(test)]
mod tests {
    use super::{shared_auth_config, supabase_config, Config, ConfigError};

    #[test]
    fn supabase_config_requires_both_values() {
        assert!(supabase_config(None, None).is_none());
        assert!(supabase_config(Some("https://x.supabase.co".into()), None).is_none());
        assert!(supabase_config(None, Some("anon".into())).is_none());
        assert!(supabase_config(Some("  ".into()), Some("anon".into())).is_none());
        let config = supabase_config(
            Some("https://x.supabase.co/".into()),
            Some("anon-key".into()),
        )
        .expect("configured");
        assert_eq!(config.url, "https://x.supabase.co");
        assert_eq!(config.anon_key, "anon-key");
    }

    #[test]
    fn bind_address_defaults_and_explicit_override_are_stable() {
        let default =
            Config::from_values(None, None, None, None, None, Some("test-secret".to_owned()))
                .unwrap();
        assert_eq!(default.bind_addr, "0.0.0.0:8080".parse().unwrap());

        let explicit = Config::from_values(
            Some("127.0.0.1:9000".to_owned()),
            Some("9999".to_owned()),
            None,
            None,
            None,
            Some("test-secret".to_owned()),
        )
        .unwrap();
        assert_eq!(explicit.bind_addr, "127.0.0.1:9000".parse().unwrap());
    }

    #[test]
    fn port_fallback_and_invalid_addresses_are_checked() {
        let config = Config::from_values(
            None,
            Some("8181".to_owned()),
            None,
            None,
            None,
            Some("test-secret".to_owned()),
        )
        .unwrap();
        assert_eq!(config.bind_addr, "0.0.0.0:8181".parse().unwrap());
        assert!(matches!(
            Config::from_values(
                Some("not-an-address".to_owned()),
                None,
                None,
                None,
                None,
                Some("test-secret".to_owned()),
            ),
            Err(ConfigError::InvalidBindAddress(_))
        ));
    }

    #[test]
    fn explicit_secret_and_supabase_config_cross_the_module_boundary() {
        let config = Config::from_values(
            None,
            None,
            Some("https://example.supabase.co/".to_owned()),
            Some("anon".to_owned()),
            Some("https://auth.example.test/shared-auth/".to_owned()),
            Some("server-secret".to_owned()),
        )
        .unwrap();
        assert_eq!(config.server_secret, b"server-secret");
        let supabase = config.supabase.expect("Supabase config");
        assert_eq!(supabase.url, "https://example.supabase.co");
        assert_eq!(supabase.anon_key, "anon");
        assert_eq!(
            config.shared_auth.expect("shared-auth config").base_url,
            "https://auth.example.test/shared-auth"
        );
    }

    #[test]
    fn shared_auth_requires_https_except_for_loopback_and_cluster_dns() {
        assert_eq!(
            shared_auth_config(Some("https://auth.example.test/shared-auth/".into()))
                .expect("https")
                .base_url,
            "https://auth.example.test/shared-auth"
        );
        assert!(shared_auth_config(Some("http://auth.example.test".into())).is_none());
        assert!(shared_auth_config(Some("http://127.0.0.1:8120".into())).is_some());
        assert!(shared_auth_config(Some("http://localhost:8120".into())).is_some());
        assert!(shared_auth_config(Some(
            "http://dd-shared-auth.shared-auth.svc.cluster.local:8120".into()
        ))
        .is_some());
    }

    /// Both deployment manifests must survive this function. 3fa-infra routes
    /// through the default-namespace gateway; the EC2 manifest still dials the
    /// service directly. Rejecting either one disables authentication silently.
    #[test]
    fn every_deployed_shared_auth_hop_is_accepted() {
        for url in [
            "http://dd-remote-gateway.default.svc.cluster.local/shared-auth",
            "http://dd-shared-auth.shared-auth.svc.cluster.local:8120",
        ] {
            assert!(
                shared_auth_config(Some(url.into())).is_some(),
                "deployed hop rejected: {url}"
            );
        }
    }

    #[test]
    fn plaintext_off_cluster_hosts_are_rejected() {
        for url in [
            "http://evil.test",
            // Suffix lookalikes must not pass as in-cluster hosts.
            "http://attacker.test/.svc.cluster.local",
            "http://notsvc.cluster.local:8120",
            "ftp://dd-shared-auth.shared-auth.svc.cluster.local",
        ] {
            assert!(
                shared_auth_config(Some(url.into())).is_none(),
                "off-cluster plaintext accepted: {url}"
            );
        }
    }
}
