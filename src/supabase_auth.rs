//! Supabase access-token verification and JWKS key rotation.
//!
//! This module owns the security boundary between a caller-supplied bearer
//! token and the verified account identity used by the rest of the service.
//! It accepts only the explicit Supabase algorithms, pins `aud` and `iss`,
//! keeps a bounded JWKS cache, and single-flights/rate-limits refreshes so an
//! unknown-`kid` flood cannot amplify into outbound requests.

use std::time::{Duration, Instant};

use jsonwebtoken::{
    decode, decode_header,
    jwk::{Jwk, JwkSet, KeyAlgorithm, PublicKeyUse},
    Algorithm, DecodingKey, Validation,
};
use serde::Deserialize;
use tokio::sync::{Mutex as AsyncMutex, RwLock};
use tracing::error;

use super::{ServiceError, SupabaseConfig};

const SUPABASE_SUBJECT_PREFIX: &str = "supabase:";
// Supabase's JWKS edge cache is ten minutes. Do not retain keys longer here or
// emergency key revocation could remain trusted well beyond the provider cache.
const JWKS_CACHE_TTL: Duration = Duration::from_secs(600);
/// Minimum wall-clock between Supabase JWKS fetches. A flood of tokens bearing
/// unknown `kid`s (random or post-rotation) must not amplify into one outbound
/// JWKS request per token; once the cache is warm, legitimate tokens are served
/// from it and never reach the network, so throttling only bounds the misses.
const JWKS_MIN_REFRESH_INTERVAL: Duration = Duration::from_secs(30);

/// Verified Supabase identity derived from an access-token JWT.
#[derive(Clone, Debug)]
pub(crate) struct SupabaseIdentity {
    pub(crate) subject: String,
    pub(crate) email: Option<String>,
}

impl SupabaseIdentity {
    /// Namespaced external subject stored on the account so a Supabase `sub`
    /// can never collide with subjects minted by another identity source.
    pub(crate) fn external_subject(&self) -> String {
        format!("{SUPABASE_SUBJECT_PREFIX}{}", self.subject)
    }
}

#[derive(Debug, Deserialize)]
struct SupabaseClaims {
    sub: String,
    /// Supabase raises this claim to `aal2` only after a verified MFA
    /// challenge. Passwordless email OTP alone establishes `aal1`.
    #[serde(default)]
    aal: Option<String>,
    #[serde(default)]
    email: Option<String>,
    /// Supabase places email confirmation state here. It is a top-level claim on
    /// current tokens and a `user_metadata` field on older ones, so both are
    /// read and either one asserting confirmation is enough.
    #[serde(default)]
    email_verified: Option<bool>,
    #[serde(default)]
    user_metadata: Option<UserMetadata>,
}

#[derive(Debug, Default, Deserialize)]
struct UserMetadata {
    #[serde(default)]
    email_verified: Option<bool>,
}

impl SupabaseClaims {
    /// Whether the current access token proves the mandatory second factor.
    fn has_aal2(&self) -> bool {
        self.aal.as_deref() == Some("aal2")
    }

    /// Whether Supabase has confirmed the caller actually controls this address.
    ///
    /// Every email-keyed decision downstream (account display identity, cloud
    /// linking, operator gating) is only as strong as the project's "Confirm
    /// email" setting. With confirmation disabled, anyone can sign up claiming
    /// someone else's address and receive a cryptographically valid token
    /// carrying it. Requiring confirmation here makes those decisions hold
    /// regardless of how the Supabase project is configured.
    fn email_is_confirmed(&self) -> bool {
        self.email_verified
            .or(self.user_metadata.as_ref().and_then(|m| m.email_verified))
            .unwrap_or(false)
    }
}

pub(crate) struct JwksCacheEntry {
    fetched_at: Instant,
    set: JwkSet,
}

/// Verifies Supabase-issued access tokens (HS256 legacy secret or asymmetric
/// JWKS keys) so device registration and cloud linking can be tied to a real,
/// server-verified identity instead of a client-asserted subject string.
pub(crate) struct SupabaseVerifier {
    pub(crate) audience: String,
    pub(crate) issuer: Option<String>,
    pub(crate) jwt_secret: Option<String>,
    pub(crate) jwks_url: Option<String>,
    pub(crate) jwks_cache: RwLock<Option<JwksCacheEntry>>,
    /// When the last JWKS refresh was *attempted* (success or failure), used to
    /// rate-limit outbound fetches. See [`JWKS_MIN_REFRESH_INTERVAL`].
    pub(crate) jwks_last_refresh: RwLock<Option<Instant>>,
    /// Single-flight guard: concurrent cache misses wait for the same refresh
    /// instead of racing and incorrectly returning 401 while a valid key is
    /// still being fetched.
    pub(crate) jwks_refresh_lock: AsyncMutex<()>,
}

impl SupabaseVerifier {
    pub(crate) fn from_config(config: &SupabaseConfig) -> Option<Self> {
        if !config.is_enabled() {
            return None;
        }
        Some(Self {
            audience: config.audience.clone(),
            issuer: config.issuer.clone(),
            jwt_secret: config.jwt_secret.clone(),
            jwks_url: config.jwks_url.clone(),
            jwks_cache: RwLock::new(None),
            jwks_last_refresh: RwLock::new(None),
            jwks_refresh_lock: AsyncMutex::new(()),
        })
    }

    fn validation(&self, alg: Algorithm) -> Validation {
        let mut validation = Validation::new(alg);
        validation.set_audience(&[self.audience.as_str()]);
        if let Some(issuer) = &self.issuer {
            validation.set_issuer(&[issuer.as_str()]);
        }
        validation.validate_exp = true;
        // `aud` is "authenticated" on every Supabase project, so it identifies
        // nothing on its own; `iss` is what pins the token to *our* project.
        // SupabaseConfig::is_enabled refuses to build a verifier without one, so
        // this is belt-and-braces.
        debug_assert!(self.issuer.is_some(), "issuer must be pinned");
        validation
    }

    pub(crate) async fn verify(
        &self,
        http: &reqwest::Client,
        token: &str,
    ) -> Result<SupabaseIdentity, ServiceError> {
        let header = decode_header(token).map_err(|_| ServiceError::Unauthorized)?;
        if !is_supported_supabase_algorithm(header.alg) {
            return Err(ServiceError::Unauthorized);
        }
        let claims = if matches!(header.alg, Algorithm::HS256) {
            let secret = self.jwt_secret.as_deref().ok_or_else(|| {
                ServiceError::Unavailable(
                    "Supabase HS256 token received but SOUND_RECORDER_SUPABASE_JWT_SECRET is not configured".to_string(),
                )
            })?;
            decode::<SupabaseClaims>(
                token,
                &DecodingKey::from_secret(secret.as_bytes()),
                &self.validation(Algorithm::HS256),
            )
            .map_err(|_| ServiceError::Unauthorized)?
            .claims
        } else {
            let kid = header.kid.ok_or(ServiceError::Unauthorized)?;
            let jwk = self.jwk_for_kid(http, &kid, header.alg).await?;
            let key = DecodingKey::from_jwk(&jwk).map_err(|_| ServiceError::Unauthorized)?;
            decode::<SupabaseClaims>(token, &key, &self.validation(header.alg))
                .map_err(|_| ServiceError::Unauthorized)?
                .claims
        };
        if !claims.has_aal2() {
            return Err(ServiceError::Unauthorized);
        }
        let subject = claims.sub.trim().to_string();
        if subject.is_empty()
            || subject.len() > 160
            || subject
                .chars()
                .any(|ch| ch.is_control() || matches!(ch, '/' | '\\'))
        {
            return Err(ServiceError::Unauthorized);
        }
        // An unconfirmed address is not evidence of identity, so it is discarded
        // before anything downstream can key a decision on it. The token is
        // still accepted — `sub` is what identifies the account — but the
        // identity carries no email, exactly as if the claim were absent.
        let email_is_confirmed = claims.email_is_confirmed();
        let email = claims
            .email
            .map(|email| email.trim().to_string())
            .filter(|email| !email.is_empty() && email.len() <= 320)
            .filter(|_| {
                if email_is_confirmed {
                    return true;
                }
                tracing::warn!(
                    auth.subject = %subject,
                    "Supabase token carries an unconfirmed email claim; discarding it"
                );
                false
            });
        Ok(SupabaseIdentity { subject, email })
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
    pub(crate) async fn try_refresh_jwks(
        &self,
        http: &reqwest::Client,
    ) -> Result<bool, ServiceError> {
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

    pub(crate) async fn refresh_jwks(&self, http: &reqwest::Client) -> Result<(), ServiceError> {
        let jwks_url = self.jwks_url.as_deref().ok_or_else(|| {
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

pub(crate) fn is_supported_supabase_algorithm(algorithm: Algorithm) -> bool {
    matches!(
        algorithm,
        Algorithm::HS256 | Algorithm::RS256 | Algorithm::ES256
    )
}

#[cfg(test)]
mod tests {
    use jsonwebtoken::{jwk::Jwk, Algorithm};

    use super::{jwk_is_usable_for_algorithm, SupabaseClaims, SupabaseVerifier, UserMetadata};
    use crate::service::SupabaseConfig;

    const TEST_ISSUER: &str = "https://project.supabase.co/auth/v1";

    fn pinned_config() -> SupabaseConfig {
        SupabaseConfig {
            url: Some("https://project.supabase.co".to_string()),
            jwt_secret: Some("legacy-secret".to_string()),
            jwks_url: None,
            issuer: Some(TEST_ISSUER.to_string()),
            audience: "authenticated".to_string(),
            publishable_key: None,
            service_role_key: None,
            validation_errors: Vec::new(),
        }
    }

    /// Mint an HS256 token the way Supabase would, so the whole verify path
    /// (not just the claim helper) is exercised.
    fn hs256_token(body: serde_json::Value) -> String {
        use jsonwebtoken::{encode, EncodingKey, Header};
        encode(
            &Header::new(Algorithm::HS256),
            &body,
            &EncodingKey::from_secret(b"legacy-secret"),
        )
        .expect("token encodes")
    }

    fn token_body(email_verified: Option<bool>) -> serde_json::Value {
        let exp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3_600;
        let mut body = serde_json::json!({
            "sub": "00000000-0000-4000-8000-000000000001",
            "email": "listener@example.test",
            "aal": "aal2",
            "aud": "authenticated",
            "iss": TEST_ISSUER,
            "exp": exp,
        });
        if let Some(verified) = email_verified {
            body["email_verified"] = serde_json::Value::Bool(verified);
        }
        body
    }

    #[test]
    fn auth_refuses_to_enable_without_a_pinned_issuer() {
        // `aud` is the literal "authenticated" on every Supabase project, so
        // `iss` is the only claim binding a token to this one. Without it a
        // token from an unrelated project passes every other check.
        assert!(SupabaseVerifier::from_config(&pinned_config()).is_some());

        let mut unpinned = pinned_config();
        unpinned.issuer = None;
        assert!(
            SupabaseVerifier::from_config(&unpinned).is_none(),
            "an unpinned issuer would admit tokens from any Supabase project"
        );
    }

    #[tokio::test]
    async fn verification_drops_an_unconfirmed_email_but_keeps_the_subject() {
        let verifier = SupabaseVerifier::from_config(&pinned_config()).expect("verifier");
        let http = reqwest::Client::new();

        let confirmed = verifier
            .verify(&http, &hs256_token(token_body(Some(true))))
            .await
            .expect("a confirmed token verifies");
        assert_eq!(confirmed.email.as_deref(), Some("listener@example.test"));

        for unconfirmed in [Some(false), None] {
            let identity = verifier
                .verify(&http, &hs256_token(token_body(unconfirmed)))
                .await
                .expect("the token itself is still valid");
            assert_eq!(
                identity.subject, "00000000-0000-4000-8000-000000000001",
                "the subject still identifies the account"
            );
            assert_eq!(
                identity.email, None,
                "an unconfirmed address must never reach an email-keyed decision"
            );
        }
    }

    #[tokio::test]
    async fn a_token_from_another_supabase_project_is_rejected() {
        let verifier = SupabaseVerifier::from_config(&pinned_config()).expect("verifier");
        let http = reqwest::Client::new();
        let mut body = token_body(Some(true));
        body["iss"] = serde_json::Value::String("https://evil.supabase.co/auth/v1".to_string());
        assert!(verifier.verify(&http, &hs256_token(body)).await.is_err());
    }

    fn claims(email: &str, verified: Option<bool>, meta_verified: Option<bool>) -> SupabaseClaims {
        SupabaseClaims {
            sub: "user-1".to_string(),
            aal: Some("aal2".to_string()),
            email: Some(email.to_string()),
            email_verified: verified,
            user_metadata: meta_verified.map(|v| UserMetadata {
                email_verified: Some(v),
            }),
        }
    }

    #[test]
    fn an_unconfirmed_email_is_never_trusted() {
        // Email-keyed decisions are only as strong as the Supabase project's
        // "Confirm email" setting. With it off, anyone could sign up claiming
        // someone else's address and get a cryptographically valid token.
        assert!(!claims("listener@example.test", Some(false), None).email_is_confirmed());
        // A token with no confirmation claim at all must fail closed.
        assert!(!claims("listener@example.test", None, None).email_is_confirmed());
    }

    #[tokio::test]
    async fn aal1_and_missing_aal_are_rejected() {
        let verifier = SupabaseVerifier::from_config(&pinned_config()).expect("verifier");
        let http = reqwest::Client::new();

        for aal in [Some("aal1"), None] {
            let mut body = token_body(Some(true));
            match aal {
                Some(value) => {
                    body["aal"] = serde_json::Value::String(value.to_string());
                }
                None => {
                    body.as_object_mut()
                        .expect("token body object")
                        .remove("aal");
                }
            }
            assert!(
                verifier.verify(&http, &hs256_token(body)).await.is_err(),
                "tokens below AAL2 must fail closed"
            );
        }
    }

    #[test]
    fn confirmation_is_read_from_either_claim_location() {
        // Current Supabase tokens carry a top-level `email_verified`; older ones
        // carry it under `user_metadata`. Either asserting it is sufficient.
        assert!(claims("a@b.com", Some(true), None).email_is_confirmed());
        assert!(claims("a@b.com", None, Some(true)).email_is_confirmed());
        assert!(claims("a@b.com", Some(true), Some(false)).email_is_confirmed());
    }

    #[test]
    fn confirmation_claims_parse_from_a_real_token_body() {
        let top: SupabaseClaims = serde_json::from_str(
            r#"{"sub":"u1","email":"a@b.com","email_verified":true,"aud":"authenticated"}"#,
        )
        .expect("parses");
        assert!(top.email_is_confirmed());

        let nested: SupabaseClaims = serde_json::from_str(
            r#"{"sub":"u1","email":"a@b.com","user_metadata":{"email_verified":true}}"#,
        )
        .expect("parses");
        assert!(nested.email_is_confirmed());

        // Unknown claims must not break deserialization.
        let extra: SupabaseClaims =
            serde_json::from_str(r#"{"sub":"u1","email":"a@b.com","role":"authenticated"}"#)
                .expect("parses");
        assert!(!extra.email_is_confirmed());
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
}
