use std::{env, time::Duration};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{TimeZone as _, Utc};
use reqwest::{header, StatusCode, Url};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use super::{AuthContext, CredentialSource};
use crate::{config::Config, error::AppError};

const DEFAULT_SECURE_COOKIE: &str = "__Host-canonical-customer-auth";
const DEFAULT_LOOPBACK_COOKIE: &str = "canonical-customer-auth";
const VERIFY_PATH: &str = "/shared-auth/auth/verify";
const MAX_TOKEN_BYTES: usize = 16 * 1024;
const MAX_EMAIL_BYTES: usize = 320;

/// Origin-side verifier for first-party Shared Auth access tokens.
///
/// The Cloudflare Worker performs the same check before forwarding protected
/// traffic, but this verifier is deliberately independent: caller-supplied
/// `x-auth-*` headers are ignored and the origin asks the configured Canonical
/// Shared Auth realm to verify the raw token again, including session
/// revocation. The edge is therefore a routing/defence layer, never the sole
/// authorization authority.
#[derive(Clone)]
pub struct SharedAuthVerifier {
    client: reqwest::Client,
    verify_url: Url,
    cookie_name: String,
    csrf_key: [u8; 32],
}

impl SharedAuthVerifier {
    pub fn from_env(config: &Config) -> Result<Self, AppError> {
        let default_cookie = if config.cookie_secure {
            DEFAULT_SECURE_COOKIE
        } else {
            DEFAULT_LOOPBACK_COOKIE
        };
        let cookie_name = env::var("SHARED_AUTH_BROWSER_COOKIE_NAME")
            .unwrap_or_else(|_| default_cookie.to_owned());
        validate_cookie_name(&cookie_name, config.cookie_secure)?;

        let default_verify_url = format!(
            "{}{}",
            config.app_base_url.trim_end_matches('/'),
            VERIFY_PATH
        );
        let verify_url = env::var("SHARED_AUTH_VERIFY_URL")
            .unwrap_or(default_verify_url)
            .parse::<Url>()
            .map_err(|_| {
                AppError::BadRequest("SHARED_AUTH_VERIFY_URL must be an absolute URL".into())
            })?;
        validate_verify_url(&verify_url)?;

        let csrf_key: [u8; 32] = config
            .session_encryption_key
            .as_slice()
            .try_into()
            .map_err(|_| AppError::Crypto)?;
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(8))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;

        Ok(Self {
            client,
            verify_url,
            cookie_name,
            csrf_key,
        })
    }

    pub fn cookie_name(&self) -> &str {
        &self.cookie_name
    }

    pub async fn authenticate_session(&self, token: &str) -> Result<AuthContext, AppError> {
        self.authenticate_as(token, CredentialSource::SessionCookie)
            .await
    }

    pub async fn authenticate_bearer(&self, token: &str) -> Result<AuthContext, AppError> {
        self.authenticate_as(token, CredentialSource::Bearer).await
    }

    async fn authenticate_as(
        &self,
        token: &str,
        source: CredentialSource,
    ) -> Result<AuthContext, AppError> {
        if token.is_empty() || token.len() > MAX_TOKEN_BYTES {
            return Err(AppError::Unauthorized);
        }

        let response = self
            .client
            .get(self.verify_url.clone())
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|_| AppError::AuthUpstream)?;

        match response.status() {
            StatusCode::OK => {}
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => return Err(AppError::Unauthorized),
            StatusCode::TOO_MANY_REQUESTS => return Err(AppError::AuthBusy),
            _ => return Err(AppError::AuthUpstream),
        }

        let headers = response.headers();
        let user_id = required_header(headers, "x-auth-user-id")?
            .parse::<Uuid>()
            .map_err(|_| AppError::Unauthorized)?;
        let email = required_header(headers, "x-auth-email")?;
        if email.is_empty() || email.len() > MAX_EMAIL_BYTES || email.chars().any(char::is_control)
        {
            return Err(AppError::Unauthorized);
        }
        // Require cryptographically verified provider provenance even though
        // the quote service does not currently branch on provider type.
        let _provider = required_header(headers, "x-auth-provider")?;
        let _provider_tenant = required_header(headers, "x-auth-provider-tenant")?;

        Ok(AuthContext {
            user_id,
            email,
            source,
            supabase_session_id: None,
            session_hash: Some(token_fingerprint(token)),
            csrf_token: self.csrf_token_for_source(token, source),
            expires_at: verified_expiry(token),
        })
    }

    fn csrf_token_for_source(&self, token: &str, source: CredentialSource) -> Option<String> {
        match source {
            CredentialSource::SessionCookie => Some(self.csrf_token(token)),
            CredentialSource::Bearer => None,
        }
    }

    fn csrf_token(&self, token: &str) -> String {
        let mut digest = Sha256::new();
        digest.update(b"canonical-plus/shared-auth-csrf/v1\0");
        digest.update(self.csrf_key);
        digest.update((token.len() as u64).to_be_bytes());
        digest.update(token.as_bytes());
        URL_SAFE_NO_PAD.encode(digest.finalize())
    }
}

fn required_header(
    headers: &reqwest::header::HeaderMap,
    name: &'static str,
) -> Result<String, AppError> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or(AppError::Unauthorized)
}

fn token_fingerprint(token: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"canonical-plus/shared-auth-token/v1\0");
    digest.update(token.as_bytes());
    URL_SAFE_NO_PAD.encode(digest.finalize())
}

/// The token has already been verified by Shared Auth before this helper is
/// called. Decoding here only preserves the verified expiry as local metadata;
/// authorization never depends on this unverified parsing path.
fn verified_expiry(token: &str) -> chrono::DateTime<Utc> {
    let fallback = Utc::now() + chrono::Duration::minutes(5);
    let Some(payload) = token.split('.').nth(1) else {
        return fallback;
    };
    let Ok(payload) = URL_SAFE_NO_PAD.decode(payload) else {
        return fallback;
    };
    let Ok(claims) = serde_json::from_slice::<serde_json::Value>(&payload) else {
        return fallback;
    };
    claims
        .get("exp")
        .and_then(serde_json::Value::as_i64)
        .and_then(|timestamp| Utc.timestamp_opt(timestamp, 0).single())
        .filter(|expiry| *expiry > Utc::now())
        .unwrap_or(fallback)
}

fn validate_cookie_name(value: &str, secure: bool) -> Result<(), AppError> {
    let valid = !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'!' | b'#'..=b'+' | b'-' | b'.' | b'^'..=b'`' | b'|' | b'~')
        });
    if !valid || (secure && !value.starts_with("__Host-")) {
        return Err(AppError::BadRequest(
            "SHARED_AUTH_BROWSER_COOKIE_NAME must be a valid host-only cookie name".into(),
        ));
    }
    Ok(())
}

fn validate_verify_url(url: &Url) -> Result<(), AppError> {
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.host_str().is_none()
    {
        return Err(AppError::BadRequest(
            "SHARED_AUTH_VERIFY_URL must not contain credentials, query, or fragment".into(),
        ));
    }
    let host = url.host_str().unwrap_or_default();
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host == "127.0.0.1"
        || host == "::1"
        || host.ends_with(".svc")
        || host.contains(".svc.");
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        return Err(AppError::BadRequest(
            "SHARED_AUTH_VERIFY_URL requires HTTPS except on loopback or Kubernetes service DNS"
                .into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verifier() -> SharedAuthVerifier {
        SharedAuthVerifier {
            client: reqwest::Client::new(),
            verify_url: Url::parse("https://app.canonical.plus/shared-auth/auth/verify").unwrap(),
            cookie_name: DEFAULT_SECURE_COOKIE.into(),
            csrf_key: [7; 32],
        }
    }

    #[test]
    fn csrf_is_stable_and_bound_to_the_verified_cookie() {
        let verifier = verifier();
        assert_eq!(
            verifier.csrf_token("token-a"),
            verifier.csrf_token("token-a")
        );
        assert_ne!(
            verifier.csrf_token("token-a"),
            verifier.csrf_token("token-b")
        );
    }

    #[test]
    fn csrf_is_only_issued_for_cookie_credentials() {
        let verifier = verifier();
        assert!(verifier
            .csrf_token_for_source("token-a", CredentialSource::SessionCookie)
            .is_some());
        assert!(verifier
            .csrf_token_for_source("token-a", CredentialSource::Bearer)
            .is_none());
    }

    #[test]
    fn secure_cookie_names_must_be_host_only() {
        assert!(validate_cookie_name(DEFAULT_SECURE_COOKIE, true).is_ok());
        assert!(validate_cookie_name("canonical-auth", true).is_err());
        assert!(validate_cookie_name("canonical-auth", false).is_ok());
    }

    #[test]
    fn verifier_url_rejects_cross_scheme_and_embedded_state() {
        assert!(validate_verify_url(
            &Url::parse("https://app.canonical.plus/shared-auth/auth/verify").unwrap()
        )
        .is_ok());
        assert!(validate_verify_url(
            &Url::parse("http://shared-auth.namespace.svc.cluster.local:8080/auth/verify").unwrap()
        )
        .is_ok());
        assert!(
            validate_verify_url(&Url::parse("http://example.com/auth/verify").unwrap()).is_err()
        );
        assert!(validate_verify_url(
            &Url::parse("https://app.canonical.plus/auth/verify?token=secret").unwrap()
        )
        .is_err());
    }
}
