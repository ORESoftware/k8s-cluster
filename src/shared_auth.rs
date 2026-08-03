//! Shared Auth authentication client for the billing control plane.
//!
//! Quaestor intentionally uses Shared Auth's revocation-aware
//! `POST /auth/introspect` endpoint rather than re-implementing provider JWT
//! verification. Shared Auth establishes identity and authentication assurance;
//! Quaestor separately resolves tenant membership and financial scopes from its
//! own database.

use std::env;
use std::fmt;
use std::net::IpAddr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::http::HeaderValue;
use reqwest::{redirect::Policy, Client, StatusCode, Url};
use serde::Deserialize;

pub const ACR_LOA1: &str = "urn:oresoftware:loa:1";
pub const ACR_LOA2: &str = "urn:oresoftware:loa:2";
const MAX_TOKEN_BYTES: usize = 16 * 1024;
const MAX_RESPONSE_BYTES: usize = 64 * 1024;
const CLOCK_SKEW_SECONDS: u64 = 30;

#[derive(Clone)]
pub struct SharedAuthConfig {
    pub base_url: String,
    pub introspect_secret: String,
    pub issuer: String,
    pub audience: String,
    pub request_timeout: Duration,
    pub require_session_id: bool,
}

impl fmt::Debug for SharedAuthConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SharedAuthConfig")
            .field("base_url", &self.base_url)
            .field("introspect_secret", &"<redacted>")
            .field("issuer", &self.issuer)
            .field("audience", &self.audience)
            .field("request_timeout", &self.request_timeout)
            .field("require_session_id", &self.require_session_id)
            .finish()
    }
}

impl SharedAuthConfig {
    /// Load Shared Auth configuration. `Ok(None)` means fully unconfigured;
    /// partially configured state is always an error so production cannot
    /// silently fall back to a weaker verifier.
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        let base_url = optional_env("BILLING_SHARED_AUTH_BASE_URL")
            .or_else(|| optional_env("SHARED_AUTH_BASE_URL"));
        let introspect_secret = optional_env("BILLING_SHARED_AUTH_INTROSPECT_SECRET")
            .or_else(|| optional_env("AUTH_INTROSPECT_SECRET"));

        if base_url.is_none() && introspect_secret.is_none() {
            return Ok(None);
        }
        let base_url = base_url.ok_or_else(|| {
            anyhow::anyhow!(
                "BILLING_SHARED_AUTH_BASE_URL is required when Shared Auth is configured"
            )
        })?;
        let introspect_secret = introspect_secret.ok_or_else(|| {
            anyhow::anyhow!(
                "BILLING_SHARED_AUTH_INTROSPECT_SECRET/AUTH_INTROSPECT_SECRET is required; \
                 Quaestor never calls an unauthenticated introspection endpoint"
            )
        })?;

        let parsed = Url::parse(&base_url)
            .map_err(|error| anyhow::anyhow!("invalid BILLING_SHARED_AUTH_BASE_URL: {error}"))?;
        if parsed.username() != ""
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || !matches!(parsed.path(), "" | "/")
        {
            anyhow::bail!(
                "BILLING_SHARED_AUTH_BASE_URL must be an origin without userinfo, path, query, or fragment"
            );
        }
        let allow_http = env_bool("BILLING_SHARED_AUTH_ALLOW_HTTP", false);
        match parsed.scheme() {
            "https" => {}
            "http" if allow_http && protected_http_host(&parsed) => {}
            "http" if allow_http => {
                anyhow::bail!(
                    "plain-http Shared Auth is limited to loopback or in-cluster service hosts"
                );
            }
            _ => {
                anyhow::bail!(
                    "BILLING_SHARED_AUTH_BASE_URL must use https; set \
                     BILLING_SHARED_AUTH_ALLOW_HTTP=true only for a protected local/in-cluster hop"
                );
            }
        }

        let allow_insecure_dev = env_bool("BILLING_ALLOW_INSECURE_DEV", false);
        if !allow_insecure_dev && introspect_secret.len() < 32 {
            anyhow::bail!(
                "BILLING_SHARED_AUTH_INTROSPECT_SECRET must contain at least 32 characters"
            );
        }

        let timeout_ms = env::var("BILLING_SHARED_AUTH_TIMEOUT_MS")
            .ok()
            .and_then(|raw| raw.parse::<u64>().ok())
            .unwrap_or(1_500)
            .clamp(100, 5_000);

        Ok(Some(Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            introspect_secret,
            issuer: optional_env("BILLING_SHARED_AUTH_ISSUER")
                .unwrap_or_else(|| "https://auth.oresoftware.dev".to_owned()),
            audience: optional_env("BILLING_SHARED_AUTH_AUDIENCE")
                .unwrap_or_else(|| "oresoftware".to_owned()),
            request_timeout: Duration::from_millis(timeout_ms),
            require_session_id: env_bool("BILLING_SHARED_AUTH_REQUIRE_SESSION_ID", true),
        }))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Aal {
    Aal1,
    Aal2,
}

impl Aal {
    pub fn is_aal2(self) -> bool {
        matches!(self, Self::Aal2)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedAuthIdentity {
    pub subject: String,
    pub provider: String,
    pub provider_tenant: String,
    pub provider_subject: String,
    pub session_id: Option<String>,
    pub email: Option<String>,
    pub email_verified: bool,
    pub roles: Vec<String>,
    pub assurance: Aal,
    pub amr: Vec<String>,
    pub acr: Option<String>,
    /// Compatibility field used by the existing authorization layer. For LOA2
    /// this is Shared Auth's signed `auth_time` (the actual factor ceremony),
    /// never the access-token `iat`. For LOA1 it falls back to token issuance.
    pub issued_at: u64,
    pub expires_at: u64,
}

impl SharedAuthIdentity {
    pub fn step_up_age_secs(&self, now: u64) -> Option<u64> {
        self.assurance
            .is_aal2()
            .then(|| now.saturating_sub(self.issued_at))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("unauthorized")]
    Unauthorized,
    #[error("shared-auth unavailable: {0}")]
    Unavailable(String),
}

#[derive(Clone)]
pub struct SharedAuthVerifier {
    config: SharedAuthConfig,
    http: Client,
}

impl fmt::Debug for SharedAuthVerifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SharedAuthVerifier")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl SharedAuthVerifier {
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        let Some(config) = SharedAuthConfig::from_env()? else {
            return Ok(None);
        };
        let http = Client::builder()
            .redirect(Policy::none())
            .connect_timeout(config.request_timeout)
            .timeout(config.request_timeout)
            .user_agent(concat!("quaestor-ledger/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| anyhow::anyhow!("building Shared Auth client: {error}"))?;
        Ok(Some(Self { config, http }))
    }

    pub async fn verify(&self, token: &str) -> Result<SharedAuthIdentity, AuthError> {
        if token.is_empty() || token.len() > MAX_TOKEN_BYTES {
            return Err(AuthError::Unauthorized);
        }
        let url = format!("{}/auth/introspect", self.config.base_url);
        let response = self
            .http
            .post(url)
            .bearer_auth(&self.config.introspect_secret)
            .json(&serde_json::json!({ "token": token }))
            .send()
            .await
            .map_err(|error| AuthError::Unavailable(error.to_string()))?;

        if response.status() == StatusCode::UNAUTHORIZED {
            // The user's token is in the JSON body; a 401 here means *our*
            // introspection service credential is wrong.
            return Err(AuthError::Unavailable(
                "introspection service credential was rejected".to_owned(),
            ));
        }
        if !response.status().is_success() {
            return Err(AuthError::Unavailable(format!(
                "introspection returned {}",
                response.status()
            )));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
        {
            return Err(AuthError::Unavailable(
                "introspection response exceeded size limit".to_owned(),
            ));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|error| AuthError::Unavailable(error.to_string()))?;
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(AuthError::Unavailable(
                "introspection response exceeded size limit".to_owned(),
            ));
        }
        let claims: Introspection = serde_json::from_slice(&bytes).map_err(|error| {
            AuthError::Unavailable(format!("invalid introspection JSON: {error}"))
        })?;
        self.identity_from_claims(claims)
    }

    fn identity_from_claims(&self, claims: Introspection) -> Result<SharedAuthIdentity, AuthError> {
        if !claims.active {
            return Err(AuthError::Unauthorized);
        }
        if claims.iss.as_deref() != Some(self.config.issuer.as_str())
            || claims.aud.as_deref() != Some(self.config.audience.as_str())
        {
            return Err(AuthError::Unavailable(
                "introspection returned an unexpected issuer or audience".to_owned(),
            ));
        }

        let now = now_seconds();
        let token_issued_at = claims.iat.ok_or_else(|| {
            AuthError::Unavailable("active token omitted required iat".to_owned())
        })?;
        let expires_at = claims.exp.ok_or_else(|| {
            AuthError::Unavailable("active token omitted required exp".to_owned())
        })?;
        if expires_at.saturating_add(CLOCK_SKEW_SECONDS) < now
            || token_issued_at > now.saturating_add(CLOCK_SKEW_SECONDS)
        {
            return Err(AuthError::Unauthorized);
        }

        let subject = bounded_identifier(claims.sub, 200)?;
        let provider = bounded_text(claims.provider, 64)?;
        let provider_tenant = bounded_text(claims.provider_tenant, 200)?;
        let provider_subject = bounded_text(claims.provider_subject, 512)?;
        let session_id = claims
            .sid
            .map(|value| bounded_identifier(Some(value), 200))
            .transpose()?;
        if self.config.require_session_id && session_id.is_none() {
            return Err(AuthError::Unauthorized);
        }

        let mut roles = normalize_tokens(claims.roles, 64, 64)?;
        roles.sort_unstable();
        roles.dedup();
        let mut amr = normalize_tokens(claims.amr, 16, 64)?;
        amr.sort_unstable();
        amr.dedup();

        let acr = claims
            .acr
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty() && value.len() <= 255);
        let assurance = match (claims.aal.unwrap_or(1), acr.as_deref()) {
            (level, Some(ACR_LOA2)) if level >= 2 => Aal::Aal2,
            (level, Some(ACR_LOA1)) if level <= 1 => Aal::Aal1,
            (1, None) => Aal::Aal1,
            // Contradictory signed assurance is an authority failure. Do not
            // silently reinterpret it as a lower or higher level.
            _ => {
                return Err(AuthError::Unavailable(
                    "introspection returned contradictory AAL/ACR claims".to_owned(),
                ));
            }
        };

        let auth_time = claims.auth_time;
        if assurance.is_aal2() && auth_time.is_none() {
            return Err(AuthError::Unavailable(
                "active LOA2 token omitted authoritative auth_time".to_owned(),
            ));
        }
        if auth_time.is_some_and(|value| {
            value > now.saturating_add(CLOCK_SKEW_SECONDS)
                || value > token_issued_at.saturating_add(CLOCK_SKEW_SECONDS)
        }) {
            return Err(AuthError::Unauthorized);
        }

        let email = claims
            .email
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty() && value.len() <= 320);

        Ok(SharedAuthIdentity {
            subject,
            provider,
            provider_tenant,
            provider_subject,
            session_id,
            email,
            email_verified: claims.email_verified,
            roles,
            assurance,
            amr,
            acr,
            issued_at: auth_time.unwrap_or(token_issued_at),
            expires_at,
        })
    }
}

#[derive(Debug, Deserialize)]
struct Introspection {
    active: bool,
    #[serde(default)]
    sub: Option<String>,
    #[serde(default)]
    iss: Option<String>,
    #[serde(default)]
    aud: Option<String>,
    #[serde(default)]
    exp: Option<u64>,
    #[serde(default)]
    iat: Option<u64>,
    #[serde(default)]
    auth_time: Option<u64>,
    #[serde(default)]
    sid: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    provider_tenant: Option<String>,
    #[serde(default)]
    provider_subject: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    email_verified: bool,
    #[serde(default)]
    roles: Vec<String>,
    #[serde(default)]
    aal: Option<u8>,
    #[serde(default)]
    amr: Vec<String>,
    #[serde(default)]
    acr: Option<String>,
}

fn bounded_identifier(value: Option<String>, max: usize) -> Result<String, AuthError> {
    let value = value.unwrap_or_default();
    let value = value.trim();
    if value.is_empty()
        || value.len() > max
        || value
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\'))
    {
        return Err(AuthError::Unauthorized);
    }
    Ok(value.to_owned())
}

fn bounded_text(value: Option<String>, max: usize) -> Result<String, AuthError> {
    let value = value.unwrap_or_default();
    let value = value.trim();
    if value.is_empty()
        || value.len() > max
        || value.chars().any(|character| character.is_control())
    {
        return Err(AuthError::Unauthorized);
    }
    Ok(value.to_owned())
}

fn normalize_tokens(
    values: Vec<String>,
    max_items: usize,
    max_len: usize,
) -> Result<Vec<String>, AuthError> {
    if values.len() > max_items {
        return Err(AuthError::Unauthorized);
    }
    values
        .into_iter()
        .map(|value| {
            let value = value.trim().to_ascii_lowercase();
            if value.is_empty()
                || value.len() > max_len
                || !value.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'_' | b':' | b'-' | b'.')
                })
            {
                Err(AuthError::Unauthorized)
            } else {
                Ok(value)
            }
        })
        .collect()
}

/// Parse an RFC 7235 bearer credential. The scheme is case-insensitive; the
/// credential itself is not. Multiple whitespace-separated credentials and
/// control characters are rejected.
pub fn bearer_token(raw: Option<&str>) -> Option<&str> {
    let raw = raw?.trim();
    let (scheme, token) = raw.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    if token.is_empty()
        || token.chars().any(char::is_whitespace)
        || HeaderValue::try_from(token).is_err()
    {
        return None;
    }
    Some(token)
}

fn protected_http_host(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    if host.eq_ignore_ascii_case("localhost")
        || host.eq_ignore_ascii_case("host.docker.internal")
        || !host.contains('.')
        || host.ends_with(".svc")
        || host.ends_with(".svc.cluster.local")
    {
        return true;
    }
    host.parse::<IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

fn optional_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn env_bool(name: &str, default: bool) -> bool {
    env::var(name)
        .ok()
        .and_then(|value| match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        })
        .unwrap_or(default)
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> SharedAuthConfig {
        SharedAuthConfig {
            base_url: "https://auth.test".to_owned(),
            introspect_secret: "x".repeat(32),
            issuer: "https://auth.test".to_owned(),
            audience: "quaestor".to_owned(),
            request_timeout: Duration::from_secs(1),
            require_session_id: true,
        }
    }

    fn active() -> Introspection {
        let now = now_seconds();
        Introspection {
            active: true,
            sub: Some("shared-user-1".to_owned()),
            iss: Some("https://auth.test".to_owned()),
            aud: Some("quaestor".to_owned()),
            exp: Some(now + 300),
            iat: Some(now),
            auth_time: Some(now),
            sid: Some("session-1".to_owned()),
            provider: Some("supabase".to_owned()),
            provider_tenant: Some("quaestor-ledger".to_owned()),
            provider_subject: Some("provider-user-1".to_owned()),
            email: Some("operator@example.com".to_owned()),
            email_verified: true,
            roles: vec!["user".to_owned()],
            aal: Some(2),
            amr: vec!["pwd".to_owned(), "totp".to_owned()],
            acr: Some(ACR_LOA2.to_owned()),
        }
    }

    #[test]
    fn active_revocation_aware_identity_preserves_assurance_time() {
        let verifier = SharedAuthVerifier {
            config: config(),
            http: Client::new(),
        };
        let claims = active();
        let expected_auth_time = claims.auth_time.unwrap();
        let identity = verifier.identity_from_claims(claims).unwrap();
        assert_eq!(identity.subject, "shared-user-1");
        assert!(identity.assurance.is_aal2());
        assert_eq!(identity.issued_at, expected_auth_time);
        assert_eq!(identity.session_id.as_deref(), Some("session-1"));
        assert_eq!(identity.amr, ["pwd", "totp"]);
    }

    #[test]
    fn inactive_or_sessionless_tokens_fail_closed() {
        let verifier = SharedAuthVerifier {
            config: config(),
            http: Client::new(),
        };
        let mut inactive = active();
        inactive.active = false;
        assert!(matches!(
            verifier.identity_from_claims(inactive),
            Err(AuthError::Unauthorized)
        ));
        let mut sessionless = active();
        sessionless.sid = None;
        assert!(matches!(
            verifier.identity_from_claims(sessionless),
            Err(AuthError::Unauthorized)
        ));
    }

    #[test]
    fn loa2_without_authoritative_auth_time_is_rejected() {
        let verifier = SharedAuthVerifier {
            config: config(),
            http: Client::new(),
        };
        let mut claims = active();
        claims.auth_time = None;
        assert!(matches!(
            verifier.identity_from_claims(claims),
            Err(AuthError::Unavailable(_))
        ));
    }

    #[test]
    fn contradictory_assurance_is_authority_failure() {
        let verifier = SharedAuthVerifier {
            config: config(),
            http: Client::new(),
        };
        let mut claims = active();
        claims.aal = Some(1);
        assert!(matches!(
            verifier.identity_from_claims(claims),
            Err(AuthError::Unavailable(_))
        ));
    }

    #[test]
    fn provider_identity_fields_remain_opaque_but_bounded() {
        let verifier = SharedAuthVerifier {
            config: config(),
            http: Client::new(),
        };
        let mut claims = active();
        claims.provider_tenant = Some("https://issuer.example/tenant/acme".to_owned());
        claims.provider_subject = Some("auth0|customers/acme/user-1".to_owned());
        let identity = verifier.identity_from_claims(claims).unwrap();
        assert_eq!(identity.provider_subject, "auth0|customers/acme/user-1");
    }

    #[test]
    fn canonical_subject_and_session_identifiers_remain_strict() {
        let verifier = SharedAuthVerifier {
            config: config(),
            http: Client::new(),
        };
        let mut claims = active();
        claims.sub = Some("../other-tenant".to_owned());
        assert!(matches!(
            verifier.identity_from_claims(claims),
            Err(AuthError::Unauthorized)
        ));
    }

    #[test]
    fn bearer_parser_is_strict_about_the_credential() {
        assert_eq!(bearer_token(Some("Bearer abc.def")), Some("abc.def"));
        assert_eq!(bearer_token(Some("bearer abc")), Some("abc"));
        assert_eq!(bearer_token(Some("Basic abc")), None);
        assert_eq!(bearer_token(Some("Bearer a b")), None);
        assert_eq!(bearer_token(Some("Bearer ")), None);
    }

    #[test]
    fn plain_http_is_limited_to_local_or_cluster_hosts() {
        assert!(protected_http_host(&Url::parse("http://shared-auth:8080").unwrap()));
        assert!(protected_http_host(
            &Url::parse("http://shared-auth.default.svc.cluster.local:8080").unwrap()
        ));
        assert!(protected_http_host(&Url::parse("http://127.0.0.1:8080").unwrap()));
        assert!(!protected_http_host(
            &Url::parse("http://auth.example.com").unwrap()
        ));
    }
}