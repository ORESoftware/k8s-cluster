//! Supabase access-token verification (JWKS, key rotation, pinned claims).
//!
//! This module owns the boundary between a caller-supplied bearer token and the
//! verified *end-user* identity the rest of the service trusts. Until this
//! existed, `billing-server` had no notion of a per-user caller at all: it
//! authenticated with a single process-wide shared bearer
//! (`BILLING_API_AUTH_BEARER`) and delegated tenant ownership entirely to an
//! upstream gateway. Anyone holding that one token could operate on *any*
//! tenant by editing the `tenant_id` path segment — a textbook IDOR. See
//! [`crate::api::auth`] for the authorization half of the fix; this module is
//! only the authentication half.
//!
//! Ported from `fabrication-server.rs`'s verifier (itself a reviewed port of
//! `sonus-auris-backend.rs`'s), with the env prefix changed to `BILLING_` and
//! the single-operator email allow-list replaced by per-tenant entitlement,
//! which is billing's actual authorization question. The cryptographic rules
//! are unchanged and deliberately so — **do not fork this logic**. If a
//! verification rule changes, it changes in every service that carries a copy.
//!
//! Security properties, all of which have tests below:
//!
//!   * **Pinned issuer.** `aud` is the literal string `"authenticated"` on every
//!     Supabase project on earth, so it identifies nothing by itself; `iss` is
//!     what binds a token to *our* project. The verifier refuses to enable
//!     without one.
//!   * **Pinned audience**, checked alongside `iss`.
//!   * **`exp` / `nbf`** enforced with a small (30s) clock-skew allowance.
//!   * **No `alg: none`**, and no algorithm outside the small Supabase set.
//!   * **No algorithm confusion.** An RS256/ES256 token is verified against a
//!     JWKS key whose *declared* algorithm matches the header, and an HS256
//!     token is verified against the configured shared secret — never against
//!     key material fetched from the JWKS. When no shared secret is configured
//!     (the JWKS-only deployment, which is the recommended one), HS256 is
//!     rejected outright, so the classic "sign an HS256 token using the RSA
//!     public key as the HMAC secret" attack has no path at all.
//!   * **Bounded, single-flighted, rate-limited JWKS refresh**, so a flood of
//!     tokens bearing unknown `kid`s cannot amplify into a flood of outbound
//!     fetches against the identity provider.
//!
//! NOTE: the Supabase *service-role* key is never used here and must never be.
//! That key bypasses RLS and is reserved for offline operator tooling; a
//! request-serving process must act as the calling user, not as the project.

use std::fmt;
use std::time::{Duration, Instant};

use jsonwebtoken::{
    Algorithm, DecodingKey, Validation, decode, decode_header,
    jwk::{Jwk, JwkSet, KeyAlgorithm, PublicKeyUse},
};
use serde::Deserialize;
use tokio::sync::{Mutex as AsyncMutex, RwLock};
use tracing::error;
use uuid::Uuid;

/// Supabase's JWKS edge cache is ten minutes. Do not retain keys longer here or
/// an emergency key revocation could remain trusted well beyond the provider's
/// own cache window.
const JWKS_CACHE_TTL: Duration = Duration::from_secs(600);

/// Minimum wall-clock between Supabase JWKS fetches. A flood of tokens bearing
/// unknown `kid`s (random, or simply post-rotation) must not amplify into one
/// outbound JWKS request per token. Once the cache is warm, legitimate tokens
/// are served from it and never touch the network, so throttling only bounds
/// the misses.
const JWKS_MIN_REFRESH_INTERVAL: Duration = Duration::from_secs(30);

/// Clock-skew allowance for `exp` / `nbf`. Deliberately small: this is a
/// tolerance for NTP drift between the identity provider and this pod, not a
/// grace period for expired sessions.
const CLOCK_SKEW_LEEWAY_SECS: u64 = 30;

/// Why a token was refused.
///
/// Every cryptographic and claim failure collapses to [`AuthError::Unauthorized`]
/// so a caller cannot use the response to distinguish "no such user" from "wrong
/// signature" from "expired". [`AuthError::Unavailable`] is reserved for the
/// genuinely different case where *we* could not reach the identity provider —
/// that is a 503 on our side, not a 401 on theirs.
#[derive(Debug, PartialEq, Eq)]
pub enum AuthError {
    Unauthorized,
    Unavailable(String),
}

/// Supabase wiring. Absent/incomplete values simply leave the verifier disabled;
/// [`Config::from_env`](crate::config::Config::from_env) is what decides whether
/// a disabled verifier is allowed to boot.
#[derive(Clone, Default)]
pub struct SupabaseConfig {
    /// `BILLING_SUPABASE_URL` — the project base URL, e.g.
    /// `https://abcdefgh.supabase.co`.
    pub url: Option<String>,
    /// `BILLING_SUPABASE_JWT_AUD`, default `authenticated`.
    pub audience: String,
    /// `BILLING_SUPABASE_JWT_ISS`. Defaults to `<url>/auth/v1`, which is what
    /// Supabase actually stamps into the `iss` claim.
    pub issuer: Option<String>,
    /// Derived from `url`; overridable with `BILLING_SUPABASE_JWKS_URL` for
    /// self-hosted GoTrue deployments that don't follow the hosted layout.
    pub jwks_url: Option<String>,
    /// `BILLING_SUPABASE_JWT_SECRET` — the *legacy* symmetric secret. Leave it
    /// unset on any project using asymmetric (JWKS) signing keys; setting it
    /// widens the accepted algorithm set to include HS256 for no benefit.
    pub jwt_secret: Option<String>,
}

// The JWT secret is a credential. Keep it off every Debug surface so an
// incidental `tracing::debug!(?config)` cannot exfiltrate it — the same
// discipline `Config` itself follows, and asserted by a test in `config.rs`.
impl fmt::Debug for SupabaseConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SupabaseConfig")
            .field("url", &self.url)
            .field("audience", &self.audience)
            .field("issuer", &self.issuer)
            .field("jwks_url", &self.jwks_url)
            .field(
                "jwt_secret",
                &self.jwt_secret.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl SupabaseConfig {
    /// Standard hosted-Supabase JWKS path.
    pub fn jwks_url_for(base_url: &str) -> String {
        format!(
            "{}/auth/v1/.well-known/jwks.json",
            base_url.trim_end_matches('/')
        )
    }

    /// Standard hosted-Supabase issuer.
    pub fn issuer_for(base_url: &str) -> String {
        format!("{}/auth/v1", base_url.trim_end_matches('/'))
    }

    /// A verifier is only usable with a pinned issuer *and* at least one way to
    /// check a signature. Without the issuer pin, a token minted by any other
    /// Supabase project passes every remaining check.
    pub fn is_enabled(&self) -> bool {
        self.url.is_some()
            && self.issuer.is_some()
            && (self.jwks_url.is_some() || self.jwt_secret.is_some())
    }
}

/// A cryptographically verified Supabase end-user.
///
/// Constructing one of these is the *authentication* event. It is deliberately
/// not the authorization event: see [`SupabaseIdentity::is_entitled_to`], which
/// the API middleware calls with the tenant from the request path.
/// Authenticator Assurance Level, from the Supabase `aal` claim.
///
/// Anything that is not exactly `aal2` collapses to [`Aal::Aal1`] — an absent,
/// unknown, or malformed value must never read as *more* assured than it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Aal {
    /// One factor, or an absent/unknown `aal`. Not stepped up.
    Aal1,
    /// Two factors (`aal2`).
    Aal2,
}

impl Aal {
    fn from_claim(raw: Option<&str>) -> Self {
        match raw.map(str::trim) {
            Some("aal2") => Aal::Aal2,
            _ => Aal::Aal1,
        }
    }

    /// True only for a genuine two-factor session.
    pub fn is_aal2(self) -> bool {
        matches!(self, Aal::Aal2)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SupabaseIdentity {
    /// The Supabase user id (`sub`).
    pub subject: String,
    pub email: Option<String>,
    /// Supabase's `role` claim (`authenticated`, `anon`, …). Recorded for
    /// audit; not currently load-bearing.
    pub role: Option<String>,
    /// Tenants this token asserts membership of, parsed from the claims below.
    pub tenant_ids: Vec<Uuid>,
    /// Assurance level (`aal`). Load-bearing: financial mutations require AAL2.
    pub assurance: Aal,
    /// Financial scopes, read from the client-unwritable `app_metadata` bucket
    /// (never from user-writable metadata). Empty unless the issuer granted them.
    pub scopes: Vec<String>,
    /// Unix time of the freshest authentication factor (`amr`), used to require
    /// a *recent* step-up for mutations. `None` when the token records no factor
    /// timestamp, which is treated as "not fresh" and fails closed.
    pub step_up_at: Option<u64>,
}

impl SupabaseIdentity {
    /// The per-tenant authorization decision — the actual IDOR fix.
    pub fn is_entitled_to(&self, tenant_id: Uuid) -> bool {
        self.tenant_ids.contains(&tenant_id)
    }

    /// Whether this identity carries the given financial scope.
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|granted| granted == scope)
    }

    /// Age, in seconds, of the freshest recorded authentication factor relative
    /// to `now`. `None` when no factor timestamp is present (treated as stale).
    pub fn step_up_age_secs(&self, now: u64) -> Option<u64> {
        self.step_up_at.map(|at| now.saturating_sub(at))
    }
}

/// Claims we read. `deny_unknown_fields` is deliberately *not* used: Supabase
/// adds claims over time and an unknown one must never fail a login.
#[derive(Debug, Deserialize)]
struct SupabaseClaims {
    sub: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    role: Option<String>,
    /// Session assurance level, e.g. `"aal1"` / `"aal2"`. Absent on older tokens.
    #[serde(default)]
    aal: Option<String>,
    /// Authentication methods with timestamps, e.g.
    /// `[{"method":"password","timestamp":1712…},{"method":"totp","timestamp":…}]`.
    /// Used only to measure how recently the caller last authenticated a factor.
    #[serde(default)]
    amr: Vec<Amr>,
    /// `app_metadata` is the only claim bucket a Supabase user cannot write to
    /// from the client SDK — it is settable solely by the service-role key or a
    /// database trigger. Tenant membership and financial scopes therefore live
    /// here and *only* here. `user_metadata` is user-writable and is never
    /// consulted for authorization.
    #[serde(default)]
    app_metadata: Option<AppMetadata>,
}

#[derive(Debug, Deserialize)]
struct Amr {
    #[serde(default)]
    timestamp: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct AppMetadata {
    #[serde(default)]
    tenant_id: Option<String>,
    #[serde(default)]
    tenant_ids: Option<Vec<String>>,
    /// Financial authorization scopes granted to this principal (e.g.
    /// `["billing:write"]`). Client-unwritable, like tenant membership.
    #[serde(default)]
    financial_scopes: Option<Vec<String>>,
}

impl SupabaseClaims {
    /// Collect the tenant ids this token asserts. Unparseable entries are
    /// dropped rather than failing the whole token: a malformed claim must
    /// narrow access, never widen it, and dropping it does exactly that (the
    /// entitlement check then fails closed with a 403).
    fn tenant_ids(&self) -> Vec<Uuid> {
        let Some(meta) = self.app_metadata.as_ref() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let singles = meta.tenant_id.iter().cloned();
        let many = meta.tenant_ids.iter().flatten().cloned();
        for raw in singles.chain(many) {
            if let Ok(id) = Uuid::parse_str(raw.trim())
                && !out.contains(&id)
            {
                out.push(id);
            }
        }
        out
    }

    /// Financial scopes asserted by this token, trimmed and de-duplicated.
    /// Empty (deny-by-default) unless `app_metadata.financial_scopes` grants any.
    fn scopes(&self) -> Vec<String> {
        let Some(meta) = self.app_metadata.as_ref() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for raw in meta.financial_scopes.iter().flatten() {
            let scope = raw.trim();
            if !scope.is_empty() && !out.iter().any(|s: &String| s == scope) {
                out.push(scope.to_string());
            }
        }
        out
    }

    /// The most recent factor timestamp in `amr`, i.e. how recently the caller
    /// last authenticated. `None` when no factor carries a timestamp.
    fn latest_amr_at(&self) -> Option<u64> {
        self.amr.iter().filter_map(|entry| entry.timestamp).max()
    }
}

struct JwksCacheEntry {
    fetched_at: Instant,
    set: JwkSet,
}

pub struct SupabaseVerifier {
    config: SupabaseConfig,
    http: reqwest::Client,
    jwks_cache: RwLock<Option<JwksCacheEntry>>,
    /// When the last JWKS refresh was *attempted* (success or failure), used to
    /// rate-limit outbound fetches. See [`JWKS_MIN_REFRESH_INTERVAL`].
    jwks_last_refresh: RwLock<Option<Instant>>,
    /// Single-flight guard: concurrent cache misses wait for the same refresh
    /// instead of racing and each returning 401 while a valid key is still in
    /// flight.
    jwks_refresh_lock: AsyncMutex<()>,
}

// The verifier holds the JWT secret via its config. Give it a Debug that cannot
// print it, so it is safe to embed in other Debug-deriving structs.
impl fmt::Debug for SupabaseVerifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SupabaseVerifier")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl SupabaseVerifier {
    /// Returns `None` when Supabase is not configured, so the caller can treat
    /// "no verifier" and "verifier disabled" as one case.
    pub fn from_config(config: &SupabaseConfig) -> Option<Self> {
        if !config.is_enabled() {
            return None;
        }
        Some(Self::new_unchecked(config.clone(), reqwest::Client::new()))
    }

    fn new_unchecked(config: SupabaseConfig, http: reqwest::Client) -> Self {
        Self {
            config,
            http,
            jwks_cache: RwLock::new(None),
            jwks_last_refresh: RwLock::new(None),
            jwks_refresh_lock: AsyncMutex::new(()),
        }
    }

    fn validation(&self, alg: Algorithm) -> Validation {
        let mut validation = Validation::new(alg);
        validation.set_audience(&[self.config.audience.as_str()]);
        if let Some(issuer) = &self.config.issuer {
            validation.set_issuer(&[issuer.as_str()]);
        }
        // A token missing any of these is malformed for our purposes; requiring
        // them means a claim can never be "absent, therefore unchecked".
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        validation.validate_exp = true;
        validation.validate_nbf = true;
        validation.leeway = CLOCK_SKEW_LEEWAY_SECS;
        // `aud` is "authenticated" on every Supabase project, so it identifies
        // nothing on its own; `iss` is what pins the token to *our* project.
        // is_enabled() refuses to construct a verifier without one, so this is
        // belt-and-braces.
        debug_assert!(self.config.issuer.is_some(), "issuer must be pinned");
        validation
    }

    /// Verify a bearer token and return the authenticated identity.
    ///
    /// This proves *who* the caller is. It says nothing about what they may
    /// touch — that is [`SupabaseIdentity::is_entitled_to`], enforced by the
    /// API middleware against the tenant in the request path.
    pub async fn verify(&self, token: &str) -> Result<SupabaseIdentity, AuthError> {
        let claims = self.verify_claims(token).await?;

        let subject = claims.sub.trim().to_string();
        if subject.is_empty()
            || subject.len() > 160
            || subject
                .chars()
                .any(|ch| ch.is_control() || matches!(ch, '/' | '\\'))
        {
            return Err(AuthError::Unauthorized);
        }

        Ok(SupabaseIdentity {
            tenant_ids: claims.tenant_ids(),
            assurance: Aal::from_claim(claims.aal.as_deref()),
            scopes: claims.scopes(),
            step_up_at: claims.latest_amr_at(),
            subject,
            email: claims
                .email
                .as_deref()
                .map(str::trim)
                .filter(|e| !e.is_empty() && e.len() <= 320)
                .map(str::to_string),
            role: claims.role,
        })
    }

    async fn verify_claims(&self, token: &str) -> Result<SupabaseClaims, AuthError> {
        // `decode_header` fails outright on `alg: none` — jsonwebtoken has no
        // `Algorithm::None` variant to deserialize into — so unsigned tokens
        // never reach the code below.
        let header = decode_header(token).map_err(|_| AuthError::Unauthorized)?;
        if !is_supported_supabase_algorithm(header.alg) {
            return Err(AuthError::Unauthorized);
        }

        if matches!(header.alg, Algorithm::HS256) {
            // Symmetric path. Only reachable when an operator has explicitly
            // configured the legacy shared secret. On a JWKS-signed project
            // (`jwt_secret` unset) this is where algorithm confusion would
            // otherwise land, and it is refused.
            let Some(secret) = self.config.jwt_secret.as_deref() else {
                return Err(AuthError::Unauthorized);
            };
            return Ok(decode::<SupabaseClaims>(
                token,
                &DecodingKey::from_secret(secret.as_bytes()),
                &self.validation(Algorithm::HS256),
            )
            .map_err(|_| AuthError::Unauthorized)?
            .claims);
        }

        // Asymmetric path. The key is chosen by `kid` *and* must declare the
        // same algorithm the header claims, so one key can never be reused
        // under a different algorithm.
        let kid = header.kid.ok_or(AuthError::Unauthorized)?;
        let jwk = self.jwk_for_kid(&kid, header.alg).await?;
        let key = DecodingKey::from_jwk(&jwk).map_err(|_| AuthError::Unauthorized)?;
        Ok(
            decode::<SupabaseClaims>(token, &key, &self.validation(header.alg))
                .map_err(|_| AuthError::Unauthorized)?
                .claims,
        )
    }

    async fn jwk_for_kid(&self, kid: &str, algorithm: Algorithm) -> Result<Jwk, AuthError> {
        if let Some(jwk) = self.cached_jwk(kid, algorithm).await {
            return Ok(jwk);
        }
        // Cache miss: the kid is unknown, is unsuitable for the token's
        // algorithm, or the cache aged out. Refresh at most once per
        // JWKS_MIN_REFRESH_INTERVAL so a burst of unknown-kid tokens cannot
        // become a burst of outbound fetches.
        let refreshed = self.try_refresh_jwks().await?;
        if let Some(jwk) = self.cached_jwk(kid, algorithm).await {
            return Ok(jwk);
        }
        if refreshed || self.jwks_cache.read().await.is_some() {
            // We have a current key set and this kid genuinely is not in it.
            Err(AuthError::Unauthorized)
        } else {
            // No cache exists and the refresh failed or was throttled. That is
            // an identity-provider availability problem, not bad caller auth.
            Err(AuthError::Unavailable(
                "Supabase signing keys are temporarily unavailable".to_string(),
            ))
        }
    }

    /// Refresh unless one was attempted within the last
    /// [`JWKS_MIN_REFRESH_INTERVAL`]. `Ok(true)` means a fetch ran and the
    /// caller should re-check the cache; `Ok(false)` means it was throttled.
    async fn try_refresh_jwks(&self) -> Result<bool, AuthError> {
        // Single-flight: whoever holds this lock does the fetch; everyone else
        // queues behind it and then sees a warm cache.
        let _refresh_guard = self.jwks_refresh_lock.lock().await;
        {
            // Reserve the refresh slot under the write lock and bail out
            // (without an HTTP call) if another task refreshed recently.
            let mut last = self.jwks_last_refresh.write().await;
            if let Some(at) = *last
                && at.elapsed() < JWKS_MIN_REFRESH_INTERVAL
            {
                return Ok(false);
            }
            *last = Some(Instant::now());
        }
        self.refresh_jwks().await?;
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

    async fn refresh_jwks(&self) -> Result<(), AuthError> {
        let jwks_url =
            self.config.jwks_url.as_deref().ok_or_else(|| {
                AuthError::Unavailable("Supabase JWKS URL is not configured".into())
            })?;
        let response = self.http.get(jwks_url).send().await.map_err(|err| {
            error!(error = %err, "Supabase JWKS fetch failed");
            AuthError::Unavailable("Supabase JWKS fetch failed".into())
        })?;
        if !response.status().is_success() {
            return Err(AuthError::Unavailable(format!(
                "Supabase JWKS fetch returned status {}",
                response.status().as_u16()
            )));
        }
        let set = response.json::<JwkSet>().await.map_err(|err| {
            error!(error = %err, "Supabase JWKS decode failed");
            AuthError::Unavailable("Supabase JWKS response was invalid".into())
        })?;
        if set.keys.is_empty() {
            return Err(AuthError::Unavailable(
                "Supabase JWKS did not contain any signing keys".into(),
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

/// Reject key-confusion inputs even when a compromised or misconfigured issuer
/// serves multiple key types under one `kid`. Supabase signing keys declare the
/// algorithm they are meant to sign with; `use` is optional in JWK but, when
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

/// `none` and every algorithm Supabase does not issue are excluded by
/// construction. Widening this set widens the verification surface for no
/// benefit.
pub fn is_supported_supabase_algorithm(algorithm: Algorithm) -> bool {
    matches!(
        algorithm,
        Algorithm::HS256 | Algorithm::RS256 | Algorithm::ES256
    )
}

/// Extract a bearer token from an `Authorization` header value.
///
/// Unlike the static-bearer comparison in [`crate::api::auth`], the scheme match
/// is ASCII-case-insensitive per RFC 7235 and surrounding whitespace is
/// tolerated, because real Supabase clients across several SDKs produce all of
/// these spellings.
pub fn bearer_token(header: Option<&str>) -> Option<&str> {
    let raw = header?.trim();
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
    use jsonwebtoken::{EncodingKey, Header};
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Throwaway 2048-bit RSA key, generated for this test module only and
    /// never used to sign anything outside it.
    const TEST_RSA_PEM: &str = concat!(
        "-----BEGIN PRIVATE KEY-----\n",
        "MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQDQmEo4shqqTtu0\n",
        "v0921VCR5/IUnVB9UW+MXqqTyKGOtFbIy8e/u9roGc90MMZDeWRcwsrYjanESxUe\n",
        "A7WiY+EOzRRoMpNXyCkVddCme/WAHu03uCwpHkqH2+5FQJg0Z6ymiI6KIZqXTqTa\n",
        "RWcVVozvU7ZkeDEyW4o13oszfPsIMglIZfD4m+ZuqklSi24dwSk63lhskbpwy4zO\n",
        "BX3W9vt0a6mYLJUoL0gsTJXAG3V6vj98Aij5YmKiBAQTqr+pertpWvUVpcIUNW7t\n",
        "dgPNdY9kxV/gZF7Cwqk8LhL8inhfUE8icNii4VnV/OebYYBTpF35ZW/Qiji6uzDl\n",
        "g1nYJIXHAgMBAAECggEAMTDot+wQa79DAqHm/HAbmvzm0sOyeosc0i507Xrv1nZi\n",
        "zOF5jpafbgLAMHdcGgdjipiJO35U3ayTI0veppgFN/dW66eZpabXQW9SMCClaXxh\n",
        "lrVA/MVe8CEGVKCCBEG6rl8ftiVsjmgGak7Zm2eYvsuwBItvlp+NvVxo2VCM5oZx\n",
        "MyxVVzHTS3QVKTlY/PPZQ5dnIGJtnR8MYaHoaewouIWH3Cl3HetuYORrTlu8KuFF\n",
        "QLBnRtkNNTivfgCk0NxUcjrW6J6qrlSBZyhY2WpEe3M+PZJKxEZRQyCeudtvohfj\n",
        "339hvBntC4V1uIzxgal4UOHGkjtF1KbKmKzvYGrUaQKBgQDsWiFiPiw+SWQHxRL/\n",
        "VDvAankEkFl0SpxeuO2yoYL9X+QxIHlco5WoKHLxcsofU/t+xwbogQtOWN3AfLDH\n",
        "UqTH4QKLsf+m8EONzM9t0NQAMtq71tXPWuQW9rUXXWf7mYpmivpS9nF8XpkTSexC\n",
        "hRYJ4BDa7T6NgzZedGsCTVl+owKBgQDh73Obo72M3BZkUNqBiG0sIcxDJoSJJG8g\n",
        "Sijv3kQguKSu/EqQPFaAfOvgL3OmHP9Gtk/x/BHn+NCVAg/WfXpHou0bO04x2Zn+\n",
        "fCOYjuhjwHCNENpV/Bnpfb5Bw1/iARjyD1m/C0XSnAFNJ2xtpp66OmQcf79JOWWY\n",
        "KT9M1fmCjQKBgQCE0ATwatV7zsvaHeEV/2RwNKR6bw8FbSO/ipVvipjL/oWBIalw\n",
        "6C+hxdEJYqK3xf6N+BMmtdT/mqpJjwfbidI0y3kdvNFXIq4jUZLCN9XZoroNUaTm\n",
        "F0ISsWGDlqZm2JnQE4qk8f1FkPbdwu1zV8vRksqF60j6RmBX5X14VrTSlwKBgQDH\n",
        "iOkJ4H8r4senyrxfP7RjEGpMN70/PT0jQDuNNDfoygkvPUNAxPkEOs86S84QO3W7\n",
        "5pEOPjc2LklP/+Uq4eBXWe2bajHx1qKo3Mu3FSbpye/ctbCN1bqwukuH2ttYRu3Y\n",
        "AXSaQ4NjsEF5+UJKSKfQAnedr7ipG5a83li4LBVSlQKBgGPPE9LVsz9PHUr+SFmO\n",
        "n2gvR6Ti3My37wnFjvgeuA/KCYzIFoUosBKDI9wMHV5C6dIYs5DpcXQwq1GohpnA\n",
        "srz9RoZGaeV6A7WQ3l0dB6mVg/JcOs0PsSec9Qjlv8cZmfYbCyeHqgQM20vVYHPI\n",
        "3HZMLeXCAT203cMvG8aeQtn5\n",
        "-----END PRIVATE KEY-----\n",
    );

    /// base64url modulus of the key above.
    const TEST_RSA_N: &str = "0JhKOLIaqk7btL9PdtVQkefyFJ1QfVFvjF6qk8ihjrRWyMvHv7va6BnPdDDGQ3lkXMLK2I2pxEsVHgO1omPhDs0UaDKTV8gpFXXQpnv1gB7tN7gsKR5Kh9vuRUCYNGespoiOiiGal06k2kVnFVaM71O2ZHgxMluKNd6LM3z7CDIJSGXw-JvmbqpJUotuHcEpOt5YbJG6cMuMzgV91vb7dGupmCyVKC9ILEyVwBt1er4_fAIo-WJiogQEE6q_qXq7aVr1FaXCFDVu7XYDzXWPZMVf4GRewsKpPC4S_Ip4X1BPInDYouFZ1fznm2GAU6Rd-WVv0Io4ursw5YNZ2CSFxw";

    const TEST_KID: &str = "test-key-1";
    const ISSUER: &str = "https://proj.supabase.co/auth/v1";
    const TENANT_A: &str = "11111111-1111-4111-8111-111111111111";
    const TENANT_B: &str = "22222222-2222-4222-8222-222222222222";

    fn now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    fn jwks_body() -> String {
        json!({
            "keys": [{
                "kty": "RSA",
                "kid": TEST_KID,
                "use": "sig",
                "alg": "RS256",
                "n": TEST_RSA_N,
                "e": "AQAB",
            }]
        })
        .to_string()
    }

    /// Minimal JWKS server that counts how many times it was fetched. Real
    /// enough for `reqwest`, small enough not to pull in a test-only HTTP dep.
    struct JwksServer {
        url: String,
        hits: Arc<AtomicUsize>,
    }

    async fn spawn_jwks_server() -> JwksServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let counter = counter.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 2048];
                    let _ = sock.read(&mut buf).await;
                    counter.fetch_add(1, Ordering::SeqCst);
                    let body = jwks_body();
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                    let _ = sock.flush().await;
                });
            }
        });
        JwksServer {
            url: format!("http://{addr}/auth/v1/.well-known/jwks.json"),
            hits,
        }
    }

    fn config_for(jwks_url: Option<String>, jwt_secret: Option<String>) -> SupabaseConfig {
        SupabaseConfig {
            url: Some("https://proj.supabase.co".into()),
            audience: "authenticated".into(),
            issuer: Some(ISSUER.into()),
            jwks_url,
            jwt_secret,
        }
    }

    /// Build an RS256 token. Every knob a test might want to corrupt is a
    /// parameter, so each negative test differs from the happy path by exactly
    /// one thing.
    fn rs256_token(kid: &str, iss: &str, aud: &str, exp_offset: i64, nbf_offset: i64) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(kid.to_string());
        let claims = json!({
            "sub": "user-abc",
            "email": "operator@example.com",
            "role": "authenticated",
            "iss": iss,
            "aud": aud,
            "exp": now() + exp_offset,
            "nbf": now() + nbf_offset,
            "iat": now(),
            "app_metadata": { "tenant_ids": [TENANT_A] },
        });
        jsonwebtoken::encode(
            &header,
            &claims,
            &EncodingKey::from_rsa_pem(TEST_RSA_PEM.as_bytes()).unwrap(),
        )
        .unwrap()
    }

    fn valid_token() -> String {
        rs256_token(TEST_KID, ISSUER, "authenticated", 3600, -60)
    }

    // --- Happy path + caching -------------------------------------------------

    #[tokio::test]
    async fn a_valid_token_yields_the_identity_and_its_tenants() {
        let server = spawn_jwks_server().await;
        let verifier = SupabaseVerifier::from_config(&config_for(Some(server.url.clone()), None))
            .expect("configured");

        let identity = verifier.verify(&valid_token()).await.unwrap();
        assert_eq!(identity.subject, "user-abc");
        assert_eq!(identity.email.as_deref(), Some("operator@example.com"));
        assert_eq!(identity.role.as_deref(), Some("authenticated"));
        assert_eq!(
            identity.tenant_ids,
            vec![Uuid::parse_str(TENANT_A).unwrap()]
        );
        assert!(identity.is_entitled_to(Uuid::parse_str(TENANT_A).unwrap()));
        assert!(!identity.is_entitled_to(Uuid::parse_str(TENANT_B).unwrap()));
    }

    #[tokio::test]
    async fn the_cache_hit_path_does_not_refetch_the_jwks() {
        let server = spawn_jwks_server().await;
        let verifier = SupabaseVerifier::from_config(&config_for(Some(server.url.clone()), None))
            .expect("configured");

        for _ in 0..5 {
            verifier.verify(&valid_token()).await.unwrap();
        }
        assert_eq!(
            server.hits.load(Ordering::SeqCst),
            1,
            "a warm cache must serve every subsequent verification"
        );
    }

    #[tokio::test]
    async fn an_unknown_kid_triggers_exactly_one_refresh() {
        let server = spawn_jwks_server().await;
        let verifier = SupabaseVerifier::from_config(&config_for(Some(server.url.clone()), None))
            .expect("configured");

        // Each of these misses the cache. Without the rate limit, each would
        // cost one outbound JWKS fetch — the amplification this guards against.
        for _ in 0..8 {
            let token = rs256_token("kid-does-not-exist", ISSUER, "authenticated", 3600, -60);
            assert_eq!(verifier.verify(&token).await, Err(AuthError::Unauthorized));
        }
        assert_eq!(
            server.hits.load(Ordering::SeqCst),
            1,
            "an unknown-kid flood must not amplify into a JWKS fetch flood"
        );
    }

    #[tokio::test]
    async fn concurrent_cold_misses_are_single_flighted() {
        let server = spawn_jwks_server().await;
        let verifier = Arc::new(
            SupabaseVerifier::from_config(&config_for(Some(server.url.clone()), None))
                .expect("configured"),
        );

        let mut handles = Vec::new();
        for _ in 0..16 {
            let v = verifier.clone();
            handles.push(tokio::spawn(async move { v.verify(&valid_token()).await }));
        }
        for h in handles {
            h.await
                .unwrap()
                .expect("all concurrent verifications succeed");
        }
        assert_eq!(
            server.hits.load(Ordering::SeqCst),
            1,
            "concurrent cold misses must share one refresh, not stampede"
        );
    }

    // --- Claim pinning --------------------------------------------------------

    #[tokio::test]
    async fn an_expired_token_is_rejected() {
        let server = spawn_jwks_server().await;
        let verifier = SupabaseVerifier::from_config(&config_for(Some(server.url.clone()), None))
            .expect("configured");
        // Well past the 30s skew allowance.
        let token = rs256_token(TEST_KID, ISSUER, "authenticated", -3600, -7200);
        assert_eq!(verifier.verify(&token).await, Err(AuthError::Unauthorized));
    }

    #[tokio::test]
    async fn a_not_yet_valid_token_is_rejected() {
        let server = spawn_jwks_server().await;
        let verifier = SupabaseVerifier::from_config(&config_for(Some(server.url.clone()), None))
            .expect("configured");
        let token = rs256_token(TEST_KID, ISSUER, "authenticated", 7200, 3600);
        assert_eq!(verifier.verify(&token).await, Err(AuthError::Unauthorized));
    }

    #[tokio::test]
    async fn a_token_from_another_supabase_project_is_rejected() {
        let server = spawn_jwks_server().await;
        let verifier = SupabaseVerifier::from_config(&config_for(Some(server.url.clone()), None))
            .expect("configured");
        // Identical in every way except `iss`. Since `aud` is "authenticated"
        // on every Supabase project, `iss` is the only thing standing between
        // us and every other project's users.
        let token = rs256_token(
            TEST_KID,
            "https://someone-else.supabase.co/auth/v1",
            "authenticated",
            3600,
            -60,
        );
        assert_eq!(verifier.verify(&token).await, Err(AuthError::Unauthorized));
    }

    #[tokio::test]
    async fn a_token_with_the_wrong_audience_is_rejected() {
        let server = spawn_jwks_server().await;
        let verifier = SupabaseVerifier::from_config(&config_for(Some(server.url.clone()), None))
            .expect("configured");
        let token = rs256_token(TEST_KID, ISSUER, "some-other-service", 3600, -60);
        assert_eq!(verifier.verify(&token).await, Err(AuthError::Unauthorized));
    }

    // --- Algorithm attacks ----------------------------------------------------

    #[tokio::test]
    async fn an_alg_none_token_is_rejected() {
        let server = spawn_jwks_server().await;
        let verifier = SupabaseVerifier::from_config(&config_for(Some(server.url.clone()), None))
            .expect("configured");

        // Hand-assemble `{"alg":"none"}` with an empty signature — the classic
        // unsigned-token forgery. jsonwebtoken cannot even parse the header
        // into an Algorithm, which is exactly the desired outcome.
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = b64.encode(br#"{"alg":"none","typ":"JWT"}"#);
        let claims = b64.encode(
            json!({
                "sub": "attacker",
                "iss": ISSUER,
                "aud": "authenticated",
                "exp": now() + 3600,
                "app_metadata": { "tenant_ids": [TENANT_A] },
            })
            .to_string()
            .as_bytes(),
        );
        let forged = format!("{header}.{claims}.");
        assert_eq!(verifier.verify(&forged).await, Err(AuthError::Unauthorized));
    }

    #[tokio::test]
    async fn an_hs256_token_signed_with_the_rsa_public_key_is_rejected() {
        let server = spawn_jwks_server().await;
        // JWKS-only project: no symmetric secret configured, which is the
        // recommended production posture.
        let verifier = SupabaseVerifier::from_config(&config_for(Some(server.url.clone()), None))
            .expect("configured");

        // The canonical algorithm-confusion attack: take the *public* key the
        // JWKS hands out, and use its bytes as an HMAC secret. A verifier that
        // picks its key by `kid` without pinning the algorithm will happily
        // validate this and hand the attacker any identity they typed in.
        let mut header = Header::new(Algorithm::HS256);
        header.kid = Some(TEST_KID.to_string());
        let claims = json!({
            "sub": "attacker",
            "iss": ISSUER,
            "aud": "authenticated",
            "exp": now() + 3600,
            "app_metadata": { "tenant_ids": [TENANT_A, TENANT_B] },
        });
        let forged = jsonwebtoken::encode(
            &header,
            &claims,
            &EncodingKey::from_secret(TEST_RSA_N.as_bytes()),
        )
        .unwrap();

        assert_eq!(verifier.verify(&forged).await, Err(AuthError::Unauthorized));
        assert_eq!(
            server.hits.load(Ordering::SeqCst),
            0,
            "the HS256 path must never consult the JWKS at all"
        );
    }

    #[tokio::test]
    async fn hs256_is_accepted_only_when_the_legacy_secret_is_configured() {
        // Same token, two configs — the only difference is whether the operator
        // opted into the legacy symmetric secret.
        let mut header = Header::new(Algorithm::HS256);
        header.kid = Some(TEST_KID.to_string());
        let claims = json!({
            "sub": "legacy-user",
            "iss": ISSUER,
            "aud": "authenticated",
            "exp": now() + 3600,
            "app_metadata": { "tenant_id": TENANT_B },
        });
        let token = jsonwebtoken::encode(
            &header,
            &claims,
            &EncodingKey::from_secret(b"legacy-secret"),
        )
        .unwrap();

        let without = SupabaseVerifier::from_config(&config_for(
            Some("http://127.0.0.1:1/jwks".into()),
            None,
        ))
        .expect("configured");
        assert_eq!(without.verify(&token).await, Err(AuthError::Unauthorized));

        let with = SupabaseVerifier::from_config(&config_for(None, Some("legacy-secret".into())))
            .expect("configured");
        let identity = with.verify(&token).await.unwrap();
        assert_eq!(identity.subject, "legacy-user");
        assert_eq!(
            identity.tenant_ids,
            vec![Uuid::parse_str(TENANT_B).unwrap()]
        );
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
        assert!(!is_supported_supabase_algorithm(Algorithm::ES384));
    }

    #[test]
    fn a_jwks_key_must_match_the_token_algorithm_and_declare_signature_use() {
        let signing_rsa: Jwk = serde_json::from_str(
            r#"{"kty":"RSA","kid":"k","use":"sig","alg":"RS256","n":"AQ","e":"AQAB"}"#,
        )
        .unwrap();
        assert!(jwk_is_usable_for_algorithm(&signing_rsa, Algorithm::RS256));
        assert!(!jwk_is_usable_for_algorithm(&signing_rsa, Algorithm::ES256));

        // An encryption key must never be pressed into service as a signing key.
        let encryption_rsa: Jwk = serde_json::from_str(
            r#"{"kty":"RSA","kid":"k","use":"enc","alg":"RS256","n":"AQ","e":"AQAB"}"#,
        )
        .unwrap();
        assert!(!jwk_is_usable_for_algorithm(
            &encryption_rsa,
            Algorithm::RS256
        ));
    }

    // --- Availability ---------------------------------------------------------

    #[tokio::test]
    async fn an_unreachable_jwks_is_reported_as_unavailable_not_unauthorized() {
        // A cold cache plus a dead identity provider is *our* failure, and
        // must not be reported to the caller as bad credentials.
        let verifier = SupabaseVerifier::from_config(&config_for(
            // Port 1 on loopback: nothing listens there.
            Some("http://127.0.0.1:1/jwks".into()),
            None,
        ))
        .expect("configured");
        assert!(matches!(
            verifier.verify(&valid_token()).await,
            Err(AuthError::Unavailable(_))
        ));
    }

    // --- Config ---------------------------------------------------------------

    #[test]
    fn a_verifier_refuses_to_enable_without_a_pinned_issuer() {
        let mut unpinned = config_for(Some("https://x/jwks".into()), None);
        unpinned.issuer = None;
        assert!(
            SupabaseVerifier::from_config(&unpinned).is_none(),
            "an unpinned issuer admits tokens from every Supabase project"
        );
    }

    #[test]
    fn a_verifier_refuses_to_enable_without_any_way_to_check_a_signature() {
        let mut no_keys = config_for(None, None);
        no_keys.jwks_url = None;
        assert!(SupabaseVerifier::from_config(&no_keys).is_none());
    }

    #[test]
    fn derived_urls_match_the_hosted_supabase_layout() {
        assert_eq!(
            SupabaseConfig::jwks_url_for("https://proj.supabase.co"),
            "https://proj.supabase.co/auth/v1/.well-known/jwks.json"
        );
        // A trailing slash must not produce a double slash.
        assert_eq!(
            SupabaseConfig::jwks_url_for("https://proj.supabase.co/"),
            "https://proj.supabase.co/auth/v1/.well-known/jwks.json"
        );
        assert_eq!(
            SupabaseConfig::issuer_for("https://proj.supabase.co"),
            "https://proj.supabase.co/auth/v1"
        );
    }

    #[test]
    fn the_debug_surface_never_prints_the_jwt_secret() {
        let config = config_for(None, Some("super-secret-value".into()));
        let rendered = format!("{config:?}");
        assert!(!rendered.contains("super-secret-value"));
        assert!(rendered.contains("<redacted>"));

        let verifier = SupabaseVerifier::from_config(&config).expect("configured");
        assert!(!format!("{verifier:?}").contains("super-secret-value"));
    }

    // --- Claim parsing --------------------------------------------------------

    #[test]
    fn tenant_ids_are_read_from_app_metadata_only() {
        // `user_metadata` is writable by the user's own access token. If tenant
        // membership were read from there, any user could grant themselves any
        // tenant — the IDOR would simply move rather than close.
        let claims: SupabaseClaims = serde_json::from_str(&format!(
            r#"{{"sub":"u","user_metadata":{{"tenant_ids":["{TENANT_A}"]}}}}"#
        ))
        .unwrap();
        assert!(claims.tenant_ids().is_empty());

        let claims: SupabaseClaims = serde_json::from_str(&format!(
            r#"{{"sub":"u","app_metadata":{{"tenant_ids":["{TENANT_A}"]}}}}"#
        ))
        .unwrap();
        assert_eq!(
            claims.tenant_ids(),
            vec![Uuid::parse_str(TENANT_A).unwrap()]
        );
    }

    #[test]
    fn both_the_singular_and_plural_tenant_claims_are_honoured() {
        let claims: SupabaseClaims = serde_json::from_str(&format!(
            r#"{{"sub":"u","app_metadata":{{"tenant_id":"{TENANT_A}","tenant_ids":["{TENANT_B}","{TENANT_A}"]}}}}"#
        ))
        .unwrap();
        let ids = claims.tenant_ids();
        assert_eq!(ids.len(), 2, "duplicates must be collapsed: {ids:?}");
        assert!(ids.contains(&Uuid::parse_str(TENANT_A).unwrap()));
        assert!(ids.contains(&Uuid::parse_str(TENANT_B).unwrap()));
    }

    #[test]
    fn a_malformed_tenant_claim_narrows_access_rather_than_widening_it() {
        let claims: SupabaseClaims = serde_json::from_str(&format!(
            r#"{{"sub":"u","app_metadata":{{"tenant_ids":["not-a-uuid","{TENANT_A}","*"]}}}}"#
        ))
        .unwrap();
        // The junk entries are dropped; they must never become a wildcard.
        assert_eq!(
            claims.tenant_ids(),
            vec![Uuid::parse_str(TENANT_A).unwrap()]
        );
    }

    #[test]
    fn unknown_claims_do_not_break_deserialization() {
        // Supabase adds claims over time; an unrecognised one must never fail
        // an otherwise-valid login.
        let claims: SupabaseClaims =
            serde_json::from_str(r#"{"sub":"u","some_future_claim":{"a":1},"aal":"aal1"}"#)
                .unwrap();
        assert_eq!(claims.sub, "u");
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
