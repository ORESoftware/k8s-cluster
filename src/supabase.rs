//! Supabase identity: verify a Supabase-issued access token (JWT) and extract
//! the account it authenticates.
//!
//! Supabase Auth owns login (email/password, OAuth, MFA) and mints a short-lived
//! access JWT. This server never sees the password; it only verifies the JWT's
//! signature and claims, then maps `sub` (the Supabase user id) onto a local
//! account. Two signing schemes are supported:
//!
//! * **Asymmetric (preferred):** RS256 / ES256. The public keys are published at
//!   the project JWKS endpoint and selected per-token by the `kid` header. Keys
//!   are cached with a TTL and refreshed on an unknown `kid` (rotation-safe).
//! * **Legacy shared secret:** HS256 with the project JWT secret. Enabled only
//!   when `SUPABASE_JWT_LEGACY_SECRET` is configured.
//!
//! Claims are validated strictly: signature, `exp` (with small leeway), `aud`,
//! and `iss` must all match the configured project. A short-lived access token
//! that has expired is rejected here — the client refreshes it with Supabase (or,
//! for convenience, unlocks its stored refresh token with the local 6-digit PIN)
//! and retries.

use crate::error::ApiError;
use jsonwebtoken::jwk::{AlgorithmParameters, JwkSet};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use uuid::Uuid;

/// How long a fetched JWKS document is trusted before a refresh. Supabase key
/// rotation is infrequent; an unknown `kid` also forces an out-of-band refresh,
/// so this only bounds staleness for revoked keys.
const JWKS_TTL: Duration = Duration::from_secs(600);
/// Clock-skew leeway for `exp`/`nbf`, in seconds.
const LEEWAY_SECS: u64 = 30;
/// Cap the JWKS response so a hostile/misconfigured endpoint can't feed us an
/// unbounded body. Real Supabase JWKS documents are a few KiB.
const MAX_JWKS_BYTES: usize = 64 * 1024;

/// The authenticated identity carried by a valid Supabase access token.
#[derive(Debug, Clone)]
pub struct SupabaseIdentity {
    /// Supabase `sub` — the stable user id we key a local account on.
    pub user_id: Uuid,
    /// `email` claim, if present (absent for some OAuth/anonymous users).
    pub email: Option<String>,
}

/// Registered claims we read from a Supabase access token. Fields beyond these
/// (role, app_metadata, …) are ignored.
#[derive(Debug, Deserialize)]
struct Claims {
    sub: String,
    #[serde(default)]
    email: Option<String>,
}

struct CachedJwks {
    keys: HashMap<String, DecodingKey>,
    fetched_at: Instant,
}

/// Verifies Supabase access tokens against a single project. Cheap to clone
/// (shared cache + HTTP client behind `Arc`).
#[derive(Clone)]
pub struct SupabaseVerifier {
    inner: Arc<Inner>,
}

struct Inner {
    issuer: String,
    audience: String,
    jwks_url: String,
    legacy_hs256: Option<DecodingKey>,
    http: reqwest::Client,
    cache: RwLock<Option<CachedJwks>>,
}

impl SupabaseVerifier {
    /// Build a verifier from environment configuration. Returns `Ok(None)` when
    /// Supabase auth is not configured, so the server runs with Supabase disabled
    /// (the `/v1/auth/supabase` route then reports it unavailable).
    ///
    /// Env:
    /// * `SUPABASE_PROJECT_URL` — e.g. `https://abcd.supabase.co` (required to enable).
    /// * `SUPABASE_JWT_AUD`     — expected audience (default `authenticated`).
    /// * `SUPABASE_JWT_LEGACY_SECRET` — legacy HS256 secret (optional).
    pub fn from_env() -> Result<Option<Self>, Box<dyn std::error::Error + Send + Sync>> {
        let Some(project_url) = std::env::var("SUPABASE_PROJECT_URL").ok() else {
            return Ok(None);
        };
        let project_url = project_url.trim_end_matches('/').to_string();
        if !project_url.starts_with("https://") {
            return Err("SUPABASE_PROJECT_URL must be an https:// URL".into());
        }
        let audience =
            std::env::var("SUPABASE_JWT_AUD").unwrap_or_else(|_| "authenticated".to_string());
        // Supabase GoTrue issues tokens with issuer `<project>/auth/v1` and
        // publishes keys at `<project>/auth/v1/.well-known/jwks.json`.
        let issuer = format!("{project_url}/auth/v1");
        let jwks_url = format!("{issuer}/.well-known/jwks.json");

        let legacy_hs256 = std::env::var("SUPABASE_JWT_LEGACY_SECRET")
            .ok()
            .filter(|s| !s.is_empty())
            .map(|s| DecodingKey::from_secret(s.as_bytes()));

        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()?;

        Ok(Some(Self {
            inner: Arc::new(Inner {
                issuer,
                audience,
                jwks_url,
                legacy_hs256,
                http,
                cache: RwLock::new(None),
            }),
        }))
    }

    /// Verify a raw access token and return the authenticated identity, or an
    /// opaque `Unauthorized` for any failure (bad signature, expired, wrong
    /// audience/issuer, malformed). Nothing about *why* is leaked to the caller.
    pub async fn verify(&self, token: &str) -> Result<SupabaseIdentity, ApiError> {
        let header = decode_header(token).map_err(|_| ApiError::Unauthorized)?;
        let claims = match header.alg {
            Algorithm::HS256 => {
                let key = self
                    .inner
                    .legacy_hs256
                    .as_ref()
                    .ok_or(ApiError::Unauthorized)?;
                self.decode_with(token, key, Algorithm::HS256)?
            }
            alg @ (Algorithm::RS256 | Algorithm::ES256) => {
                let kid = header.kid.ok_or(ApiError::Unauthorized)?;
                let key = self.decoding_key(&kid).await?;
                self.decode_with(token, &key, alg)?
            }
            // Reject anything else (incl. `none`) outright.
            _ => return Err(ApiError::Unauthorized),
        };

        let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized)?;
        Ok(SupabaseIdentity {
            user_id,
            email: claims.email,
        })
    }

    fn decode_with(
        &self,
        token: &str,
        key: &DecodingKey,
        alg: Algorithm,
    ) -> Result<Claims, ApiError> {
        let mut validation = Validation::new(alg);
        validation.leeway = LEEWAY_SECS;
        validation.set_audience(&[&self.inner.audience]);
        validation.set_issuer(&[&self.inner.issuer]);
        validation.set_required_spec_claims(&["exp", "aud", "iss", "sub"]);
        decode::<Claims>(token, key, &validation)
            .map(|data| data.claims)
            .map_err(|_| ApiError::Unauthorized)
    }

    /// Resolve a signing key by `kid`. A **fresh** cache is authoritative: if it
    /// doesn't contain the `kid`, the token is rejected immediately. Only a stale
    /// or absent cache triggers a network refetch.
    ///
    /// This matters for DoS resistance: without it, a stream of tokens carrying
    /// random unknown `kid`s would force one JWKS fetch *per request* (an
    /// amplified fan-out to Supabase), because a fresh-but-missing lookup would
    /// keep falling through to refetch. The cost is that a genuinely rotated key
    /// is only picked up after the TTL (≤ `JWKS_TTL`), which is acceptable.
    async fn decoding_key(&self, kid: &str) -> Result<DecodingKey, ApiError> {
        if let Some(cached) = self.inner.cache.read().await.as_ref() {
            if cached.fetched_at.elapsed() < JWKS_TTL {
                // Fresh cache is the source of truth — hit or miss, no refetch.
                return cached.keys.get(kid).cloned().ok_or(ApiError::Unauthorized);
            }
        }
        // Stale or absent cache: refetch once, then decide.
        let keys = self.fetch_jwks().await?;
        let key = keys.get(kid).cloned();
        *self.inner.cache.write().await = Some(CachedJwks {
            keys,
            fetched_at: Instant::now(),
        });
        key.ok_or(ApiError::Unauthorized)
    }

    async fn fetch_jwks(&self) -> Result<HashMap<String, DecodingKey>, ApiError> {
        let resp = self
            .inner
            .http
            .get(&self.inner.jwks_url)
            .send()
            .await
            .map_err(|error| {
                tracing::warn!(error = %error, "JWKS fetch failed");
                ApiError::Unauthorized
            })?;
        if !resp.status().is_success() {
            tracing::warn!(status = %resp.status(), "JWKS endpoint returned non-2xx");
            return Err(ApiError::Unauthorized);
        }
        // Bound the body before buffering it.
        if resp
            .content_length()
            .is_some_and(|n| n as usize > MAX_JWKS_BYTES)
        {
            return Err(ApiError::Unauthorized);
        }
        let body = resp.bytes().await.map_err(|_| ApiError::Unauthorized)?;
        if body.len() > MAX_JWKS_BYTES {
            return Err(ApiError::Unauthorized);
        }
        let jwks: JwkSet = serde_json::from_slice(&body).map_err(|_| ApiError::Unauthorized)?;
        Ok(parse_jwks(&jwks))
    }
}

/// Convert a JWKS document into decoding keys indexed by `kid`, skipping any key
/// we don't understand (missing kid, unsupported algorithm) rather than failing
/// the whole set.
fn parse_jwks(jwks: &JwkSet) -> HashMap<String, DecodingKey> {
    let mut out = HashMap::new();
    for jwk in &jwks.keys {
        let Some(kid) = jwk.common.key_id.clone() else {
            continue;
        };
        let key = match &jwk.algorithm {
            AlgorithmParameters::RSA(rsa) => DecodingKey::from_rsa_components(&rsa.n, &rsa.e).ok(),
            AlgorithmParameters::EllipticCurve(ec) => {
                DecodingKey::from_ec_components(&ec.x, &ec.y).ok()
            }
            _ => None,
        };
        if let Some(key) = key {
            out.insert(kid, key);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{encode, EncodingKey, Header};
    use serde::Serialize;
    use std::time::{SystemTime, UNIX_EPOCH};

    const ISSUER: &str = "https://proj.supabase.co/auth/v1";
    const SECRET: &[u8] = b"legacy-project-jwt-secret";

    #[derive(Serialize)]
    struct TestClaims {
        sub: String,
        aud: String,
        iss: String,
        exp: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        email: Option<String>,
    }

    fn now() -> usize {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as usize
    }

    fn hs256_verifier() -> SupabaseVerifier {
        SupabaseVerifier {
            inner: Arc::new(Inner {
                issuer: ISSUER.to_string(),
                audience: "authenticated".to_string(),
                jwks_url: "https://proj.supabase.co/auth/v1/.well-known/jwks.json".to_string(),
                legacy_hs256: Some(DecodingKey::from_secret(SECRET)),
                http: reqwest::Client::new(),
                cache: RwLock::new(None),
            }),
        }
    }

    fn mint(claims: &TestClaims) -> String {
        encode(
            &Header::new(Algorithm::HS256),
            claims,
            &EncodingKey::from_secret(SECRET),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn accepts_valid_token_and_extracts_identity() {
        let sub = "b3f1c2d4-0000-1111-2222-333344445555";
        let token = mint(&TestClaims {
            sub: sub.to_string(),
            aud: "authenticated".to_string(),
            iss: ISSUER.to_string(),
            exp: now() + 3600,
            email: Some("user@example.com".to_string()),
        });
        let id = hs256_verifier().verify(&token).await.unwrap();
        assert_eq!(id.user_id, Uuid::parse_str(sub).unwrap());
        assert_eq!(id.email.as_deref(), Some("user@example.com"));
    }

    #[tokio::test]
    async fn rejects_expired_token() {
        let token = mint(&TestClaims {
            sub: "b3f1c2d4-0000-1111-2222-333344445555".to_string(),
            aud: "authenticated".to_string(),
            iss: ISSUER.to_string(),
            exp: now() - 3600,
            email: None,
        });
        assert!(hs256_verifier().verify(&token).await.is_err());
    }

    #[tokio::test]
    async fn rejects_wrong_audience() {
        let token = mint(&TestClaims {
            sub: "b3f1c2d4-0000-1111-2222-333344445555".to_string(),
            aud: "some-other-service".to_string(),
            iss: ISSUER.to_string(),
            exp: now() + 3600,
            email: None,
        });
        assert!(hs256_verifier().verify(&token).await.is_err());
    }

    #[tokio::test]
    async fn rejects_wrong_issuer() {
        let token = mint(&TestClaims {
            sub: "b3f1c2d4-0000-1111-2222-333344445555".to_string(),
            aud: "authenticated".to_string(),
            iss: "https://evil.example.com/auth/v1".to_string(),
            exp: now() + 3600,
            email: None,
        });
        assert!(hs256_verifier().verify(&token).await.is_err());
    }

    #[tokio::test]
    async fn rejects_tampered_signature() {
        let mut token = mint(&TestClaims {
            sub: "b3f1c2d4-0000-1111-2222-333344445555".to_string(),
            aud: "authenticated".to_string(),
            iss: ISSUER.to_string(),
            exp: now() + 3600,
            email: None,
        });
        // Flip the last signature character.
        let last = token.pop().unwrap();
        token.push(if last == 'A' { 'B' } else { 'A' });
        assert!(hs256_verifier().verify(&token).await.is_err());
    }

    #[tokio::test]
    async fn rejects_hs256_when_no_legacy_secret_configured() {
        let mut v = hs256_verifier();
        v.inner = Arc::new(Inner {
            issuer: ISSUER.to_string(),
            audience: "authenticated".to_string(),
            jwks_url: "https://proj.supabase.co/auth/v1/.well-known/jwks.json".to_string(),
            legacy_hs256: None,
            http: reqwest::Client::new(),
            cache: RwLock::new(None),
        });
        let token = mint(&TestClaims {
            sub: "b3f1c2d4-0000-1111-2222-333344445555".to_string(),
            aud: "authenticated".to_string(),
            iss: ISSUER.to_string(),
            exp: now() + 3600,
            email: None,
        });
        assert!(v.verify(&token).await.is_err());
    }

    #[tokio::test]
    async fn rejects_non_uuid_subject() {
        let token = mint(&TestClaims {
            sub: "not-a-uuid".to_string(),
            aud: "authenticated".to_string(),
            iss: ISSUER.to_string(),
            exp: now() + 3600,
            email: None,
        });
        assert!(hs256_verifier().verify(&token).await.is_err());
    }
}
