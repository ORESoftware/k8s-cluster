//! Per-project Supabase token verifier.
//!
//! Adapted from the org's reviewed verifier (sonus-auris-backend.rs →
//! daedalus-api-server.rs): accept only the token's declared algorithm, pin
//! `iss` and `aud`, keep a bounded JWKS cache, and single-flight/rate-limit
//! refreshes so an unknown-`kid` flood cannot amplify into outbound requests.
//!
//! ## Tandem resilience (Supabase down → we keep working)
//! The cache has two thresholds. Within [`JWKS_SOFT_TTL`] a cached key is served
//! directly. Past it we try to refresh — but if Supabase is unreachable and the
//! key is still within the longer [`JWKS_GRACE_TTL`], we serve the **stale**
//! cached key rather than failing. JWKS keys rotate rarely, so serving a
//! recently-fetched key through a multi-minute Supabase outage is safe and keeps
//! the exchange path alive. (A brand-new, never-seen `kid` during an outage is
//! still rejected — we cannot verify a key we never fetched.)

use std::time::{Duration, Instant};

use jsonwebtoken::{decode, decode_header, jwk::JwkSet, Algorithm, DecodingKey, Validation};
use tokio::sync::{Mutex as AsyncMutex, RwLock};

use crate::config::SupabaseProject;
use crate::error::AuthError;

use super::claims::{SupabaseClaims, VerifiedIdentity};

/// Fresh window: a cached key newer than this is used without any refresh.
/// Matches Supabase's JWKS edge cache.
const JWKS_SOFT_TTL: Duration = Duration::from_secs(600);
/// Grace window: past the soft TTL, a cached key is still served if a refresh
/// fails (Supabase down). Comfortably survives a multi-minute outage.
const JWKS_GRACE_TTL: Duration = Duration::from_secs(3600);
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

    fn validation(&self, alg: Algorithm) -> Validation {
        let mut validation = Validation::new(alg);
        validation.set_audience(&[self.audience.as_str()]);
        validation.set_issuer(&[self.issuer.as_str()]);
        validation.validate_exp = true;
        validation
    }

    /// Verify a bearer token against this project and extract the identity.
    /// Every failure returns [`AuthError::Unauthorized`].
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
            // Never let an attacker choose the verification primitive from an
            // untrusted JWT header. Supabase asymmetric keys are EC or RSA.
            if !matches!(header.alg, Algorithm::ES256 | Algorithm::RS256) {
                return Err(AuthError::Unauthorized);
            }
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

        if claims.sub.is_empty() || claims.sub.len() > 512 {
            return Err(AuthError::Unauthorized);
        }

        let (auth_level, auth_methods) = claims.assurance();
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
            auth_level,
            auth_methods,
        })
    }

    /// Resolve a `kid` to a decoding key with tandem resilience.
    async fn decoding_key_for(
        &self,
        http: &reqwest::Client,
        kid: &str,
    ) -> Result<DecodingKey, AuthError> {
        // 1. Fresh cache hit.
        if let Some(key) = self.cached_key(kid, JWKS_SOFT_TTL).await {
            return Ok(key);
        }

        // 2. Need a refresh (cold / soft-expired / unknown kid). Single-flight.
        let _guard = self.refresh_lock.lock().await;
        if let Some(key) = self.cached_key(kid, JWKS_SOFT_TTL).await {
            return Ok(key); // another waiter refreshed while we held for the lock
        }

        let may_fetch = {
            let last = *self.last_refresh.read().await;
            last.map(|t| t.elapsed() >= JWKS_MIN_REFRESH_INTERVAL)
                .unwrap_or(true)
        };
        if may_fetch {
            *self.last_refresh.write().await = Some(Instant::now());
            match self.fetch_jwks(http).await {
                Ok(set) => {
                    let key = set
                        .find(kid)
                        .and_then(|jwk| DecodingKey::from_jwk(jwk).ok());
                    *self.cache.write().await = Some(JwksCacheEntry {
                        fetched_at: Instant::now(),
                        set,
                    });
                    if let Some(key) = key {
                        return Ok(key);
                    }
                    // Fresh JWKS genuinely lacks this kid → reject.
                    return Err(AuthError::Unauthorized);
                }
                Err(_) => {
                    // Supabase unreachable — fall through to the grace path.
                    tracing::warn!(
                        project = %self.project,
                        "Supabase JWKS refresh failed; attempting stale-within-grace"
                    );
                }
            }
        }

        // 3. Refresh unavailable or rate-limited: serve a stale key within grace.
        if let Some(key) = self.cached_key(kid, JWKS_GRACE_TTL).await {
            tracing::warn!(
                project = %self.project,
                "serving stale JWKS key within grace window (Supabase refresh unavailable)"
            );
            return Ok(key);
        }
        Err(AuthError::Unauthorized)
    }

    /// A cached decoding key for `kid`, iff the cache entry is younger than
    /// `max_age` and contains `kid`.
    async fn cached_key(&self, kid: &str, max_age: Duration) -> Option<DecodingKey> {
        let guard = self.cache.read().await;
        let entry = guard.as_ref()?;
        if entry.fetched_at.elapsed() >= max_age {
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
        if resp
            .content_length()
            .is_some_and(|length| length > 1024 * 1024)
        {
            return Err(AuthError::Upstream);
        }
        let bytes = resp.bytes().await.map_err(|_| AuthError::Upstream)?;
        if bytes.len() > 1024 * 1024 {
            return Err(AuthError::Upstream);
        }
        serde_json::from_slice::<JwkSet>(&bytes).map_err(|_| AuthError::Upstream)
    }

    /// Test-only: seed the JWKS cache as though a fetch just happened, so the
    /// stale-within-grace path can be exercised without a network.
    #[cfg(test)]
    pub async fn seed_cache_for_test(&self, set: JwkSet, age: Duration) {
        *self.cache.write().await = Some(JwksCacheEntry {
            fetched_at: Instant::now() - age,
            set,
        });
        // Mark a very recent refresh attempt so decoding_key_for skips the fetch
        // and goes straight to the grace path.
        *self.last_refresh.write().await = Some(Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SupabaseApiKeys;
    use jsonwebtoken::{Algorithm, EncodingKey, Header};
    use p256::pkcs8::{DecodePrivateKey, EncodePrivateKey, LineEnding};

    /// A deterministic P-256 signing key from a fixed scalar (no transcription
    /// risk, no RNG). `seed` distinguishes independent keys.
    fn supa_pem() -> String {
        let sk = p256::SecretKey::from_slice(&[9u8; 32]).unwrap();
        sk.to_pkcs8_pem(LineEnding::LF).unwrap().to_string()
    }

    fn now_plus(secs: i64) -> i64 {
        chrono::Utc::now().timestamp() + secs
    }

    fn hs256_project() -> SupabaseProject {
        SupabaseProject {
            name: "test-org".to_string(),
            project_ref: "testref".to_string(),
            issuer: Some("https://testref.supabase.co/auth/v1".to_string()),
            jwks_url: Some("http://127.0.0.1:1/jwks".to_string()),
            audience: "authenticated".to_string(),
            publishable_key_env: None,
            secret_key_env: None,
            service_role_key_env: None,
            jwt_secret_env: None,
            api_keys: SupabaseApiKeys::default(),
            hs256_secret: Some("super-secret-test-key".to_string()),
        }
    }

    fn es256_project() -> SupabaseProject {
        SupabaseProject {
            name: "es-org".to_string(),
            project_ref: "esref".to_string(),
            issuer: Some("https://esref.supabase.co/auth/v1".to_string()),
            // Dead address: any real fetch fails, forcing the grace path.
            jwks_url: Some("http://127.0.0.1:1/jwks".to_string()),
            audience: "authenticated".to_string(),
            publishable_key_env: None,
            secret_key_env: None,
            service_role_key_env: None,
            jwt_secret_env: None,
            api_keys: SupabaseApiKeys::default(),
            hs256_secret: None,
        }
    }

    fn sign_hs256(secret: &str, claims: serde_json::Value) -> String {
        jsonwebtoken::encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(secret.as_bytes()),
        )
        .unwrap()
    }

    /// Build a one-key JWKS from a PEM's public half, tagged with `kid`.
    fn jwks_from(pem: &str, kid: &str) -> JwkSet {
        let sk = p256::SecretKey::from_pkcs8_pem(pem).unwrap();
        let mut jwk = serde_json::to_value(sk.public_key().to_jwk()).unwrap();
        let obj = jwk.as_object_mut().unwrap();
        obj.insert("kid".into(), kid.into());
        obj.insert("alg".into(), "ES256".into());
        obj.insert("use".into(), "sig".into());
        serde_json::from_value(serde_json::json!({ "keys": [jwk] })).unwrap()
    }

    fn sign_es256(pem: &str, kid: &str, claims: serde_json::Value) -> String {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(kid.to_string());
        jsonwebtoken::encode(
            &header,
            &claims,
            &EncodingKey::from_ec_pem(pem.as_bytes()).unwrap(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn hs256_verifies_and_extracts_identity() {
        let v = SupabaseVerifier::new(&hs256_project());
        let token = sign_hs256(
            "super-secret-test-key",
            serde_json::json!({
                "sub": "user-123", "aud": "authenticated",
                "iss": "https://testref.supabase.co/auth/v1",
                "exp": now_plus(3600), "email": "a@b.co", "email_verified": true,
            }),
        );
        let id = v.verify(&reqwest::Client::new(), &token).await.unwrap();
        assert_eq!(id.supabase_user_id, "user-123");
        assert_eq!(id.project, "test-org");
        assert!(id.email_verified);
        assert_eq!(id.email.as_deref(), Some("a@b.co"));
    }

    #[tokio::test]
    async fn rejects_wrong_secret_and_wrong_issuer() {
        let v = SupabaseVerifier::new(&hs256_project());
        let bad_sig = sign_hs256(
            "not-the-secret",
            serde_json::json!({"sub":"x","aud":"authenticated","iss":"https://testref.supabase.co/auth/v1","exp":now_plus(60)}),
        );
        assert!(v.verify(&reqwest::Client::new(), &bad_sig).await.is_err());

        let wrong_iss = sign_hs256(
            "super-secret-test-key",
            serde_json::json!({"sub":"x","aud":"authenticated","iss":"https://evil.example/auth/v1","exp":now_plus(60)}),
        );
        assert!(v.verify(&reqwest::Client::new(), &wrong_iss).await.is_err());
    }

    #[tokio::test]
    async fn expired_token_is_rejected() {
        let v = SupabaseVerifier::new(&hs256_project());
        let expired = sign_hs256(
            "super-secret-test-key",
            serde_json::json!({"sub":"x","aud":"authenticated","iss":"https://testref.supabase.co/auth/v1","exp":now_plus(-3600)}),
        );
        assert!(v.verify(&reqwest::Client::new(), &expired).await.is_err());
    }

    // Tandem resilience: Supabase JWKS endpoint is unreachable, but a recently
    // cached key is still within grace → verification keeps working.
    #[tokio::test]
    async fn stale_jwks_within_grace_still_verifies_when_supabase_down() {
        let v = SupabaseVerifier::new(&es256_project());
        // Cache is 12 minutes old: past the 10-min soft TTL, inside the 60-min grace.
        v.seed_cache_for_test(jwks_from(&supa_pem(), "k1"), Duration::from_secs(720))
            .await;
        let token = sign_es256(
            &supa_pem(),
            "k1",
            serde_json::json!({
                "sub":"es-user","aud":"authenticated",
                "iss":"https://esref.supabase.co/auth/v1","exp":now_plus(3600),
            }),
        );
        // jwks_url points at a dead port, so the refresh fails and the grace path runs.
        let id = v.verify(&reqwest::Client::new(), &token).await.unwrap();
        assert_eq!(id.supabase_user_id, "es-user");
    }

    // Beyond the grace window, a stale key is no longer trusted.
    #[tokio::test]
    async fn stale_jwks_beyond_grace_is_rejected() {
        let v = SupabaseVerifier::new(&es256_project());
        v.seed_cache_for_test(jwks_from(&supa_pem(), "k1"), Duration::from_secs(7200))
            .await; // 2h > 60m grace
        let token = sign_es256(
            &supa_pem(),
            "k1",
            serde_json::json!({"sub":"es-user","aud":"authenticated","iss":"https://esref.supabase.co/auth/v1","exp":now_plus(3600)}),
        );
        assert!(v.verify(&reqwest::Client::new(), &token).await.is_err());
    }

    fn decoy_pem() -> String {
        p256::SecretKey::from_slice(&[11u8; 32])
            .unwrap()
            .to_pkcs8_pem(LineEnding::LF)
            .unwrap()
            .to_string()
    }

    fn one_jwk(pem: &str, kid: &str) -> serde_json::Value {
        let sk = p256::SecretKey::from_pkcs8_pem(pem).unwrap();
        let mut jwk = serde_json::to_value(sk.public_key().to_jwk()).unwrap();
        let obj = jwk.as_object_mut().unwrap();
        obj.insert("kid".into(), kid.into());
        obj.insert("alg".into(), "ES256".into());
        obj.insert("use".into(), "sig".into());
        jwk
    }

    // A JWKS with multiple keys: the token's `kid` selects the right one.
    #[tokio::test]
    async fn selects_correct_key_by_kid_among_many() {
        let v = SupabaseVerifier::new(&es256_project());
        let set: JwkSet = serde_json::from_value(serde_json::json!({
            "keys": [ one_jwk(&decoy_pem(), "decoy"), one_jwk(&supa_pem(), "real") ]
        }))
        .unwrap();
        v.seed_cache_for_test(set, Duration::from_secs(60)).await;

        // Signed by the "real" key; must be selected out of the two.
        let token = sign_es256(
            &supa_pem(),
            "real",
            serde_json::json!({"sub":"multi","aud":"authenticated","iss":"https://esref.supabase.co/auth/v1","exp":now_plus(3600)}),
        );
        assert_eq!(
            v.verify(&reqwest::Client::new(), &token)
                .await
                .unwrap()
                .supabase_user_id,
            "multi"
        );

        // A token whose kid isn't in the (fresh) set is rejected.
        let unknown = sign_es256(
            &decoy_pem(),
            "ghost",
            serde_json::json!({"sub":"x","aud":"authenticated","iss":"https://esref.supabase.co/auth/v1","exp":now_plus(3600)}),
        );
        assert!(v.verify(&reqwest::Client::new(), &unknown).await.is_err());
    }
}
