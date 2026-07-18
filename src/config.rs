//! Process configuration loaded once at startup.

use std::net::{AddrParseError, SocketAddr};

const DEFAULT_BIND_ADDR: &str = "0.0.0.0:8080";
const DEFAULT_AUTH_MAX_CONCURRENT: usize = 2;

#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    pub bind_addr: SocketAddr,
    pub auth_max_concurrent: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("DATABASE_URL must be set (Postgres connection string)")]
    MissingDatabaseUrl,
    #[error("BIND_ADDR must be a valid socket address")]
    InvalidBindAddress(#[source] AddrParseError),
    #[error("THREEFA_AUTH_MAX_CONCURRENT must be a positive integer")]
    InvalidAuthConcurrency,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_values(
            std::env::var("DATABASE_URL").ok(),
            std::env::var("BIND_ADDR").ok(),
            std::env::var("THREEFA_AUTH_MAX_CONCURRENT").ok(),
        )
    }

    fn from_values(
        database_url: Option<String>,
        bind_addr: Option<String>,
        auth_max_concurrent: Option<String>,
    ) -> Result<Self, ConfigError> {
        let database_url = database_url
            .filter(|value| !value.trim().is_empty())
            .ok_or(ConfigError::MissingDatabaseUrl)?;
        let bind_addr = bind_addr
            .unwrap_or_else(|| DEFAULT_BIND_ADDR.to_owned())
            .parse()
            .map_err(ConfigError::InvalidBindAddress)?;
        let auth_max_concurrent = match auth_max_concurrent {
            Some(value) => value
                .parse::<usize>()
                .ok()
                .filter(|value| *value > 0)
                .ok_or(ConfigError::InvalidAuthConcurrency)?,
            None => DEFAULT_AUTH_MAX_CONCURRENT,
        };

        Ok(Self {
            database_url,
            bind_addr,
            auth_max_concurrent,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured(bind: Option<&str>, auth_concurrency: Option<&str>) -> Config {
        Config::from_values(
            Some("postgres://db/threefa".to_owned()),
            bind.map(str::to_owned),
            auth_concurrency.map(str::to_owned),
        )
        .expect("valid test config")
    }

    #[test]
    fn defaults_are_cluster_safe() {
        let config = configured(None, None);
        assert_eq!(config.bind_addr, "0.0.0.0:8080".parse().unwrap());
        assert_eq!(config.auth_max_concurrent, 2);
    }

    #[test]
    fn explicit_values_are_parsed() {
        let config = configured(Some("127.0.0.1:9000"), Some("7"));
        assert_eq!(config.bind_addr, "127.0.0.1:9000".parse().unwrap());
        assert_eq!(config.auth_max_concurrent, 7);
    }

    #[test]
    fn missing_or_blank_database_url_is_rejected() {
        for database_url in [None, Some("   ".to_owned())] {
            assert!(matches!(
                Config::from_values(database_url, None, None),
                Err(ConfigError::MissingDatabaseUrl)
            ));
        }
    }

    #[test]
    fn invalid_bind_and_auth_limits_are_rejected() {
        assert!(matches!(
            Config::from_values(
                Some("postgres://db/threefa".to_owned()),
                Some("not-an-address".to_owned()),
                None,
            ),
            Err(ConfigError::InvalidBindAddress(_))
        ));
        for value in ["0", "nope"] {
            assert!(matches!(
                Config::from_values(
                    Some("postgres://db/threefa".to_owned()),
                    None,
                    Some(value.to_owned()),
                ),
                Err(ConfigError::InvalidAuthConcurrency)
            ));
        }
    }
}
