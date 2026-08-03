//! Shared Auth access-token verification with a direct-Supabase migration path.
//!
//! The filename and public type names are retained for source compatibility with
//! the first per-user-auth rollout. Production configuration should use
//! `BILLING_SHARED_AUTH_*`: Shared Auth mints one ES256 identity token for every
//! OreSoftware application, and this service verifies it locally against the
//! authority's pinned issuer, audience, and JWKS.
//!
//! Authorization remains local to Quaestor Ledger. Shared Auth proves identity
//! and assurance; namespaced `roles` become tenant entitlements and billing
//! scopes only through the explicit parsing rules below.
//!
//! Security properties:
//!
//! * issuer and audience are mandatory and pinned;
//! * only ES256/RS256 JWKS keys are accepted for Shared Auth;
//! * HS256 exists only for the explicitly configured legacy Supabase path;
//! * the JWK `kid`, declared algorithm, and public-key use must all match;
//! * JWKS fetches are redirect-free, bounded, single-flight, and rate-limited;
//! * a recently stale key may be used only during an authority outage, never to
//!   satisfy an unknown `kid`;
//! * product tenancy comes only from `quaestor:tenant:<uuid>` roles (or the
//!   legacy Supabase app-metadata bridge), never from `provider_tenant`;
//! * future `auth_time` values fail closed rather than becoming age zero.

use std::env;
use std::fmt;
use std::time::{Duration, Instant};

use jsonwebtoken::{
    Algorithm, DecodingKey, Validation, decode, decode_header,
    jwk::{Jwk, JwkSet, KeyAlgorithm, PublicKeyUse},
};
use serde::Deserialize;
use tokio::sync::{Mutex as AsyncMutex, RwLock};
use tracing::{error, warn};
use uuid::Uuid;

const JWKS_CACHE_TTL: Duration = Duration::from_secs(600);
const JWKS_STALE_GRACE: Duration = Duration::from_secs(1_200);
const JWKS_MIN_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
const JWKS_HTTP_TIMEOUT: Duration = Duration::from_secs(5);
const JWKS_MAX_BYTES: usize = 256 * 1024;
const CLOCK_SKEW_LEEWAY_SECS: u64 = 30;

const SHARED_AUTH_DEFAULT_AUDIENCE: &str = "oresoftware";
const SHARED_AUTH_AAL2_ACR: &str = "urn:oresoftware:loa:2";
const QUAESTOR_TENANT_ROLE_PREFIX: &str = "quaestor:tenant:";
const QUAESTOR_BILLING_WRITE_ROLE: &str = "quaestor:billing:write";
const SCOPE_FINANCIAL_WRITE: &str = "billing:write";

/// Why a token was refused.
///
/// Cryptographic and claim failures collapse to `Unauthorized`. `Unavailable`
/// is reserved for a failure to reach or decode the configured signing-key
/// authority, allowing the HTTP layer to return 503 rather than lying with 401.
#[derive(Debug, PartialEq, Eq)]
pub enum AuthError {
    Unauthorized,
    Unavailable(String),
}

/// JWT authority wiring.
///
/// The historical field names remain so existing constructors and tests compile.
/// `effective()` overlays `BILLING_SHARED_AUTH_*` when present, making Shared
/// Auth the production authority without requiring every call site to change at
/// once. The legacy symmetric secret is deliberately discarded on that path.
#[derive(Clone, Default)]
pub struct SupabaseConfig {
    /// Authority base URL. Optional when issuer and JWKS URL are explicit.
    pub url: Option<String>,
    pub audience: String,
    pub issuer: Option<String>,
    pub jwks_url: Option<String>,
    /// Legacy direct-Supabase HS256 secret. Never used for Shared Auth.
    pub jwt_secret: Option<String>,
}

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
    pub fn jwks_url_for(base_url: &str) -> String {
        format!(
            "{}/auth/v1/.well-known/jwks.json",
            base_url.trim_end_matches('/')
        )
    }

    pub fn issuer_for(base_url: &str) -> String {
        format!("{}/auth/v1", base_url.trim_end_matches('/'))
    }

    pub fn shared_auth_jwks_url_for(base_url: &str) -> String {
        format!(
            "{}/.well-known/jwks.json",
            base_url.trim_end_matches('/')
        )
    }

    /// Shared Auth is the production default. Direct Supabase verification is
    /// accepted only under an explicit migration flag (or in tests/local
    /// insecure development), preventing an old provider-specific config from
    /// silently bypassing the centralized authority.
    pub fn is_enabled(&self) -> bool {
        if shared_auth_env_selected() {
            return self.effective().is_structurally_enabled();
        }
        let direct_migration_allowed = cfg!(test)
            || env_bool("BILLING_ALLOW_DIRECT_SUPABASE_AUTH", false)
            || env_bool("BILLING_ALLOW_INSECURE_DEV", false);
        direct_migration_allowed && self.is_structurally_enabled()
    }

    fn is_structurally_enabled(&self) -> bool {
        self.issuer
            .as_deref()
            .is_some_and(|issuer| !issuer.trim().is_empty())
            && !self.audience.trim().is_empty()
            && (self
                .jwks_url
                .as_deref()
                .is_some_and(|url| !url.trim().is_empty())
                || self
                    .jwt_secret
                    .as_deref()
                    .is_some_and(|secret| !secret.is_empty()))
    }

    /// Overlay production Shared Auth settings while retaining legacy Supabase
    /// values as an explicit migration path.
    fn effective(&self) -> Self {
        let shared_base = optional_env("BILLING_SHARED_AUTH_URL");
        let shared_issuer = optional_env("BILLING_SHARED_AUTH_ISSUER");
        let shared_jwks = optional_env("BILLING_SHARED_AUTH_JWKS_URL")
            .or_else(|| shared_base.as_deref().map(Self::shared_auth_jwks_url_for));

        if !shared_auth_env_selected() {
            return self.clone();
        }

        Self {
            url: shared_base.or_else(|| self.url.clone()),
            audience: optional_env("BILLING_SHARED_AUTH_AUDIENCE")
                .unwrap_or_else(|| SHARED_AUTH_DEFAULT_AUDIENCE.to_string()),
            issuer: shared_issuer,
            jwks_url: shared_jwks,
            // Shared Auth signs asymmetrically. Carrying a legacy HMAC secret
            // into this authority would widen the accepted algorithm set.
            jwt_secret: None,
        }
    }
}

fn optional_env(name: &str) -> Option<String> {
    env::var(name).ok().and_then(|raw| {
        let value = raw.trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

fn shared_auth_env_selected() -> bool {
    optional_env("BILLING_SHARED_AUTH_URL").is_some()
        || optional_env("BILLING_SHARED_AUTH_ISSUER").is_some()
        || optional_env("BILLING_SHARED_AUTH_JWKS_URL").is_some()
}

fn env_bool(name: &str, default: bool) -> bool {
    env::var(name)
        .map(|raw| matches!(raw.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(default)
}

/// Authenticator Assurance Level.
///
/// Anything absent, malformed, inconsistent, or unknown is AAL1. The only path
/// to AAL2 is an exact supported claim combination.
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

/// A verified caller identity used by Quaestor's authorization middleware.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SupabaseIdentity {
    pub subject: String,
    pub email: Option<String>,
    pub role: Option<String>,
    pub tenant_ids: Vec<Uuid>,
    pub assurance: Aal,
    pub scopes: Vec<String>,
    pub step_up_at: Option<u64>,
}

impl SupabaseIdentity {
    pub fn is_entitled_to(&self, tenant_id: Uuid) -> bool {
        self.tenant_ids.contains(&tenant_id)
    }

    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|granted| granted == scope)
    }

    /// Age of the trusted assurance event.
    ///
    /// A timestamp more than the JWT clock-skew allowance in the future is not
    /// "fresh"; it is invalid and therefore fails closed.
    pub fn step_up_age_secs(&self, now: u64) -> Option<u64> {
        let at = self.step_up_at?;
        if at > now.saturating_add(CLOCK_SKEW_LEEWAY_SECS) {
            return None;
        }
        Some(now.saturating_sub(at))
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum AalClaim {
    Numeric(u8),
    Text(String),
}

impl AalClaim {
    fn asserts_aal2(&self) -> bool {
        match self {
            Self::Numeric(level) => *level == 2,
            Self::Text(level) => level.trim() == "aal2",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum AmrClaim {
    Method(String),
    Detail(AmrDetail),
}

impl AmrClaim {
    fn timestamp(&self) -> Option<u64> {
        match self {
            Self::Method(_) => None,
            Self::Detail(detail) => detail.timestamp,
        }
    }

    #[allow(dead_code)]
    fn method(&self) -> Option<&str> {
        match self {
            Self::Method(method) => Some(method),
            Self::Detail(detail) => detail.method.as_deref(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct AmrDetail {
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    timestamp: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct AppMetadata {
    #[serde(default)]
    tenant_id: Option<String>,
    #[serde(default)]
    tenant_ids: Option<Vec<String>>,
    #[serde(default)]
    financial_scopes: Option<Vec<String>>,
}

/// Provider-neutral claims plus the legacy fields needed during migration.
///
/// Unknown claims are intentionally accepted so the authority can add
/// non-security metadata without breaking every service.
#[derive(Debug, Deserialize)]
struct AuthorityClaims {
    sub: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    roles: Vec<String>,
    #[serde(default)]
    aal: Option<AalClaim>,
    #[serde(default)]
    acr: Option<String>,
    #[serde(default)]
    auth_time: Option<u64>,
    #[serde(default)]
    amr: Vec<AmrClaim>,
    #[serde(default)]
    app_metadata: Option<AppMetadata>,
}

impl AuthorityClaims {
    fn assurance(&self) -> Aal {
        let aal_asserts_two = self
            .aal
            .as_ref()
            .is_some_and(AalClaim::asserts_aal2);

        match self.acr.as_deref().map(str::trim) {
            // Shared Auth emits both numeric `aal` and OIDC `acr`. Requiring
            // agreement prevents a malformed token from choosing whichever
            // representation is more permissive.
            Some(SHARED_AUTH_AAL2_ACR) if aal_asserts_two => Aal::Aal2,
            Some(_) => Aal::Aal1,
            // Direct Supabase tokens do not carry our ACR vocabulary.
            None if aal_asserts_two => Aal::Aal2,
            None => Aal::Aal1,
        }
    }

    fn normalized_roles(&self) -> Vec<&str> {
        let mut roles = Vec::new();
        for raw in &self.roles {
            let role = raw.trim();
            if valid_role(role) && !roles.contains(&role) {
                roles.push(role);
            }
        }
        roles
    }

    fn tenant_ids(&self) -> Vec<Uuid> {
        let mut out = Vec::new();

        // Canonical Shared Auth contract.
        for role in self.normalized_roles() {
            let Some(raw_tenant) = role.strip_prefix(QUAESTOR_TENANT_ROLE_PREFIX) else {
                continue;
            };
            if let Ok(tenant_id) = Uuid::parse_str(raw_tenant)
                && !out.contains(&tenant_id)
            {
                out.push(tenant_id);
            }
        }

        // Direct-Supabase migration bridge. Shared Auth tokens do not carry
        // app_metadata, so this cannot broaden the canonical path.
        if let Some(metadata) = &self.app_metadata {
            let single = metadata.tenant_id.iter().map(String::as_str);
            let many = metadata
                .tenant_ids
                .iter()
                .flatten()
                .map(String::as_str);
            for raw in single.chain(many) {
                if let Ok(tenant_id) = Uuid::parse_str(raw.trim())
                    && !out.contains(&tenant_id)
                {
                    out.push(tenant_id);
                }
            }
        }

        out
    }

    fn scopes(&self) -> Vec<String> {
        let mut out = Vec::new();

        if self
            .normalized_roles()
            .contains(&QUAESTOR_BILLING_WRITE_ROLE)
        {
            out.push(SCOPE_FINANCIAL_WRITE.to_string());
        }

        // Direct-Supabase migration bridge.
        if let Some(metadata) = &self.app_metadata {
            for raw in metadata.financial_scopes.iter().flatten() {
                let scope = raw.trim();
                if valid_scope(scope) && !out.iter().any(|existing| existing == scope) {
                    out.push(scope.to_string());
                }
            }
        }

        out
    }

    fn trusted_auth_time(&self, assurance: Aal) -> Option<u64> {
        if !assurance.is_aal2() {
            return None;
        }
        // Shared Auth emits the standard top-level claim. The AMR timestamp is
        // retained only for the direct-Supabase migration path.
        self.auth_time
            .or_else(|| self.amr.iter().filter_map(AmrClaim::timestamp).max())
    }
}

fn valid_role(role: &str) -> bool {
    !role.is_empty()
        && role.len() <= 64
        && role.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b':' | b'_' | b'-')
        })
}

fn valid_scope(scope: &str) -> bool {
    !scope.is_empty()
        && scope.len() <= 64
        && scope.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b':' | b'_' | b'-')
        })
}

struct JwksCacheEntry {
    fetched_at: Instant,
    set: JwkSet,
}

pub struct SupabaseVerifier {
    config: SupabaseConfig,
    http: reqwest::Client,
    jwks_cache: RwLock<Option<JwksCacheEntry>>,
    jwks_last_refresh: RwLock<Option<Instant>>,
    jwks_refresh_lock: AsyncMutex<()>,
}

impl fmt::Debug for SupabaseVerifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SupabaseVerifier")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl SupabaseVerifier {
    pub fn from_config(config: &SupabaseConfig) -> Option<Self> {
        if !config.is_enabled() {
            return None;
        }
        let config = config.effective();
        let http = match reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(JWKS_HTTP_TIMEOUT)
            .build()
        {
            Ok(client) => client,
            Err(error) => {
                error!(%error, "failed to construct Shared Auth JWKS client");
                return None;
            }
        };
        Some(Self::new_unchecked(config, http))
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

    fn validation(&self, algorithm: Algorithm) -> Validation {
        let mut validation = Validation::new(algorithm);
        validation.set_audience(&[self.config.audience.as_str()]);
        if let Some(issuer) = &self.config.issuer {
            validation.set_issuer(&[issuer.as_str()]);
        }
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        validation.validate_exp = true;
        validation.validate_nbf = true;
        validation.leeway = CLOCK_SKEW_LEEWAY_SECS;
        debug_assert!(self.config.issuer.is_some(), "issuer must be pinned");
        validation
    }

    pub async fn verify(&self, token: &str) -> Result<SupabaseIdentity, AuthError> {
        if token.is_empty() || token.len() > 16 * 1024 {
            return Err(AuthError::Unauthorized);
        }
        let claims = self.verify_claims(token).await?;

        let subject = claims.sub.trim().to_string();
        if subject.is_empty()
            || subject.len() > 160
            || subject
                .chars()
                .any(|character| character.is_control() || matches!(character, '/' | '\\'))
        {
            return Err(AuthError::Unauthorized);
        }

        let assurance = claims.assurance();
        let step_up_at = claims.trusted_auth_time(assurance);
        Ok(SupabaseIdentity {
            subject,
            email: claims
                .email
                .as_deref()
                .map(str::trim)
                .filter(|email| !email.is_empty() && email.len() <= 320)
                .map(str::to_string),
            role: claims
                .role
                .filter(|role| role.len() <= 64 && !role.chars().any(char::is_control)),
            tenant_ids: claims.tenant_ids(),
            assurance,
            scopes: claims.scopes(),
            step_up_at,
        })
    }

    async fn verify_claims(&self, token: &str) -> Result<AuthorityClaims, AuthError> {
        let header = decode_header(token).map_err(|_| AuthError::Unauthorized)?;
        if !is_supported_supabase_algorithm(header.alg) {
            return Err(AuthError::Unauthorized);
        }

        if header.alg == Algorithm::HS256 {
            let Some(secret) = self.config.jwt_secret.as_deref() else {
                return Err(AuthError::Unauthorized);
            };
            return decode::<AuthorityClaims>(
                token,
                &DecodingKey::from_secret(secret.as_bytes()),
                &self.validation(Algorithm::HS256),
            )
            .map(|decoded| decoded.claims)
            .map_err(|_| AuthError::Unauthorized);
        }

        let kid = header.kid.ok_or(AuthError::Unauthorized)?;
        let jwk = self.jwk_for_kid(&kid, header.alg).await?;
        let key = DecodingKey::from_jwk(&jwk).map_err(|_| AuthError::Unauthorized)?;
        decode::<AuthorityClaims>(token, &key, &self.validation(header.alg))
            .map(|decoded| decoded.claims)
            .map_err(|_| AuthError::Unauthorized)
    }

    async fn jwk_for_kid(&self, kid: &str, algorithm: Algorithm) -> Result<Jwk, AuthError> {
        if let Some(jwk) = self
            .cached_jwk(kid, algorithm, JWKS_CACHE_TTL)
            .await
        {
            return Ok(jwk);
        }

        match self.try_refresh_jwks().await {
            Ok(refreshed) => {
                if let Some(jwk) = self
                    .cached_jwk(kid, algorithm, JWKS_CACHE_TTL)
                    .await
                {
                    return Ok(jwk);
                }
                if !refreshed {
                    if let Some(jwk) = self
                        .cached_jwk(kid, algorithm, JWKS_STALE_GRACE)
                        .await
                    {
                        warn!(
                            key.id = kid,
                            "using recently stale Shared Auth signing key while refresh is throttled"
                        );
                        return Ok(jwk);
                    }
                }
                if self.jwks_cache.read().await.is_some() {
                    Err(AuthError::Unauthorized)
                } else {
                    Err(AuthError::Unavailable(
                        "identity signing keys are unavailable".to_string(),
                    ))
                }
            }
            Err(error) => {
                // Resilience without key-confusion: only a key with the same
                // kid and algorithm from the bounded stale cache may be used.
                if let Some(jwk) = self
                    .cached_jwk(kid, algorithm, JWKS_STALE_GRACE)
                    .await
                {
                    warn!(
                        key.id = kid,
                        "using recently stale Shared Auth signing key during JWKS outage"
                    );
                    return Ok(jwk);
                }
                Err(error)
            }
        }
    }

    async fn try_refresh_jwks(&self) -> Result<bool, AuthError> {
        let _guard = self.jwks_refresh_lock.lock().await;
        {
            let mut last_refresh = self.jwks_last_refresh.write().await;
            if let Some(at) = *last_refresh
                && at.elapsed() < JWKS_MIN_REFRESH_INTERVAL
            {
                return Ok(false);
            }
            *last_refresh = Some(Instant::now());
        }
        self.refresh_jwks().await?;
        Ok(true)
    }

    async fn cached_jwk(
        &self,
        kid: &str,
        algorithm: Algorithm,
        max_age: Duration,
    ) -> Option<Jwk> {
        let cache = self.jwks_cache.read().await;
        let entry = cache.as_ref()?;
        if entry.fetched_at.elapsed() > max_age {
            return None;
        }
        let jwk = entry.set.find(kid)?;
        jwk_is_usable_for_algorithm(jwk, algorithm).then(|| jwk.clone())
    }

    async fn refresh_jwks(&self) -> Result<(), AuthError> {
        let jwks_url = self.config.jwks_url.as_deref().ok_or_else(|| {
            AuthError::Unavailable("identity JWKS URL is not configured".to_string())
        })?;
        let response = self.http.get(jwks_url).send().await.map_err(|fetch_error| {
            error!(error = %fetch_error, "Shared Auth JWKS fetch failed");
            AuthError::Unavailable("identity JWKS fetch failed".to_string())
        })?;
        if !response.status().is_success() {
            return Err(AuthError::Unavailable(format!(
                "identity JWKS fetch returned status {}",
                response.status().as_u16()
            )));
        }
        if response
            .content_length()
            .is_some_and(|length| length > JWKS_MAX_BYTES as u64)
        {
            return Err(AuthError::Unavailable(
                "identity JWKS response exceeded the size limit".to_string(),
            ));
        }
        let body = response.bytes().await.map_err(|decode_error| {
            error!(error = %decode_error, "Shared Auth JWKS body read failed");
            AuthError::Unavailable("identity JWKS response was unreadable".to_string())
        })?;
        if body.len() > JWKS_MAX_BYTES {
            return Err(AuthError::Unavailable(
                "identity JWKS response exceeded the size limit".to_string(),
            ));
        }
        let set = serde_json::from_slice::<JwkSet>(&body).map_err(|decode_error| {
            error!(error = %decode_error, "Shared Auth JWKS decode failed");
            AuthError::Unavailable("identity JWKS response was invalid".to_string())
        })?;
        if set.keys.is_empty() {
            return Err(AuthError::Unavailable(
                "identity JWKS did not contain any signing keys".to_string(),
            ));
        }
        *self.jwks_cache.write().await = Some(JwksCacheEntry {
            fetched_at: Instant::now(),
            set,
        });
        Ok(())
    }
}

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

/// Historical public name retained for callers/tests.
pub fn is_supported_supabase_algorithm(algorithm: Algorithm) -> bool {
    matches!(
        algorithm,
        Algorithm::HS256 | Algorithm::RS256 | Algorithm::ES256
    )
}

/// Extract a bearer token using the RFC 7235 case-insensitive scheme rule.
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
    use serde_json::json;

    fn claims(value: serde_json::Value) -> AuthorityClaims {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn shared_auth_roles_are_the_only_canonical_product_grants() {
        let tenant = Uuid::new_v4();
        let parsed = claims(json!({
            "sub": "user-1",
            "roles": [
                format!("quaestor:tenant:{tenant}"),
                "quaestor:billing:write",
                "other-product:admin",
                "quaestor:tenant:not-a-uuid"
            ],
            "aal": 2,
            "acr": SHARED_AUTH_AAL2_ACR,
            "auth_time": 1_700_000_000u64
        }));
        assert_eq!(parsed.tenant_ids(), vec![tenant]);
        assert_eq!(parsed.scopes(), vec![SCOPE_FINANCIAL_WRITE]);
        assert_eq!(parsed.assurance(), Aal::Aal2);
        assert_eq!(
            parsed.trusted_auth_time(parsed.assurance()),
            Some(1_700_000_000)
        );
    }

    #[test]
    fn provider_tenant_is_not_a_supported_entitlement_claim() {
        let parsed = claims(json!({
            "sub": "user-1",
            "provider_tenant": Uuid::new_v4().to_string(),
            "roles": []
        }));
        assert!(parsed.tenant_ids().is_empty());
    }

    #[test]
    fn inconsistent_or_unknown_assurance_fails_closed() {
        for value in [
            json!({"sub":"u","aal":2,"acr":"urn:oresoftware:loa:1","auth_time":1}),
            json!({"sub":"u","aal":1,"acr":SHARED_AUTH_AAL2_ACR,"auth_time":1}),
            json!({"sub":"u","aal":"aal3","auth_time":1}),
            json!({"sub":"u","acr":SHARED_AUTH_AAL2_ACR,"auth_time":1}),
        ] {
            let parsed = claims(value);
            assert_eq!(parsed.assurance(), Aal::Aal1);
            assert_eq!(parsed.trusted_auth_time(parsed.assurance()), None);
        }
    }

    #[test]
    fn legacy_supabase_metadata_is_narrow_migration_bridge() {
        let tenant = Uuid::new_v4();
        let parsed = claims(json!({
            "sub": "user-1",
            "aal": "aal2",
            "amr": [{"method":"totp","timestamp":1_700_000_010u64}],
            "app_metadata": {
                "tenant_ids": [tenant.to_string(), "malformed"],
                "financial_scopes": ["billing:write", "", "<invalid>"]
            }
        }));
        assert_eq!(parsed.tenant_ids(), vec![tenant]);
        assert_eq!(parsed.scopes(), vec![SCOPE_FINANCIAL_WRITE]);
        assert_eq!(parsed.assurance(), Aal::Aal2);
        assert_eq!(
            parsed.trusted_auth_time(parsed.assurance()),
            Some(1_700_000_010)
        );
    }

    #[test]
    fn future_step_up_timestamp_fails_closed() {
        let identity = SupabaseIdentity {
            subject: "user-1".into(),
            email: None,
            role: None,
            tenant_ids: vec![],
            assurance: Aal::Aal2,
            scopes: vec![SCOPE_FINANCIAL_WRITE.into()],
            step_up_at: Some(2_000),
        };
        assert_eq!(identity.step_up_age_secs(1_000), None);
        assert_eq!(identity.step_up_age_secs(1_980), Some(0));
        assert_eq!(identity.step_up_age_secs(2_100), Some(100));
    }

    #[test]
    fn role_and_scope_grammars_are_bounded() {
        assert!(valid_role("quaestor:billing:write"));
        assert!(valid_role("quaestor:tenant:00000000-0000-0000-0000-000000000000"));
        assert!(!valid_role("Quaestor:billing:write"));
        assert!(!valid_role("quaestor/billing/write"));
        assert!(!valid_scope("<script>"));
    }

    #[test]
    fn bearer_scheme_is_case_insensitive_and_nonempty() {
        assert_eq!(bearer_token(Some("Bearer abc")), Some("abc"));
        assert_eq!(bearer_token(Some(" bearer   abc  ")), Some("abc"));
        assert_eq!(bearer_token(Some("Basic abc")), None);
        assert_eq!(bearer_token(Some("Bearer ")), None);
        assert_eq!(bearer_token(None), None);
    }

    #[test]
    fn algorithm_surface_is_deliberately_small() {
        assert!(is_supported_supabase_algorithm(Algorithm::ES256));
        assert!(is_supported_supabase_algorithm(Algorithm::RS256));
        assert!(is_supported_supabase_algorithm(Algorithm::HS256));
        assert!(!is_supported_supabase_algorithm(Algorithm::ES384));
        assert!(!is_supported_supabase_algorithm(Algorithm::RS512));
    }

    #[test]
    fn debug_redacts_legacy_symmetric_secret() {
        let debug = format!(
            "{:?}",
            SupabaseConfig {
                url: None,
                audience: "oresoftware".into(),
                issuer: Some("https://auth.example".into()),
                jwks_url: Some("https://auth.example/.well-known/jwks.json".into()),
                jwt_secret: Some("do-not-print-me".into()),
            }
        );
        assert!(!debug.contains("do-not-print-me"));
        assert!(debug.contains("<redacted>"));
    }
}
