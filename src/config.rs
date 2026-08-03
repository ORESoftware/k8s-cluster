//! Runtime configuration for Shared Auth.
//!
//! See `.env.example` for the environment contract. Supabase projects are a
//! registry: each upstream issuer/audience/key-set is configured explicitly and
//! unified tokens are signed by this service.

use std::{env, fs, net::SocketAddr, path::PathBuf, str::FromStr, time::Duration};

use serde::Deserialize;

use crate::error::AuthError;

const DEFAULT_BIND_ADDR: &str = "0.0.0.0:8080";
const DEFAULT_ISSUER: &str = "https://auth.oresoftware.com";
const DEFAULT_AUDIENCE: &str = "oresoftware";
const DEFAULT_ACCESS_TTL_SECS: u64 = 900;
const DEFAULT_REFRESH_TTL_SECS: u64 = 30 * 24 * 60 * 60;
const DEFAULT_MAGIC_LINK_TTL_SECS: u64 = 15 * 60;
const DEFAULT_DB_MAX_CONNECTIONS: u32 = 10;
const DEFAULT_REDIS_KEY_PREFIX: &str = "shared-auth";
const DEFAULT_FROM_NAME: &str = "OreSoftware";
const DEFAULT_MAX_BODY_BYTES: usize = 64 * 1024;
const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 15;
const DEFAULT_WEBAUTHN_RP_NAME: &str = "OreSoftware";

/// Fully pinned external identity provider configuration.
#[derive(Clone, Debug, Deserialize)]
pub struct SupabaseProject {
    /// Stable local identifier, e.g. `fiducia-customer`.
    pub project_ref: String,
    /// Expected `iss`. Defaults to `https://<project_ref>.supabase.co/auth/v1`.
    #[serde(default)]
    pub issuer: Option<String>,
    /// Expected `aud`. Supabase commonly uses `authenticated`.
    #[serde(default = "default_supabase_audience")]
    pub audience: String,
    /// Public JWKS URL. Defaults to `<issuer>/.well-known/jwks.json`.
    #[serde(default)]
    pub jwks_url: Option<String>,
    /// Optional publishable/anon key for the direct `/auth/v1/user` fallback.
    #[serde(default)]
    pub publishable_key: Option<String>,
    /// Whether this provider is allowed to create a local principal when the
    /// provider subject has never been seen before.
    #[serde(default)]
    pub allow_signup: bool,
}

fn default_supabase_audience() -> String {
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

fn configured(value: Option<&str>) -> bool {
    value.is_some_and(|value| !value.trim().is_empty())
}

impl MagicLinkConfig {
    pub fn is_enabled(&self) -> bool {
        configured(self.sendgrid_api_key.as_deref())
            && configured(self.otp_pepper.as_deref())
            && configured(self.from_email.as_deref())
            && configured(self.link_base_url.as_deref())
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
        configured(self.account_sid.as_deref())
            && configured(self.auth_token.as_deref())
            && configured(self.service_sid.as_deref())
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
    /// disabled and returns `401`.
    pub webhook_secret: Option<String>,
    /// HMAC secret for authenticated service-to-service introspection.
    pub introspect_secret: Option<String>,
    /// Explicit local-development escape hatch for anonymous introspection.
    pub allow_unauthenticated_introspection: bool,
    /// Master key for encrypting MFA factor secrets at rest (AES-256-GCM).
    pub factor_encryption_key: Option<[u8; 32]>,
    /// WebAuthn relying-party configuration. `None` leaves passkeys unavailable.
    pub webauthn: Option<WebauthnConfig>,
    pub max_body_bytes: usize,
    pub request_timeout: Duration,
}

#[derive(Clone)]
pub struct WebauthnConfig {
    pub rp_id: String,
    pub rp_origin: String,
    pub rp_name: String,
}

impl AppConfig {
    pub fn from_env() -> Result<Self, AuthError> {
        let bind_addr = env::var("AUTH_BIND_ADDR")
            .unwrap_or_else(|_| DEFAULT_BIND_ADDR.to_string())
            .parse::<SocketAddr>()
            .map_err(|_| AuthError::Configuration("AUTH_BIND_ADDR is invalid".into()))?;

        let projects = load_projects()?;
        if projects.is_empty() {
            return Err(AuthError::Configuration(
                "at least one Supabase project is required".into(),
            ));
        }

        let signing_key_path = required_env("AUTH_SIGNING_KEY_FILE")?;
        let ec_private_pem = fs::read_to_string(&signing_key_path).map_err(|error| {
            AuthError::Configuration(format!(
                "failed to read AUTH_SIGNING_KEY_FILE {signing_key_path:?}: {error}"
            ))
        })?;
        let signing = SigningConfig {
            ec_private_pem,
            key_id: env::var("AUTH_SIGNING_KEY_ID").unwrap_or_else(|_| "auth-1".into()),
            issuer: env::var("AUTH_ISSUER").unwrap_or_else(|_| DEFAULT_ISSUER.into()),
            audience: env::var("AUTH_AUDIENCE").unwrap_or_else(|_| DEFAULT_AUDIENCE.into()),
            ttl_secs: env_u64("AUTH_ACCESS_TTL_SECS", DEFAULT_ACCESS_TTL_SECS)?,
        };

        let db = optional_env("AUTH_DATABASE_URL").map(|url| DbConfig {
            url,
            max_connections: env_u32("AUTH_DB_MAX_CONNECTIONS", DEFAULT_DB_MAX_CONNECTIONS)
                .unwrap_or(DEFAULT_DB_MAX_CONNECTIONS),
        });
        let redis = optional_env("AUTH_REDIS_URL").map(|url| RedisConfig {
            url,
            key_prefix: env::var("AUTH_REDIS_KEY_PREFIX")
                .unwrap_or_else(|_| DEFAULT_REDIS_KEY_PREFIX.into()),
        });
        let sessions = SessionConfig {
            refresh_ttl_secs: env_u64("AUTH_REFRESH_TTL_SECS", DEFAULT_REFRESH_TTL_SECS)?,
            allow_registration: env_bool("AUTH_ALLOW_REGISTRATION", false),
        };
        let magic_links = MagicLinkConfig {
            sendgrid_api_key: optional_env("AUTH_SENDGRID_API_KEY"),
            otp_pepper: optional_env("AUTH_OTP_PEPPER"),
            from_email: optional_env("AUTH_EMAIL_FROM"),
            from_name: env::var("AUTH_EMAIL_FROM_NAME")
                .unwrap_or_else(|_| DEFAULT_FROM_NAME.into()),
            link_base_url: optional_env("AUTH_MAGIC_LINK_BASE_URL"),
            ttl_secs: env_u64("AUTH_MAGIC_LINK_TTL_SECS", DEFAULT_MAGIC_LINK_TTL_SECS)?,
            allow_signup: env_bool("AUTH_MAGIC_LINK_ALLOW_SIGNUP", false),
        };
        let twilio_verify = TwilioVerifyConfig {
            account_sid: optional_env("AUTH_TWILIO_ACCOUNT_SID"),
            auth_token: optional_env("AUTH_TWILIO_AUTH_TOKEN"),
            service_sid: optional_env("AUTH_TWILIO_VERIFY_SERVICE_SID"),
        };
        let webhook_secret = optional_env("AUTH_WEBHOOK_SECRET");
        let introspect_secret = optional_env("AUTH_INTROSPECT_SECRET");
        let allow_unauthenticated_introspection =
            env_bool("AUTH_ALLOW_UNAUTHENTICATED_INTROSPECTION", false);
        let factor_encryption_key = parse_factor_encryption_key()?;
        let webauthn = load_webauthn()?;
        let max_body_bytes = env_usize("AUTH_MAX_BODY_BYTES", DEFAULT_MAX_BODY_BYTES)?;
        let request_timeout = Duration::from_secs(env_u64(
            "AUTH_REQUEST_TIMEOUT_SECS",
            DEFAULT_REQUEST_TIMEOUT_SECS,
        )?);

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
            allow_unauthenticated_introspection,
            factor_encryption_key,
            webauthn,
            max_body_bytes,
            request_timeout,
        })
    }
}

fn load_projects() -> Result<Vec<SupabaseProject>, AuthError> {
    if let Ok(path) = env::var("AUTH_SUPABASE_PROJECTS_FILE") {
        let content = fs::read_to_string(&path).map_err(|error| {
            AuthError::Configuration(format!(
                "failed to read AUTH_SUPABASE_PROJECTS_FILE {path:?}: {error}"
            ))
        })?;
        return parse_projects(&content);
    }
    let inline = required_env("AUTH_SUPABASE_PROJECTS")?;
    parse_projects(&inline)
}

fn parse_projects(value: &str) -> Result<Vec<SupabaseProject>, AuthError> {
    let projects = serde_json::from_str::<Vec<SupabaseProject>>(value)
        .map_err(|error| AuthError::Configuration(format!("invalid Supabase projects JSON: {error}")))?;
    validate_projects(&projects)?;
    Ok(projects)
}

fn validate_projects(projects: &[SupabaseProject]) -> Result<(), AuthError> {
    for (index, project) in projects.iter().enumerate() {
        if project.project_ref.trim().is_empty() {
            return Err(AuthError::Configuration(format!(
                "Supabase project at index {index} has an empty project_ref"
            )));
        }
        if project.audience.trim().is_empty() {
            return Err(AuthError::Configuration(format!(
                "Supabase project {} has an empty audience",
                project.project_ref
            )));
        }
        if project.issuer().trim().is_empty() {
            return Err(AuthError::Configuration(format!(
                "Supabase project {} has an empty issuer",
                project.project_ref
            )));
        }
        if project.jwks_url().trim().is_empty() {
            return Err(AuthError::Configuration(format!(
                "Supabase project {} has an empty JWKS URL",
                project.project_ref
            )));
        }
        for earlier in &projects[..index] {
            if earlier.project_ref == project.project_ref {
                return Err(AuthError::Configuration(format!(
                    "duplicate Supabase project_ref {}",
                    project.project_ref
                )));
            }
            if earlier.issuer() == project.issuer() {
                return Err(AuthError::Configuration(format!(
                    "duplicate Supabase issuer {}",
                    project.issuer()
                )));
            }
        }
    }
    Ok(())
}

fn parse_factor_encryption_key() -> Result<Option<[u8; 32]>, AuthError> {
    let Some(hex_value) = optional_env("AUTH_FACTOR_ENCRYPTION_KEY_HEX") else {
        return Ok(None);
    };
    if hex_value.len() != 64 {
        return Err(AuthError::Configuration(
            "AUTH_FACTOR_ENCRYPTION_KEY_HEX must be 64 hexadecimal characters".into(),
        ));
    }
    let mut key = [0_u8; 32];
    for (index, chunk) in hex_value.as_bytes().chunks_exact(2).enumerate() {
        let encoded = std::str::from_utf8(chunk).map_err(|_| {
            AuthError::Configuration(
                "AUTH_FACTOR_ENCRYPTION_KEY_HEX must contain only hexadecimal characters".into(),
            )
        })?;
        key[index] = u8::from_str_radix(encoded, 16).map_err(|_| {
            AuthError::Configuration(
                "AUTH_FACTOR_ENCRYPTION_KEY_HEX must contain only hexadecimal characters".into(),
            )
        })?;
    }
    Ok(Some(key))
}

fn load_webauthn() -> Result<Option<WebauthnConfig>, AuthError> {
    let rp_id = optional_env("AUTH_WEBAUTHN_RP_ID");
    let rp_origin = optional_env("AUTH_WEBAUTHN_RP_ORIGIN");
    match (rp_id, rp_origin) {
        (None, None) => Ok(None),
        (Some(rp_id), Some(rp_origin)) => {
            let parsed = reqwest::Url::parse(&rp_origin).map_err(|_| {
                AuthError::Configuration("AUTH_WEBAUTHN_RP_ORIGIN is invalid".into())
            })?;
            if !matches!(parsed.scheme(), "http" | "https") {
                return Err(AuthError::Configuration(
                    "AUTH_WEBAUTHN_RP_ORIGIN must use http or https".into(),
                ));
            }
            Ok(Some(WebauthnConfig {
                rp_id,
                rp_origin,
                rp_name: env::var("AUTH_WEBAUTHN_RP_NAME")
                    .unwrap_or_else(|_| DEFAULT_WEBAUTHN_RP_NAME.into()),
            }))
        }
        _ => Err(AuthError::Configuration(
            "AUTH_WEBAUTHN_RP_ID and AUTH_WEBAUTHN_RP_ORIGIN must be set together".into(),
        )),
    }
}

fn required_env(name: &str) -> Result<String, AuthError> {
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AuthError::Configuration(format!("{name} is required")))
}

fn optional_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_bool(name: &str, default: bool) -> bool {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .map(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(default)
}

fn env_u64(name: &str, default: u64) -> Result<u64, AuthError> {
    env::var(name)
        .ok()
        .map(|value| {
            u64::from_str(value.trim()).map_err(|_| {
                AuthError::Configuration(format!("{name} must be an unsigned integer"))
            })
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn env_u32(name: &str, default: u32) -> Result<u32, AuthError> {
    env::var(name)
        .ok()
        .map(|value| {
            u32::from_str(value.trim()).map_err(|_| {
                AuthError::Configuration(format!("{name} must be an unsigned integer"))
            })
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn env_usize(name: &str, default: usize) -> Result<usize, AuthError> {
    env::var(name)
        .ok()
        .map(|value| {
            usize::from_str(value.trim()).map_err(|_| {
                AuthError::Configuration(format!("{name} must be an unsigned integer"))
            })
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

pub fn signing_key_path_from_env() -> Option<PathBuf> {
    env::var("AUTH_SIGNING_KEY_FILE").ok().map(PathBuf::from)
}
