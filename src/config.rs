//! Process configuration loaded once at startup.

use std::net::{AddrParseError, SocketAddr};

const DEFAULT_BIND_ADDR: &str = "0.0.0.0:8080";
/// Telemetry listens on its own port so the public Ingress (which fronts
/// `bind_addr`) cannot reach `/metrics` at all. 9091 is fixed by contract with
/// `3fa-infra` and `deploy/k8s/{deployment,service,networkpolicy}.yaml`.
const DEFAULT_METRICS_BIND_ADDR: &str = "0.0.0.0:9091";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedAuthConfig {
    pub base_url: String,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    pub bind_addr: SocketAddr,
    /// Second listener carrying `/metrics` only. Never fronted by the Ingress.
    pub metrics_bind_addr: SocketAddr,
    pub shared_auth: Option<SharedAuthConfig>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("DATABASE_URL must be set (Postgres connection string)")]
    MissingDatabaseUrl,
    #[error("BIND_ADDR must be a valid socket address")]
    InvalidBindAddress(#[source] AddrParseError),
    #[error("METRICS_BIND_ADDR must be a valid socket address")]
    InvalidMetricsBindAddress(#[source] AddrParseError),
    #[error("SHARED_AUTH_BASE_URL must use HTTPS, loopback HTTP, or trusted cluster HTTP")]
    InvalidSharedAuthBaseUrl,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_values(
            std::env::var("DATABASE_URL").ok(),
            std::env::var("BIND_ADDR").ok(),
            std::env::var("METRICS_BIND_ADDR").ok(),
            std::env::var("SHARED_AUTH_BASE_URL").ok(),
        )
    }

    fn from_values(
        database_url: Option<String>,
        bind_addr: Option<String>,
        metrics_bind_addr: Option<String>,
        shared_auth_base_url: Option<String>,
    ) -> Result<Self, ConfigError> {
        let database_url = database_url
            .filter(|value| !value.trim().is_empty())
            .ok_or(ConfigError::MissingDatabaseUrl)?;
        let bind_addr = bind_addr
            .unwrap_or_else(|| DEFAULT_BIND_ADDR.to_owned())
            .parse()
            .map_err(ConfigError::InvalidBindAddress)?;
        let metrics_bind_addr = metrics_bind_addr
            .unwrap_or_else(|| DEFAULT_METRICS_BIND_ADDR.to_owned())
            .parse()
            .map_err(ConfigError::InvalidMetricsBindAddress)?;
        let shared_auth = shared_auth_base_url
            .map(|value| {
                let base_url = value.trim().trim_end_matches('/').to_owned();
                if valid_shared_auth_base_url(&base_url) {
                    Ok(SharedAuthConfig { base_url })
                } else {
                    Err(ConfigError::InvalidSharedAuthBaseUrl)
                }
            })
            .transpose()?;

        Ok(Self {
            database_url,
            bind_addr,
            metrics_bind_addr,
            shared_auth,
        })
    }
}

fn valid_shared_auth_base_url(value: &str) -> bool {
    !value.is_empty()
        && (value.starts_with("https://")
            || value.starts_with("http://127.0.0.1:")
            || value.starts_with("http://localhost:")
            || value.starts_with("http://dd-shared-auth.")
            || value.starts_with("http://dd-remote-gateway."))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured(bind: Option<&str>) -> Config {
        Config::from_values(
            Some("postgres://db/threefa".to_owned()),
            bind.map(str::to_owned),
            None,
            None,
        )
        .expect("valid test config")
    }

    #[test]
    fn defaults_are_cluster_safe() {
        let config = configured(None);
        assert_eq!(config.bind_addr, "0.0.0.0:8080".parse().unwrap());
        // Telemetry defaults to its own port. The Ingress fronts 8080 only, so
        // this is what keeps /metrics off the public surface by default.
        assert_eq!(config.metrics_bind_addr, "0.0.0.0:9091".parse().unwrap());
        assert_ne!(config.metrics_bind_addr.port(), config.bind_addr.port());
    }

    #[test]
    fn explicit_values_are_parsed() {
        let config = configured(Some("127.0.0.1:9000"));
        assert_eq!(config.bind_addr, "127.0.0.1:9000".parse().unwrap());

        let config = Config::from_values(
            Some("postgres://db/threefa".to_owned()),
            None,
            Some("127.0.0.1:19091".to_owned()),
            None,
        )
        .expect("valid test config");
        assert_eq!(config.metrics_bind_addr, "127.0.0.1:19091".parse().unwrap());
    }

    #[test]
    fn missing_or_blank_database_url_is_rejected() {
        for database_url in [None, Some("   ".to_owned())] {
            assert!(matches!(
                Config::from_values(database_url, None, None, None),
                Err(ConfigError::MissingDatabaseUrl)
            ));
        }
    }

    #[test]
    fn invalid_bind_address_is_rejected() {
        assert!(matches!(
            Config::from_values(
                Some("postgres://db/threefa".to_owned()),
                Some("not-an-address".to_owned()),
                None,
                None,
            ),
            Err(ConfigError::InvalidBindAddress(_))
        ));
        assert!(matches!(
            Config::from_values(
                Some("postgres://db/threefa".to_owned()),
                None,
                Some("not-an-address".to_owned()),
                None,
            ),
            Err(ConfigError::InvalidMetricsBindAddress(_))
        ));
    }

    #[test]
    fn shared_auth_url_is_bounded_to_tls_or_trusted_http_targets() {
        for value in [
            "https://auth.oresoftware.dev/shared-auth/",
            "http://127.0.0.1:8120",
            "http://dd-shared-auth.shared-auth.svc.cluster.local:8120",
            "http://dd-remote-gateway.default.svc.cluster.local/shared-auth",
        ] {
            let config = Config::from_values(
                Some("postgres://db/threefa".to_owned()),
                None,
                None,
                Some(value.to_owned()),
            )
            .expect("trusted shared-auth URL");
            assert!(config.shared_auth.is_some());
            assert!(!config.shared_auth.unwrap().base_url.ends_with('/'));
        }
        assert!(matches!(
            Config::from_values(
                Some("postgres://db/threefa".to_owned()),
                None,
                None,
                Some("http://auth.example.test".to_owned()),
            ),
            Err(ConfigError::InvalidSharedAuthBaseUrl)
        ));
    }
}
