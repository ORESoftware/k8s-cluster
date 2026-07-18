use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE, URL_SAFE_NO_PAD},
    Engine as _,
};
use std::{collections::HashSet, env, path::PathBuf, time::Duration};
use thiserror::Error;

#[derive(Clone)]
pub struct Config {
    pub port: u16,
    pub static_dir: PathBuf,
    pub app_asset_dir: PathBuf,
    pub database_url: String,
    pub database_max_connections: u32,
    pub app_base_url: String,
    pub allowed_origins: HashSet<String>,
    pub session_cookie: String,
    pub cookie_secure: bool,
    pub session_encryption_key: Vec<u8>,
    pub session_ttl: Duration,
    pub login_rate_limit_attempts: u32,
    pub login_rate_limit_global_attempts: u32,
    pub login_rate_limit_window: Duration,
    pub login_rate_limit_max_keys: usize,
    pub login_auth_max_concurrency: usize,
    pub bearer_auth_max_concurrency: usize,
    pub supabase_url: String,
    pub supabase_publishable_key: String,
}

#[derive(Clone)]
pub struct MigrationConfig {
    pub database_url: String,
    pub database_max_connections: u32,
}

/// Configuration loaded only by the separately deployed session revoker.
#[derive(Clone)]
pub struct SessionRevokerConfig {
    pub database_url: String,
    pub database_max_connections: u32,
    pub session_encryption_key: Vec<u8>,
    pub supabase_url: String,
    pub supabase_publishable_key: String,
}

impl std::fmt::Debug for MigrationConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MigrationConfig")
            .field("database_url", &"[REDACTED]")
            .field("database_max_connections", &self.database_max_connections)
            .finish()
    }
}

impl std::fmt::Debug for SessionRevokerConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SessionRevokerConfig")
            .field("database_url", &"[REDACTED]")
            .field("database_max_connections", &self.database_max_connections)
            .field("session_encryption_key", &"[REDACTED]")
            .field("supabase_url", &self.supabase_url)
            .field("supabase_publishable_key", &"[REDACTED]")
            .finish()
    }
}

impl std::fmt::Debug for Config {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Config")
            .field("port", &self.port)
            .field("static_dir", &self.static_dir)
            .field("app_asset_dir", &self.app_asset_dir)
            .field("database_url", &"[REDACTED]")
            .field("database_max_connections", &self.database_max_connections)
            .field("app_base_url", &self.app_base_url)
            .field("allowed_origins", &self.allowed_origins)
            .field("session_cookie", &self.session_cookie)
            .field("cookie_secure", &self.cookie_secure)
            .field("session_encryption_key", &"[REDACTED]")
            .field("session_ttl", &self.session_ttl)
            .field("login_rate_limit_attempts", &self.login_rate_limit_attempts)
            .field(
                "login_rate_limit_global_attempts",
                &self.login_rate_limit_global_attempts,
            )
            .field("login_rate_limit_window", &self.login_rate_limit_window)
            .field("login_rate_limit_max_keys", &self.login_rate_limit_max_keys)
            .field(
                "login_auth_max_concurrency",
                &self.login_auth_max_concurrency,
            )
            .field(
                "bearer_auth_max_concurrency",
                &self.bearer_auth_max_concurrency,
            )
            .field("supabase_url", &self.supabase_url)
            .field("supabase_publishable_key", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("required environment variable {0} is missing")]
    Missing(&'static str),
    #[error("environment variable {name} has an invalid value: {message}")]
    Invalid { name: &'static str, message: String },
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let app_base_url = validated_origin("APP_BASE_URL", &required("APP_BASE_URL")?)?;
        let cookie_secure = optional_bool("COOKIE_SECURE", app_base_url.starts_with("https://"))?;
        let default_cookie = if cookie_secure {
            "__Host-canonical_session"
        } else {
            "canonical_session"
        };
        let origins = env::var("APP_ALLOWED_ORIGINS").unwrap_or_else(|_| app_base_url.clone());
        let allowed_origins = origins
            .split(',')
            .map(str::trim)
            .filter(|origin| !origin.is_empty())
            .map(|origin| validated_origin("APP_ALLOWED_ORIGINS", origin))
            .collect::<Result<HashSet<_>, _>>()?;
        if allowed_origins.is_empty() {
            return Err(ConfigError::Invalid {
                name: "APP_ALLOWED_ORIGINS",
                message: "at least one exact origin is required".into(),
            });
        }
        if allowed_origins.len() > 16 {
            return Err(ConfigError::Invalid {
                name: "APP_ALLOWED_ORIGINS",
                message: "at most 16 exact origins are allowed".into(),
            });
        }
        if !allowed_origins.contains(&app_base_url) {
            return Err(ConfigError::Invalid {
                name: "APP_ALLOWED_ORIGINS",
                message: "must include APP_BASE_URL".into(),
            });
        }

        let session_encryption_key = session_encryption_key_from_env()?;

        let supabase_url = validated_supabase_url(required("SUPABASE_URL")?)?;

        let session_ttl_days = optional_parse::<u64>("APP_SESSION_TTL_DAYS", 30)?;
        if !(1..=30).contains(&session_ttl_days) {
            return Err(ConfigError::Invalid {
                name: "APP_SESSION_TTL_DAYS",
                message: "must be between 1 and 30 days".into(),
            });
        }
        let session_ttl = session_ttl_from_days(session_ttl_days)?;

        let session_cookie =
            env::var("APP_SESSION_COOKIE").unwrap_or_else(|_| default_cookie.to_owned());
        if session_cookie.starts_with("__Host-") && !cookie_secure {
            return Err(ConfigError::Invalid {
                name: "APP_SESSION_COOKIE",
                message: "a __Host- cookie requires COOKIE_SECURE=true".into(),
            });
        }
        if !is_loopback_origin(&app_base_url) && !cookie_secure {
            return Err(ConfigError::Invalid {
                name: "COOKIE_SECURE",
                message: "must be true for non-loopback application origins".into(),
            });
        }
        if !is_loopback_origin(&app_base_url) && !session_cookie.starts_with("__Host-") {
            return Err(ConfigError::Invalid {
                name: "APP_SESSION_COOKIE",
                message: "must use a __Host- prefix for non-loopback application origins".into(),
            });
        }

        let login_rate_limit_attempts = optional_parse("LOGIN_RATE_LIMIT_ATTEMPTS", 5)?;
        if !(1..=20).contains(&login_rate_limit_attempts) {
            return Err(ConfigError::Invalid {
                name: "LOGIN_RATE_LIMIT_ATTEMPTS",
                message: "must be between 1 and 20".into(),
            });
        }
        let login_rate_limit_window_seconds =
            optional_parse::<u64>("LOGIN_RATE_LIMIT_WINDOW_SECONDS", 600)?;
        if !(1..=3_600).contains(&login_rate_limit_window_seconds) {
            return Err(ConfigError::Invalid {
                name: "LOGIN_RATE_LIMIT_WINDOW_SECONDS",
                message: "must be between 1 and 3600 seconds".into(),
            });
        }
        let login_rate_limit_max_keys = optional_parse("LOGIN_RATE_LIMIT_MAX_KEYS", 4_096)?;
        if !(128..=65_536).contains(&login_rate_limit_max_keys) {
            return Err(ConfigError::Invalid {
                name: "LOGIN_RATE_LIMIT_MAX_KEYS",
                message: "must be between 128 and 65536".into(),
            });
        }
        let login_rate_limit_global_attempts =
            optional_parse("LOGIN_RATE_LIMIT_GLOBAL_ATTEMPTS", 500)?;
        if !(10..=100_000).contains(&login_rate_limit_global_attempts) {
            return Err(ConfigError::Invalid {
                name: "LOGIN_RATE_LIMIT_GLOBAL_ATTEMPTS",
                message: "must be between 10 and 100000".into(),
            });
        }
        let login_auth_max_concurrency = validated_login_auth_max_concurrency(optional_parse(
            "LOGIN_AUTH_MAX_CONCURRENCY",
            16,
        )?)?;
        let bearer_auth_max_concurrency = optional_parse("BEARER_AUTH_MAX_CONCURRENCY", 32)?;
        if !(1..=256).contains(&bearer_auth_max_concurrency) {
            return Err(ConfigError::Invalid {
                name: "BEARER_AUTH_MAX_CONCURRENCY",
                message: "must be between 1 and 256".into(),
            });
        }

        let database_max_connections = optional_parse("DATABASE_MAX_CONNECTIONS", 10)?;
        if !(1..=100).contains(&database_max_connections) {
            return Err(ConfigError::Invalid {
                name: "DATABASE_MAX_CONNECTIONS",
                message: "must be between 1 and 100".into(),
            });
        }

        Ok(Self {
            port: optional_parse("PORT", 8081)?,
            static_dir: env::var_os("STATIC_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("static")),
            app_asset_dir: env::var_os("APP_ASSET_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("client/dist")),
            database_url: required("DATABASE_URL")?,
            database_max_connections,
            app_base_url,
            allowed_origins,
            session_cookie,
            cookie_secure,
            session_encryption_key,
            session_ttl,
            login_rate_limit_attempts,
            login_rate_limit_global_attempts,
            login_rate_limit_window: Duration::from_secs(login_rate_limit_window_seconds),
            login_rate_limit_max_keys,
            login_auth_max_concurrency,
            bearer_auth_max_concurrency,
            supabase_url,
            supabase_publishable_key: validated_supabase_publishable_key(required(
                "SUPABASE_PUBLISHABLE_KEY",
            )?)?,
        })
    }
}

fn is_loopback_origin(origin: &str) -> bool {
    reqwest::Url::parse(origin)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .is_some_and(|host| {
            host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1"
        })
}

fn validated_origin(name: &'static str, value: &str) -> Result<String, ConfigError> {
    let parsed = reqwest::Url::parse(value).map_err(|error| ConfigError::Invalid {
        name,
        message: format!("expected an absolute HTTP(S) origin: {error}"),
    })?;
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.path() != "/"
    {
        return Err(ConfigError::Invalid {
            name,
            message: "expected an origin without credentials, path, query, or fragment".into(),
        });
    }
    let host = parsed.host_str().ok_or_else(|| ConfigError::Invalid {
        name,
        message: "a host is required".into(),
    })?;
    let is_exact_loopback =
        host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1";
    match parsed.scheme() {
        "https" => {}
        "http" if is_exact_loopback => {}
        _ => {
            return Err(ConfigError::Invalid {
                name,
                message: "HTTPS is required except for exact loopback origins".into(),
            });
        }
    }
    Ok(parsed.origin().ascii_serialization())
}

impl MigrationConfig {
    /// Loads only the privileged connection needed by the migration command.
    /// Keeping this separate from `Config` means a deploy job never needs the
    /// HTTP server's Supabase or session-encryption settings.
    pub fn from_env() -> Result<Self, ConfigError> {
        let database_max_connections = optional_parse("MIGRATION_DATABASE_MAX_CONNECTIONS", 2)?;
        if !(1..=16).contains(&database_max_connections) {
            return Err(ConfigError::Invalid {
                name: "MIGRATION_DATABASE_MAX_CONNECTIONS",
                message: "must be between 1 and 16".into(),
            });
        }
        Ok(Self {
            database_url: required("MIGRATION_DATABASE_URL")?,
            database_max_connections,
        })
    }
}

impl SessionRevokerConfig {
    /// Loads no customer HTTP, cookie, static-asset, or migration settings.
    /// The worker credential is intentionally distinct from `DATABASE_URL`.
    pub fn from_env() -> Result<Self, ConfigError> {
        let database_max_connections =
            optional_parse("SESSION_REVOCATION_DATABASE_MAX_CONNECTIONS", 2)?;
        if !(1..=8).contains(&database_max_connections) {
            return Err(ConfigError::Invalid {
                name: "SESSION_REVOCATION_DATABASE_MAX_CONNECTIONS",
                message: "must be between 1 and 8".into(),
            });
        }
        Ok(Self {
            database_url: required("SESSION_REVOCATION_DATABASE_URL")?,
            database_max_connections,
            session_encryption_key: session_encryption_key_from_env()?,
            supabase_url: validated_supabase_url(required("SUPABASE_URL")?)?,
            supabase_publishable_key: validated_supabase_publishable_key(required(
                "SUPABASE_PUBLISHABLE_KEY",
            )?)?,
        })
    }
}

fn session_encryption_key_from_env() -> Result<Vec<u8>, ConfigError> {
    let key_text = required("APP_SESSION_ENCRYPTION_KEY")?;
    let key = STANDARD
        .decode(key_text)
        .map_err(|_| ConfigError::Invalid {
            name: "APP_SESSION_ENCRYPTION_KEY",
            message: "expected standard base64".into(),
        })?;
    if key.len() != 32 {
        return Err(ConfigError::Invalid {
            name: "APP_SESSION_ENCRYPTION_KEY",
            message: "decoded key must be exactly 32 bytes".into(),
        });
    }
    if key.iter().all(|byte| *byte == 0) {
        return Err(ConfigError::Invalid {
            name: "APP_SESSION_ENCRYPTION_KEY",
            message: "replace the all-zero example with a random key".into(),
        });
    }
    Ok(key)
}

fn session_ttl_from_days(days: u64) -> Result<Duration, ConfigError> {
    let seconds = days
        .checked_mul(24)
        .and_then(|hours| hours.checked_mul(60))
        .and_then(|minutes| minutes.checked_mul(60))
        .ok_or_else(|| ConfigError::Invalid {
            name: "APP_SESSION_TTL_DAYS",
            message: "value is too large".into(),
        })?;
    Ok(Duration::from_secs(seconds))
}

fn validated_login_auth_max_concurrency(value: usize) -> Result<usize, ConfigError> {
    if (1..=256).contains(&value) {
        Ok(value)
    } else {
        Err(ConfigError::Invalid {
            name: "LOGIN_AUTH_MAX_CONCURRENCY",
            message: "must be between 1 and 256".into(),
        })
    }
}

fn validated_supabase_url(value: String) -> Result<String, ConfigError> {
    let parsed = reqwest::Url::parse(&value).map_err(|error| ConfigError::Invalid {
        name: "SUPABASE_URL",
        message: format!("expected an absolute URL: {error}"),
    })?;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(ConfigError::Invalid {
            name: "SUPABASE_URL",
            message: "credentials are not allowed".into(),
        });
    }
    if parsed.fragment().is_some() {
        return Err(ConfigError::Invalid {
            name: "SUPABASE_URL",
            message: "fragments are not allowed".into(),
        });
    }
    if parsed.query().is_some() {
        return Err(ConfigError::Invalid {
            name: "SUPABASE_URL",
            message: "query strings are not allowed".into(),
        });
    }
    let host = parsed.host_str().ok_or_else(|| ConfigError::Invalid {
        name: "SUPABASE_URL",
        message: "a host is required".into(),
    })?;
    let is_exact_loopback = host.eq_ignore_ascii_case("localhost")
        || host == "127.0.0.1"
        || host == "::1"
        || host == "[::1]";
    match parsed.scheme() {
        "https" => {}
        "http" if is_exact_loopback => {}
        _ => {
            return Err(ConfigError::Invalid {
                name: "SUPABASE_URL",
                message: "HTTPS is required except for localhost, 127.0.0.1, or ::1".into(),
            });
        }
    }
    Ok(parsed.as_str().trim_end_matches('/').to_owned())
}

/// Accepts only Supabase's low-privilege API-key forms.
///
/// New projects should use an opaque `sb_publishable_...` key. Legacy projects
/// may still use the JWT-shaped `anon` key, but a legacy `service_role` JWT or
/// an opaque `sb_secret_...` key must fail closed if it is accidentally mounted
/// into the customer-facing server.
fn validated_supabase_publishable_key(value: String) -> Result<String, ConfigError> {
    let value = value.trim();
    if value
        .strip_prefix("sb_publishable_")
        .is_some_and(|suffix| !suffix.is_empty())
    {
        return Ok(value.to_owned());
    }

    let invalid = || {
        ConfigError::Invalid {
        name: "SUPABASE_PUBLISHABLE_KEY",
        message: "expected an sb_publishable_ key or a legacy anon JWT; secret/service-role keys are forbidden"
            .into(),
    }
    };
    if value.starts_with("sb_secret_") {
        return Err(invalid());
    }

    let mut segments = value.split('.');
    let (Some(header), Some(payload), Some(signature), None) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return Err(invalid());
    };
    if header.is_empty() || payload.is_empty() || signature.is_empty() {
        return Err(invalid());
    }
    let payload = URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| URL_SAFE.decode(payload))
        .map_err(|_| invalid())?;
    let claims: serde_json::Value = serde_json::from_slice(&payload).map_err(|_| invalid())?;
    if claims.get("role").and_then(serde_json::Value::as_str) != Some("anon") {
        return Err(invalid());
    }
    Ok(value.to_owned())
}

fn required(name: &'static str) -> Result<String, ConfigError> {
    env::var(name)
        .map_err(|_| ConfigError::Missing(name))
        .and_then(|value| {
            if value.trim().is_empty() {
                Err(ConfigError::Missing(name))
            } else {
                Ok(value)
            }
        })
}

fn optional_parse<T>(name: &'static str, default: T) -> Result<T, ConfigError>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match env::var(name) {
        Ok(value) => value.parse().map_err(|error: T::Err| ConfigError::Invalid {
            name,
            message: error.to_string(),
        }),
        Err(_) => Ok(default),
    }
}

fn optional_bool(name: &'static str, default: bool) -> Result<bool, ConfigError> {
    match env::var(name) {
        Ok(value) => match value.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" => Ok(true),
            "0" | "false" | "no" => Ok(false),
            _ => Err(ConfigError::Invalid {
                name,
                message: "expected true or false".into(),
            }),
        },
        Err(_) => Ok(default),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        session_ttl_from_days, validated_login_auth_max_concurrency, validated_origin,
        validated_supabase_publishable_key, validated_supabase_url,
    };
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use std::time::Duration;

    fn legacy_key(role: &str) -> String {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD.encode(
            serde_json::json!({ "role": role, "iss": "supabase" })
                .to_string()
                .as_bytes(),
        );
        format!("{header}.{payload}.test-signature")
    }

    #[test]
    fn supabase_url_requires_https_except_for_exact_loopback_hosts() {
        assert_eq!(
            validated_supabase_url("https://project.supabase.co/".into()).unwrap(),
            "https://project.supabase.co"
        );
        assert!(validated_supabase_url("http://project.supabase.co".into()).is_err());
        assert!(validated_supabase_url("http://localhost.evil.test".into()).is_err());
        assert!(validated_supabase_url("http://127.0.0.2".into()).is_err());
        assert!(validated_supabase_url("http://localhost:54321/".into()).is_ok());
        assert!(validated_supabase_url("http://127.0.0.1:54321/".into()).is_ok());
        assert!(validated_supabase_url("http://[::1]:54321/".into()).is_ok());
    }

    #[test]
    fn supabase_url_rejects_credentials_fragments_and_queries() {
        assert!(validated_supabase_url("https://user:pass@example.com".into()).is_err());
        assert!(validated_supabase_url("https://example.com/#fragment".into()).is_err());
        assert!(validated_supabase_url("https://example.com/?query=value".into()).is_err());
    }

    #[test]
    fn supabase_publishable_key_accepts_only_low_privilege_forms() {
        assert_eq!(
            validated_supabase_publishable_key("sb_publishable_example".into()).unwrap(),
            "sb_publishable_example"
        );
        let anon = legacy_key("anon");
        assert_eq!(
            validated_supabase_publishable_key(anon.clone()).unwrap(),
            anon
        );

        assert!(validated_supabase_publishable_key("sb_secret_example".into()).is_err());
        assert!(validated_supabase_publishable_key(legacy_key("service_role")).is_err());
        assert!(validated_supabase_publishable_key("test-publishable-key".into()).is_err());
        assert!(validated_supabase_publishable_key("sb_publishable_".into()).is_err());
    }

    #[test]
    fn session_ttl_days_cannot_overflow_seconds() {
        assert_eq!(
            session_ttl_from_days(30).unwrap(),
            Duration::from_secs(30 * 24 * 60 * 60)
        );
        assert!(session_ttl_from_days(u64::MAX).is_err());
    }

    #[test]
    fn login_auth_concurrency_is_bounded() {
        assert_eq!(validated_login_auth_max_concurrency(1).unwrap(), 1);
        assert_eq!(validated_login_auth_max_concurrency(256).unwrap(), 256);
        assert!(validated_login_auth_max_concurrency(0).is_err());
        assert!(validated_login_auth_max_concurrency(257).is_err());
    }

    #[test]
    fn application_origins_are_exact_and_secure() {
        assert_eq!(
            validated_origin("APP_BASE_URL", "https://app.example.com/").unwrap(),
            "https://app.example.com"
        );
        assert!(validated_origin("APP_BASE_URL", "https://app.example.com/path").is_err());
        assert!(validated_origin("APP_BASE_URL", "https://app.example.com/?query=1").is_err());
        assert!(validated_origin("APP_BASE_URL", "http://app.example.com").is_err());
        assert!(validated_origin("APP_BASE_URL", "http://localhost:8081").is_ok());
    }
}
