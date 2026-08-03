//! Environment-driven configuration.
//!
//! Everything sensitive (provider API keys, DB URL, signing key, Management PAT) is
//! injected via the environment — in the cluster from an `ExternalSecret`
//! pointing at the shared `ClusterSecretStore` (`deploy/k8s/externalsecret.yaml`).
//! Nothing here reads from disk except the PEM signing key, which may be given
//! inline or as a path.

use std::net::SocketAddr;

use serde::Deserialize;

use crate::error::ConfigError;

/// One Supabase project this server accepts tokens from. Each org in the
/// account (3fa-app, athlet-o-store, fiducia-cloud, sonus-auris, …) maps to one
/// project with its own issuer and JWKS.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupabaseProject {
    /// Stable slug used in logs, metrics, and the `supabase_project` mirror
    /// column, e.g. `"fiducia-cloud"`.
    pub name: String,
    /// Supabase project ref, e.g. `abcdefghijklmnopqrst`. Used to derive
    /// `issuer`/`jwks_url` when those are not given explicitly.
    pub project_ref: String,
    /// Token issuer to pin (`iss`). Defaults to
    /// `https://<project_ref>.supabase.co/auth/v1`.
    #[serde(default)]
    pub issuer: Option<String>,
    /// JWKS endpoint. Defaults to `<issuer>/.well-known/jwks.json`.
    #[serde(default)]
    pub jwks_url: Option<String>,
    /// Expected audience. Supabase stamps `authenticated` on end-user tokens.
    #[serde(default = "default_audience")]
    pub audience: String,
    /// Name of the environment variable holding the publishable/legacy anon key.
    /// The key itself must never appear in `AUTH_SUPABASE_PROJECTS`.
    #[serde(default)]
    pub publishable_key_env: Option<String>,
    /// Name of the environment variable holding a modern server-side secret key.
    #[serde(default)]
    pub secret_key_env: Option<String>,
    /// Name of the environment variable holding a legacy service-role key.
    #[serde(default)]
    pub service_role_key_env: Option<String>,
    /// Legacy HS256 only: name of the environment variable holding the JWT secret.
    /// Prefer asymmetric JWKS verification and omit this for modern projects.
    #[serde(default)]
    pub jwt_secret_env: Option<String>,
    /// Resolved runtime credentials. Serde always skips these fields so secret
    /// values cannot be embedded in the provider metadata JSON by mistake.
    #[serde(skip)]
    pub api_keys: SupabaseApiKeys,
    #[serde(skip)]
    pub hs256_secret: Option<String>,
}

/// Runtime-only Supabase credentials resolved from environment-variable names in
/// `SupabaseProject`. This type deliberately does not implement `Debug` or
/// `Serialize`, which keeps accidental config logging from exposing key values.
#[derive(Clone, Default)]
pub struct SupabaseApiKeys {
    pub publishable_key: Option<String>,
    pub secret_key: Option<String>,
    pub service_role_key: Option<String>,
}

fn default_audience() -> String {
    "authenticated".to_string()
}

impl SupabaseProject {
    /// The issuer to pin, derived from `project_ref` when not set explicitly.
    pub fn issuer(&self) -> String {
        self.issuer
            .clone()
            .unwrap_or_else(|| format!("https://{}.supabase.co/auth/v1", self.project_ref))
    }

    /// The JWKS URL, derived from the issuer when not set explicitly.
    pub fn jwks_url(&self) -> String {
        self.jwks_url
            .clone()
            .unwrap_or_else(|| format!("{}/.well-known/jwks.json", self.issuer()))
    }
}

/// How this server signs the unified OreSoftware JWTs it mints.
#[derive(Clone)]
pub struct SigningConfig {
    /// PKCS#8 PEM of an EC P-256 private key (ES256). Held in memory only.
    pub ec_private_pem: String,
    /// `kid` advertised in our JWKS and stamped on tokens. Downstream services
    /// select the verification key by this id, so keep it stable across rotation
    /// windows (publish old+new together while rotating).
    pub key_id: String,
    /// `iss` on the tokens we mint.
    pub issuer: String,
    /// `aud` on the tokens we mint — the set of OreSoftware services meant to
    /// accept them.
    pub audience: String,
    /// Lifetime of a minted token, in seconds.
    pub ttl_secs: u64,
}

/// AWS RDS identity-mirror connection.
#[derive(Clone)]
pub struct DbConfig {
    /// `postgres://…` DSN. `search_path` should include `shared_auth`.
    pub url: String,
    pub max_connections: u32,
}

/// Optional Redis/Valkey cache in the private network. It is never the source
/// of truth; losing it only removes acceleration and distributed rate limits.
#[derive(Clone)]
pub struct RedisConfig {
    pub url: String,
    pub key_prefix: String,
}

#[derive(Clone)]
pub struct SessionConfig {
    pub refresh_ttl_secs: u64,
    pub allow_registration: bool,
}

/// Optional RDS-backed passwordless login delivered through SendGrid.
///
/// Empty values keep the server deployable and leave only the magic-link
/// request endpoint unavailable. Local password login and Supabase exchange
/// continue to work independently.
#[derive(Clone)]
pub struct MagicLinkConfig {
    pub sendgrid_api_key: Option<String>,
    pub otp_pepper: Option<String>,
    pub from_email: Option<String>,
    pub from_name: String,
    pub link_base_url: Option<String>,
    pub ttl_secs: u64,
    pub allow_signup: bool,
}

impl MagicLinkConfig {
    pub fn is_enabled(&self) -> bool {
        self.sendgrid_api_key.is_some()
            && self.otp_pepper.is_some()
            && self.from_email.is_some()
            && self.link_base_url.is_some()
    }
}

#[derive(Clone)]
pub struct TwilioVerifyConfig {
    pub account_sid: Option<String>,
    pub auth_token: Option<String>,
    pub service_sid: Option<String>,
}

impl TwilioVerifyConfig {
    pub fn is_enabled(&self) -> bool {
        self.account_sid.is_some() && self.auth_token.is_some() && self.service_sid.is_some()
    }
}

/// Fully-resolved configuration.
#[derive(Clone)]
pub struct AppConfig {
    pub bind_addr: SocketAddr,
    pub projects: Vec<SupabaseProject>,
    pub signing: SigningConfig,
    /// Optional: without a DB the server still verifies + mints, it just skips
    /// mirroring identities.
    pub db: Option<DbConfig>,
    pub redis: Option<RedisConfig>,
    pub sessions: SessionConfig,
    pub magic_links: MagicLinkConfig,
    pub twilio_verify: TwilioVerifyConfig,
    /// HMAC secret for `/internal/webhook/sync`. When absent the endpoint is
    /// disabled (404), rather than exposed without authentication.
    pub webhook_secret: Option<String>,
    /// Shared service credential required to call `/auth/introspect`. When set,
    /// callers must present `Authorization: Bearer <secret>` and unauthenticated
    /// callers are rejected. When absent, introspection stays open for backward
    /// compatibility and logs a one-time deprecation warning. Setting this is
    /// strongly recommended, since introspection returns full token claims.
    pub introspect_secret: Option<String>,
    /// Optional CORS allow-list for browser callers.
    pub cors_allow_origins: Vec<String>,
}

impl AppConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let bind_addr = env_or("AUTH_BIND_ADDR", "0.0.0.0:8120")
            .parse()
            .map_err(|_| ConfigError::Invalid("AUTH_BIND_ADDR"))?;

        // Supabase is a secondary authority. An empty registry is valid for a
        // local-only deployment and lets future provider adapters be introduced
        // without making Supabase a hard startup dependency.
        let projects_json = env_or("AUTH_SUPABASE_PROJECTS", "[]");
        let mut projects: Vec<SupabaseProject> = serde_json::from_str(&projects_json)
            .map_err(|_| ConfigError::Invalid("AUTH_SUPABASE_PROJECTS (expected JSON array)"))?;
        resolve_provider_secrets(&mut projects)?;
        validate_projects(&projects)?;

        let ec_private_pem = load_signing_pem()?;
        let signing = SigningConfig {
            ec_private_pem,
            key_id: env_or("AUTH_SIGNING_KID", "shared-auth-v1"),
            issuer: env_or("AUTH_ISSUER", "https://auth.oresoftware.dev"),
            audience: env_or("AUTH_AUDIENCE", "oresoftware"),
            ttl_secs: env_or("AUTH_ACCESS_TOKEN_TTL_SECS", "900")
                .parse()
                .map_err(|_| ConfigError::Invalid("AUTH_ACCESS_TOKEN_TTL_SECS"))?,
        };
        if !(60..=86_400).contains(&signing.ttl_secs) {
            return Err(ConfigError::Invalid(
                "AUTH_ACCESS_TOKEN_TTL_SECS must be between 60 and 86400",
            ));
        }

        let db = match std::env::var("AUTH_DATABASE_URL")
            .ok()
            .filter(|s| !s.is_empty())
        {
            Some(url) => Some(DbConfig {
                url,
                max_connections: env_or("AUTH_DB_MAX_CONNECTIONS", "5")
                    .parse()
                    .map_err(|_| ConfigError::Invalid("AUTH_DB_MAX_CONNECTIONS"))?,
            }),
            None if parse_bool("AUTH_ALLOW_DBLESS", false)? => None,
            None => return Err(ConfigError::Missing("AUTH_DATABASE_URL")),
        };

        let redis = std::env::var("AUTH_REDIS_URL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|url| RedisConfig {
                url,
                key_prefix: env_or("AUTH_REDIS_KEY_PREFIX", "shared-auth:v1"),
            });

        let refresh_ttl_secs = env_or("AUTH_REFRESH_TOKEN_TTL_SECS", "2592000")
            .parse()
            .map_err(|_| ConfigError::Invalid("AUTH_REFRESH_TOKEN_TTL_SECS"))?;
        if !(300..=31_536_000).contains(&refresh_ttl_secs) {
            return Err(ConfigError::Invalid(
                "AUTH_REFRESH_TOKEN_TTL_SECS must be between 300 and 31536000",
            ));
        }
        let sessions = SessionConfig {
            refresh_ttl_secs,
            allow_registration: parse_bool("AUTH_ALLOW_REGISTRATION", false)?,
        };

        let magic_link_ttl_secs = env_or("AUTH_MAGIC_LINK_TTL_SECS", "900")
            .parse()
            .map_err(|_| ConfigError::Invalid("AUTH_MAGIC_LINK_TTL_SECS"))?;
        if !(300..=3_600).contains(&magic_link_ttl_secs) {
            return Err(ConfigError::Invalid(
                "AUTH_MAGIC_LINK_TTL_SECS must be between 300 and 3600",
            ));
        }
        let magic_link_base_url = optional_env("AUTH_MAGIC_LINK_BASE_URL");
        if let Some(value) = magic_link_base_url.as_deref() {
            validate_magic_link_base_url(value)?;
        }
        let from_email = optional_env("AUTH_EMAIL_FROM");
        if from_email
            .as_deref()
            .is_some_and(|value| !looks_like_email(value))
        {
            return Err(ConfigError::Invalid("AUTH_EMAIL_FROM"));
        }
        let otp_pepper = optional_env("AUTH_OTP_PEPPER");
        if otp_pepper.as_ref().is_some_and(|value| value.len() < 32) {
            return Err(ConfigError::Invalid(
                "AUTH_OTP_PEPPER must contain at least 32 bytes",
            ));
        }
        let magic_links = MagicLinkConfig {
            sendgrid_api_key: optional_env("AUTH_SENDGRID_API_KEY"),
            otp_pepper,
            from_email,
            from_name: env_or("AUTH_EMAIL_FROM_NAME", "OreSoftware"),
            link_base_url: magic_link_base_url,
            ttl_secs: magic_link_ttl_secs,
            allow_signup: parse_bool("AUTH_MAGIC_LINK_ALLOW_SIGNUP", false)?,
        };
        let twilio_verify = TwilioVerifyConfig {
            account_sid: optional_env("AUTH_TWILIO_ACCOUNT_SID"),
            auth_token: optional_env("AUTH_TWILIO_AUTH_TOKEN"),
            service_sid: optional_env("AUTH_TWILIO_VERIFY_SERVICE_SID"),
        };
        validate_twilio_identifier(
            twilio_verify.account_sid.as_deref(),
            "AUTH_TWILIO_ACCOUNT_SID",
            "AC",
        )?;
        validate_twilio_identifier(
            twilio_verify.service_sid.as_deref(),
            "AUTH_TWILIO_VERIFY_SERVICE_SID",
            "VA",
        )?;

        let webhook_secret = std::env::var("AUTH_WEBHOOK_SECRET")
            .ok()
            .filter(|value| !value.is_empty());
        if webhook_secret
            .as_ref()
            .is_some_and(|secret| secret.len() < 32)
        {
            return Err(ConfigError::Invalid(
                "AUTH_WEBHOOK_SECRET must contain at least 32 bytes",
            ));
        }

        let introspect_secret = std::env::var("AUTH_INTROSPECT_SECRET")
            .ok()
            .filter(|value| !value.is_empty());
        if introspect_secret
            .as_ref()
            .is_some_and(|secret| secret.len() < 32)
        {
            return Err(ConfigError::Invalid(
                "AUTH_INTROSPECT_SECRET must contain at least 32 bytes",
            ));
        }

        let cors_allow_origins = env_or("AUTH_CORS_ALLOW_ORIGINS", "")
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();

        Ok(Self {
            bind_addr,
            projects,
            signing,
            db,
            redis,
            sessions,
            magic_links,
            twilio_verify,
            webhook_secret,
            introspect_secret,
            cors_allow_origins,
        })
    }
}

fn optional_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn validate_magic_link_base_url(value: &str) -> Result<(), ConfigError> {
    let parsed =
        reqwest::Url::parse(value).map_err(|_| ConfigError::Invalid("AUTH_MAGIC_LINK_BASE_URL"))?;
    let loopback_http = parsed.scheme() == "http"
        && parsed
            .host_str()
            .is_some_and(|host| matches!(host, "localhost" | "127.0.0.1" | "::1"));
    if parsed.scheme() != "https" && parsed.scheme() != "sonusauris" && !loopback_http {
        return Err(ConfigError::Invalid(
            "AUTH_MAGIC_LINK_BASE_URL must use HTTPS, sonusauris, or loopback HTTP",
        ));
    }
    if parsed.username() != ""
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(ConfigError::Invalid(
            "AUTH_MAGIC_LINK_BASE_URL must not contain credentials, query, or fragment",
        ));
    }
    Ok(())
}

fn looks_like_email(value: &str) -> bool {
    let mut parts = value.split('@');
    let local = parts.next().unwrap_or_default();
    let domain = parts.next().unwrap_or_default();
    !local.is_empty()
        && domain.contains('.')
        && parts.next().is_none()
        && value.len() <= 320
        && !value.chars().any(char::is_whitespace)
}

fn validate_twilio_identifier(
    value: Option<&str>,
    key: &'static str,
    prefix: &str,
) -> Result<(), ConfigError> {
    if value.is_some_and(|value| {
        value.len() != 34
            || !value.starts_with(prefix)
            || !value.bytes().all(|byte| byte.is_ascii_alphanumeric())
    }) {
        return Err(ConfigError::Invalid(key));
    }
    Ok(())
}

fn validate_projects(projects: &[SupabaseProject]) -> Result<(), ConfigError> {
    let mut names = std::collections::HashSet::new();
    let mut issuers = std::collections::HashSet::new();
    for project in projects {
        if project.name.is_empty()
            || project.name.len() > 64
            || !project
                .name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(ConfigError::Invalid("invalid Supabase project name"));
        }
        if !names.insert(project.name.clone()) || !issuers.insert(project.issuer()) {
            return Err(ConfigError::Invalid("duplicate Supabase project or issuer"));
        }
        if project
            .hs256_secret
            .as_ref()
            .is_some_and(|secret| secret.len() < 32)
        {
            return Err(ConfigError::Invalid(
                "Supabase HS256 secrets must contain at least 32 bytes",
            ));
        }
    }
    Ok(())
}

fn resolve_provider_secrets(projects: &mut [SupabaseProject]) -> Result<(), ConfigError> {
    for project in projects {
        project.api_keys = SupabaseApiKeys {
            publishable_key: read_referenced_secret(&project.publishable_key_env)?,
            secret_key: read_referenced_secret(&project.secret_key_env)?,
            service_role_key: read_referenced_secret(&project.service_role_key_env)?,
        };
        project.hs256_secret = read_referenced_secret(&project.jwt_secret_env)?;
    }
    Ok(())
}

fn read_referenced_secret(env_name: &Option<String>) -> Result<Option<String>, ConfigError> {
    let Some(env_name) = env_name.as_deref() else {
        return Ok(None);
    };
    if env_name.is_empty()
        || env_name.len() > 128
        || !env_name.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_uppercase() || byte == b'_' || (index > 0 && byte.is_ascii_digit())
        })
    {
        return Err(ConfigError::Invalid(
            "Supabase credential env names must use uppercase A-Z, digits, and underscores",
        ));
    }
    std::env::var(env_name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(Some)
        .ok_or(ConfigError::Invalid(
            "referenced Supabase credential env var is missing or empty",
        ))
}

fn parse_bool(key: &'static str, default: bool) -> Result<bool, ConfigError> {
    match std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .as_deref()
    {
        None => Ok(default),
        Some("1" | "true" | "TRUE" | "yes" | "YES") => Ok(true),
        Some("0" | "false" | "FALSE" | "no" | "NO") => Ok(false),
        Some(_) => Err(ConfigError::Invalid(key)),
    }
}

/// Load the EC signing PEM from `AUTH_SIGNING_KEY_PEM` (inline) or
/// `AUTH_SIGNING_KEY_FILE` (path).
fn load_signing_pem() -> Result<String, ConfigError> {
    if let Ok(inline) = std::env::var("AUTH_SIGNING_KEY_PEM") {
        if !inline.trim().is_empty() {
            return Ok(inline);
        }
    }
    if let Ok(path) = std::env::var("AUTH_SIGNING_KEY_FILE") {
        return std::fs::read_to_string(&path)
            .map_err(|_| ConfigError::Invalid("AUTH_SIGNING_KEY_FILE unreadable"));
    }
    Err(ConfigError::Missing(
        "AUTH_SIGNING_KEY_PEM or AUTH_SIGNING_KEY_FILE",
    ))
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issuer_and_jwks_derive_from_project_ref() {
        let p = SupabaseProject {
            name: "fiducia-cloud".into(),
            project_ref: "abcref".into(),
            issuer: None,
            jwks_url: None,
            audience: "authenticated".into(),
            publishable_key_env: None,
            secret_key_env: None,
            service_role_key_env: None,
            jwt_secret_env: None,
            api_keys: SupabaseApiKeys::default(),
            hs256_secret: None,
        };
        assert_eq!(p.issuer(), "https://abcref.supabase.co/auth/v1");
        assert_eq!(
            p.jwks_url(),
            "https://abcref.supabase.co/auth/v1/.well-known/jwks.json"
        );
    }

    #[test]
    fn explicit_issuer_and_jwks_override_derivation() {
        let p = SupabaseProject {
            name: "x".into(),
            project_ref: "r".into(),
            issuer: Some("https://custom.example/iss".into()),
            jwks_url: Some("https://custom.example/keys".into()),
            audience: "authenticated".into(),
            publishable_key_env: None,
            secret_key_env: None,
            service_role_key_env: None,
            jwt_secret_env: None,
            api_keys: SupabaseApiKeys::default(),
            hs256_secret: None,
        };
        assert_eq!(p.issuer(), "https://custom.example/iss");
        assert_eq!(p.jwks_url(), "https://custom.example/keys");
    }

    #[test]
    fn projects_json_parses_with_defaults() {
        let v: Vec<SupabaseProject> = serde_json::from_str(
            r#"[{"name":"3fa-app","project_ref":"ref1"},
                {"name":"sonus-auris","project_ref":"ref2","audience":"custom",
                 "publishable_key_env":"AUTH_SUPABASE_SONUS_PUBLISHABLE_KEY",
                 "jwt_secret_env":"AUTH_SUPABASE_SONUS_JWT_SECRET"}]"#,
        )
        .unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].audience, "authenticated"); // default
        assert!(v[0].hs256_secret.is_none());
        assert_eq!(v[1].audience, "custom");
        assert_eq!(
            v[1].publishable_key_env.as_deref(),
            Some("AUTH_SUPABASE_SONUS_PUBLISHABLE_KEY")
        );
        assert_eq!(
            v[1].jwt_secret_env.as_deref(),
            Some("AUTH_SUPABASE_SONUS_JWT_SECRET")
        );
        assert!(v[1].hs256_secret.is_none());
    }

    #[test]
    fn inline_provider_secrets_are_rejected() {
        let result = serde_json::from_str::<SupabaseProject>(
            r#"{"name":"x","project_ref":"ref","hs256_secret":"must-not-be-inline"}"#,
        );
        assert!(result.is_err());
    }
}
