//! Supabase access-token verification, JWKS key rotation, and the org email gate.
//!
//! This module owns the security boundary between a caller-supplied bearer
//! token and the verified operator identity the rest of the service trusts.
//! It accepts only the explicit Supabase algorithms, pins `aud` and `iss`,
//! keeps a bounded JWKS cache, and single-flights/rate-limits refreshes so an
//! unknown-`kid` flood cannot amplify into outbound requests.
//!
//! Adapted from sonus-auris-backend.rs's verifier — the org's reviewed
//! implementation — with one addition: verification alone is not authorization.
//! Daedalus is single-operator, so a cryptographically valid token is still
//! rejected unless its `email` claim is on the configured allow-list.
//!
//! NOTE: the service-role key is never used here. That key bypasses RLS and is
//! reserved for offline operator tooling (see the daedalus-fab MCP server);
//! a request-serving process must act as the calling user, not as the project.

use std::time::{Duration, Instant};

use jsonwebtoken::{
    decode, decode_header,
    jwk::{Jwk, JwkSet, KeyAlgorithm, PublicKeyUse},
    Algorithm, DecodingKey, Validation,
};
use serde::Deserialize;
use tokio::sync::{Mutex as AsyncMutex, RwLock};
use tracing::error;

use crate::{config::SupabaseConfig, error::ServiceError};

// Supabase's JWKS edge cache is ten minutes. Do not retain keys longer here or
// emergency key revocation could remain trusted well beyond the provider cache.
const JWKS_CACHE_TTL: Duration = Duration::from_secs(600);
/// Minimum wall-clock between Supabase JWKS fetches. A flood of tokens bearing
/// unknown `kid`s (random or post-rotation) must not amplify into one outbound
/// JWKS request per token; once the cache is warm, legitimate tokens are served
/// from it and never reach the network, so throttling only bounds the misses.
const JWKS_MIN_REFRESH_INTERVAL: Duration = Duration::from_secs(30);

/// A verified Supabase identity that has also cleared the email allow-list.
///
/// Constructing this type is the authorization event: routes take it as proof
/// and never re-check the gate themselves.
#[derive(Clone, Debug)]
pub(crate) struct Operator {
    pub(crate) subject: String,
    pub(crate) email: String,
}

#[derive(Debug, Deserialize)]
struct SupabaseClaims {
    sub: String,
    #[serde(default)]
    email: Option<String>,
}

struct JwksCacheEntry {
    fetched_at: Instant,
    set: JwkSet,
}

pub(crate) struct SupabaseVerifier {
    config: SupabaseConfig,
    jwks_cache: RwLock<Option<JwksCacheEntry>>,
    /// When the last JWKS refresh was *attempted* (success or failure), used to
    /// rate-limit outbound fetches. See [`JWKS_MIN_REFRESH_INTERVAL`].
    jwks_last_refresh: RwLock<Option<Instant>>,
    /// Single-flight guard: concurrent cache misses wait for the same refresh
    /// instead of racing and incorrectly returning 401 while a valid key is
    /// still being fetched.
    jwks_refresh_lock: AsyncMutex<()>,
}

impl SupabaseVerifier {
    pub(crate) fn from_config(config: &SupabaseConfig) -> Option<Self> {
        if !config.is_enabled() {
            return None;
        }
        Some(Self {
            config: config.clone(),
            jwks_cache: RwLock::new(None),
            jwks_last_refresh: RwLock::new(None),
            jwks_refresh_lock: AsyncMutex::new(()),
        })
    }

    fn validation(&self, alg: Algorithm) -> Validation {
        let mut validation = Validation::new(alg);
        validation.set_audience(&[self.config.audience.as_str()]);
        if let Some(issuer) = &self.config.issuer {
            validation.set_issuer(&[issuer.as_str()]);
        }
        validation.validate_exp = true;
        validation
    }

    /// Verify a bearer token and enforce the email gate.
    ///
    /// Returns [`ServiceError::Unauthorized`] for every rejection reason —
    /// bad signature, wrong audience, expired, or email not permitted — so the
    /// response cannot be used to probe which operators exist.
    pub(crate) async fn authorize(
        &self,
        http: &reqwest::Client,
        token: &str,
    ) -> Result<Operator, ServiceError> {
        let claims = self.verify_claims(http, token).await?;

        let subject = claims.sub.trim().to_string();
        if subject.is_empty()
            || subject.len() > 160
            || subject
                .chars()
                .any(|ch| ch.is_control() || matches!(ch, '/' | '\\'))
        {
            return Err(ServiceError::Unauthorized);
        }

        let email = claims
            .email
            .map(|email| email.trim().to_string())
            .filter(|email| !email.is_empty() && email.len() <= 320);

        // Verification proved who the caller is; this decides whether they may
        // act. Both must pass.
        if !self.config.permits(email.as_deref()) {
            tracing::warn!(
                auth.subject = %subject,
                "verified Supabase token rejected by the email allow-list"
            );
            return Err(ServiceError::Unauthorized);
        }

        Ok(Operator {
            subject,
            // permits() returned true, which is only possible for Some(email).
            email: email.expect("allow-list cannot admit a token without an email"),
        })
    }

    async fn verify_claims(
        &self,
        http: &reqwest::Client,
        token: &str,
    ) -> Result<SupabaseClaims, ServiceError> {
        let header = decode_header(token).map_err(|_| ServiceError::Unauthorized)?;
        if !is_supported_supabase_algorithm(header.alg) {
            return Err(ServiceError::Unauthorized);
        }
        if matches!(header.alg, Algorithm::HS256) {
            let secret = self.config.jwt_secret.as_deref().ok_or_else(|| {
                ServiceError::Unavailable(
                    "Supabase HS256 token received but DAEDALUS_API_SUPABASE_JWT_SECRET is not configured"
                        .to_string(),
                )
            })?;
            Ok(decode::<SupabaseClaims>(
                token,
                &DecodingKey::from_secret(secret.as_bytes()),
                &self.validation(Algorithm::HS256),
            )
            .map_err(|_| ServiceError::Unauthorized)?
            .claims)
        } else {
            let kid = header.kid.ok_or(ServiceError::Unauthorized)?;
            let jwk = self.jwk_for_kid(http, &kid, header.alg).await?;
            let key = DecodingKey::from_jwk(&jwk).map_err(|_| ServiceError::Unauthorized)?;
            Ok(
                decode::<SupabaseClaims>(token, &key, &self.validation(header.alg))
                    .map_err(|_| ServiceError::Unauthorized)?
                    .claims,
            )
        }
    }

    async fn jwk_for_kid(
        &self,
        http: &reqwest::Client,
        kid: &str,
        algorithm: Algorithm,
    ) -> Result<Jwk, ServiceError> {
        if let Some(jwk) = self.cached_jwk(kid, algorithm).await {
            return Ok(jwk);
        }
        // Cache miss: the kid is unknown, unsuitable for the token algorithm,
        // or the cache aged out. Refresh at most once per
        // JWKS_MIN_REFRESH_INTERVAL so a burst of unknown-kid tokens cannot
        // turn into a burst of outbound JWKS fetches.
        let refreshed = self.try_refresh_jwks(http).await?;
        if let Some(jwk) = self.cached_jwk(kid, algorithm).await {
            return Ok(jwk);
        }
        if refreshed || self.jwks_cache.read().await.is_some() {
            Err(ServiceError::Unauthorized)
        } else {
            // No cache exists and a prior refresh failed or is throttled. This
            // is an identity-provider availability failure, not bad caller auth.
            Err(ServiceError::Unavailable(
                "Supabase signing keys are temporarily unavailable".to_string(),
            ))
        }
    }

    /// Refreshes the JWKS cache unless a refresh was attempted within the last
    /// [`JWKS_MIN_REFRESH_INTERVAL`]. Returns `Ok(true)` if a refresh ran (so the
    /// caller should re-check the cache) and `Ok(false)` if it was throttled.
    async fn try_refresh_jwks(&self, http: &reqwest::Client) -> Result<bool, ServiceError> {
        let _refresh_guard = self.jwks_refresh_lock.lock().await;
        {
            // Fast path: reserve the refresh slot under the write lock and bail
            // out (without an HTTP call) if another task refreshed recently.
            let mut last = self.jwks_last_refresh.write().await;
            if let Some(at) = *last {
                if at.elapsed() < JWKS_MIN_REFRESH_INTERVAL {
                    return Ok(false);
                }
            }
            *last = Some(Instant::now());
        }
        self.refresh_jwks(http).await?;
        Ok(true)
    }

    async fn cached_jwk(&self, kid: &str, algorithm: Algorithm) -> Option<Jwk> {
        let guard = self.jwks_cache.read().await;
        let entry = guard.as_ref()?;
        if entry.fetched_at.elapsed() > JWKS_CACHE_TTL {
            return None;
        }
        let jwk = entry.set.find(kid)?;
        jwk_is_usable_for_algorithm(jwk, algorithm).then(|| jwk.clone())
    }

    async fn refresh_jwks(&self, http: &reqwest::Client) -> Result<(), ServiceError> {
        let jwks_url = self.config.jwks_url.as_deref().ok_or_else(|| {
            ServiceError::Unavailable("Supabase JWKS URL is not configured".to_string())
        })?;
        let response = http.get(jwks_url).send().await.map_err(|err| {
            error!(error = %err, "Supabase JWKS fetch failed");
            ServiceError::Unavailable("Supabase JWKS fetch failed".to_string())
        })?;
        if !response.status().is_success() {
            return Err(ServiceError::Unavailable(format!(
                "Supabase JWKS fetch returned status {}",
                response.status().as_u16()
            )));
        }
        let set = response.json::<JwkSet>().await.map_err(|err| {
            error!(error = %err, "Supabase JWKS decode failed");
            ServiceError::Unavailable("Supabase JWKS response was invalid".to_string())
        })?;
        if set.keys.is_empty() {
            return Err(ServiceError::Unavailable(
                "Supabase JWKS did not contain any signing keys".to_string(),
            ));
        }
        let mut guard = self.jwks_cache.write().await;
        *guard = Some(JwksCacheEntry {
            fetched_at: Instant::now(),
            set,
        });
        Ok(())
    }
}

/// Reject key-confusion inputs even when a compromised/misconfigured issuer
/// returns multiple key types under one `kid`. Supabase signing keys identify
/// their intended signing algorithm; key use is optional in JWK but, when
/// present, must be `sig`.
fn jwk_is_usable_for_algorithm(jwk: &Jwk, algorithm: Algorithm) -> bool {
    let signing_use = matches!(
        &jwk.common.public_key_use,
        None | Some(PublicKeyUse::Signature)
    );
    let matching_algorithm = matches!(
        (jwk.common.key_algorithm, algorithm),
        (Some(KeyAlgorithm::RS256), Algorithm::RS256)
            | (Some(KeyAlgorithm::ES256), Algorithm::ES256)
    );
    signing_use && matching_algorithm
}

/// `none` and the HMAC-vs-RSA confusion algorithms are excluded by construction.
pub(crate) fn is_supported_supabase_algorithm(algorithm: Algorithm) -> bool {
    matches!(
        algorithm,
        Algorithm::HS256 | Algorithm::RS256 | Algorithm::ES256
    )
}

/// Extract a bearer token from an `Authorization` header value.
pub(crate) fn bearer_token(header: Option<&str>) -> Option<&str> {
    let raw = header?.trim();
    // ASCII case-insensitive scheme match, per RFC 7235.
    let (scheme, token) = raw.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("Bearer") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then_some(token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SupabaseConfig;

    fn gated_config() -> SupabaseConfig {
        SupabaseConfig {
            audience: "authenticated".to_string(),
            issuer: None,
            jwt_secret: Some("secret".to_string()),
            jwks_url: None,
            allowed_emails: vec!["alexander.d.mills@gmail.com".to_string()],
        }
    }

    #[test]
    fn jwks_key_must_match_the_token_algorithm_and_signature_use() {
        let signing_rsa: Jwk = serde_json::from_str(
            r#"{"kty":"RSA","kid":"key-1","use":"sig","alg":"RS256","n":"AQ","e":"AQAB"}"#,
        )
        .unwrap();
        assert!(jwk_is_usable_for_algorithm(&signing_rsa, Algorithm::RS256));
        assert!(!jwk_is_usable_for_algorithm(&signing_rsa, Algorithm::ES256));

        let encryption_rsa: Jwk = serde_json::from_str(
            r#"{"kty":"RSA","kid":"key-1","use":"enc","alg":"RS256","n":"AQ","e":"AQAB"}"#,
        )
        .unwrap();
        assert!(!jwk_is_usable_for_algorithm(
            &encryption_rsa,
            Algorithm::RS256
        ));
    }

    #[test]
    fn only_the_three_supabase_algorithms_are_accepted() {
        assert!(is_supported_supabase_algorithm(Algorithm::HS256));
        assert!(is_supported_supabase_algorithm(Algorithm::RS256));
        assert!(is_supported_supabase_algorithm(Algorithm::ES256));
        // Notably excluded: HS384/512 and RS384/512 are not Supabase-issued,
        // and admitting them widens the verification surface for no benefit.
        assert!(!is_supported_supabase_algorithm(Algorithm::HS512));
        assert!(!is_supported_supabase_algorithm(Algorithm::RS512));
    }

    #[test]
    fn verifier_is_absent_unless_both_key_and_gate_are_configured() {
        assert!(SupabaseVerifier::from_config(&gated_config()).is_some());

        let mut ungated = gated_config();
        ungated.allowed_emails.clear();
        assert!(
            SupabaseVerifier::from_config(&ungated).is_none(),
            "a verifier without an email gate would admit every project user"
        );
    }

    #[test]
    fn bearer_token_parsing_is_scheme_insensitive_and_strict() {
        assert_eq!(bearer_token(Some("Bearer abc123")), Some("abc123"));
        assert_eq!(bearer_token(Some("bearer abc123")), Some("abc123"));
        assert_eq!(bearer_token(Some("  Bearer   abc123  ")), Some("abc123"));
        assert_eq!(bearer_token(Some("Basic abc123")), None);
        assert_eq!(bearer_token(Some("Bearer")), None);
        assert_eq!(bearer_token(Some("Bearer   ")), None);
        assert_eq!(bearer_token(None), None);
    }
}
