//! Per-project Supabase token verifier.
//!
//! Adapted from the org's reviewed verifier (sonus-auris-backend.rs →
//! daedalus-api-server.rs): accept only the token's declared algorithm, pin
//! `iss` and `aud`, keep a bounded JWKS cache, and single-flight/rate-limit
//! refreshes so an unknown-`kid` flood cannot amplify into outbound requests.
//!
//! The one generalization: this verifier is instantiated once *per project*, and
//! [`super::ProjectRegistry`] owns routing an incoming token to the right one.

use std::time::{Duration, Instant};

use jsonwebtoken::{decode, decode_header, jwk::JwkSet, Algorithm, DecodingKey, Validation};
use tokio::sync::{Mutex as AsyncMutex, RwLock};

use crate::config::SupabaseProject;
use crate::error::AuthError;

use super::claims::{SupabaseClaims, VerifiedIdentity};

/// Supabase's JWKS edge cache is ten minutes; do not retain keys longer or an
/// emergency revocation stays trusted past the provider cache.
const JWKS_CACHE_TTL: Duration = Duration::from_secs(600);
/// Floor between outbound JWKS fetches, so unknown-`kid` misses can't amplify.
const JWKS_MIN_REFRESH_INTERVAL: Duration = Duration::from_secs(30);

struct JwksCacheEntry {
    fetched_at: Instant,
    set: JwkSet,
}

pub struct SupabaseVerifier {
    project: String,
    issuer: String,
    audience: String,
    jwks_url: String,
    /// Legacy HS256 shared-secret path; `None` for JWKS projects.
    hs256_secret: Option<String>,
    cache: RwLock<Option<JwksCacheEntry>>,
    last_refresh: RwLock<Option<Instant>>,
    refresh_lock: AsyncMutex<()>,
}

impl SupabaseVerifier {
    pub fn new(project: &SupabaseProject) -> Self {
        Self {
            project: project.name.clone(),
            issuer: project.issuer(),
            audience: project.audience.clone(),
            jwks_url: project.jwks_url(),
            hs256_secret: project.hs256_secret.clone(),
            cache: RwLock::new(None),
            last_refresh: RwLock::new(None),
            refresh_lock: AsyncMutex::new(()),
        }
    }

    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    pub fn project(&self) -> &str {
        &self.project
    }

    fn validation(&self, alg: Algorithm) -> Validation {
        let mut validation = Validation::new(alg);
        validation.set_audience(&[self.audience.as_str()]);
        validation.set_issuer(&[self.issuer.as_str()]);
        validation.validate_exp = true;
        validation
    }

    /// Verify a bearer token against this project and extract the identity.
    /// Every failure returns [`AuthError::Unauthorized`] so callers cannot probe
    /// which projects or users exist.
    pub async fn verify(
        &self,
        http: &reqwest::Client,
        token: &str,
    ) -> Result<VerifiedIdentity, AuthError> {
        let header = decode_header(token).map_err(|_| AuthError::Unauthorized)?;

        let claims = if let Some(secret) = &self.hs256_secret {
            let validation = self.validation(Algorithm::HS256);
            decode::<SupabaseClaims>(
                token,
                &DecodingKey::from_secret(secret.as_bytes()),
                &validation,
            )
            .map_err(|_| AuthError::Unauthorized)?
            .claims
        } else {
            let kid = header.kid.ok_or(AuthError::Unauthorized)?;
            let key = self.decoding_key_for(http, &kid).await?;
            let validation = self.validation(header.alg);
            decode::<SupabaseClaims>(token, &key, &validation)
                .map_err(|_| AuthError::Unauthorized)?
                .claims
        };

        let email = claims
            .email
            .as_deref()
            .map(str::trim)
            .filter(|e| !e.is_empty() && e.len() <= 320)
            .map(String::from);

        Ok(VerifiedIdentity {
            project: self.project.clone(),
            supabase_user_id: claims.sub.clone(),
            email_verified: claims.email_is_confirmed(),
            email,
            phone: claims.phone.clone(),
            role: claims.role.clone(),
            user_metadata: claims
                .user_metadata
                .clone()
                .unwrap_or(serde_json::Value::Null),
            app_metadata: claims
                .app_metadata
                .clone()
                .unwrap_or(serde_json::Value::Null),
        })
    }

    /// Resolve a `kid` to a decoding key, refreshing the JWKS at most once per
    /// [`JWKS_MIN_REFRESH_INTERVAL`] and single-flighting concurrent misses.
    async fn decoding_key_for(
        &self,
        http: &reqwest::Client,
        kid: &str,
    ) -> Result<DecodingKey, AuthError> {
        if let Some(key) = self.lookup_cached(kid).await {
            return Ok(key);
        }

        // Miss: take the single-flight lock so concurrent misses share one fetch.
        let _guard = self.refresh_lock.lock().await;
        // Another waiter may have refreshed while we waited on the lock.
        if let Some(key) = self.lookup_cached(kid).await {
            return Ok(key);
        }

        {
            let last = *self.last_refresh.read().await;
            if let Some(last) = last {
                if last.elapsed() < JWKS_MIN_REFRESH_INTERVAL {
                    // Rate-limited: don't hammer the provider on a bad-kid flood.
                    return Err(AuthError::Unauthorized);
                }
            }
        }

        *self.last_refresh.write().await = Some(Instant::now());
        let set = self.fetch_jwks(http).await?;
        let key = set
            .find(kid)
            .and_then(|jwk| DecodingKey::from_jwk(jwk).ok())
            .ok_or(AuthError::Unauthorized)?;
        *self.cache.write().await = Some(JwksCacheEntry {
            fetched_at: Instant::now(),
            set,
        });
        Ok(key)
    }

    async fn lookup_cached(&self, kid: &str) -> Option<DecodingKey> {
        let guard = self.cache.read().await;
        let entry = guard.as_ref()?;
        if entry.fetched_at.elapsed() >= JWKS_CACHE_TTL {
            return None;
        }
        entry
            .set
            .find(kid)
            .and_then(|jwk| DecodingKey::from_jwk(jwk).ok())
    }

    async fn fetch_jwks(&self, http: &reqwest::Client) -> Result<JwkSet, AuthError> {
        let resp = http
            .get(&self.jwks_url)
            .send()
            .await
            .map_err(|_| AuthError::Upstream)?;
        if !resp.status().is_success() {
            return Err(AuthError::Upstream);
        }
        resp.json::<JwkSet>().await.map_err(|_| AuthError::Upstream)
    }
}
