//! Claims for unified and delegated OreSoftware JWTs.

use serde::{Deserialize, Serialize};

/// The token this server mints. `sub` is the stable OreSoftware `shared_user_id`
/// (not a provider-specific subject), so downstream services get one identity
/// namespace regardless of the authority that authenticated the user.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OreClaims {
    /// Stable OreSoftware user id (`shared_user_id`).
    pub sub: String,
    pub iss: String,
    pub aud: String,
    /// Issued-at / expiry (unix seconds).
    pub iat: u64,
    pub exp: u64,
    pub nbf: u64,
    pub jti: String,
    /// Opaque session id used for revocation checks. Old stateless tokens may
    /// omit it during migration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sid: Option<String>,
    pub provider: String,
    pub provider_tenant: String,
    pub provider_subject: String,
    /// Compatibility aliases for current Supabase consumers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supabase_user_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    pub email_verified: bool,
    #[serde(default)]
    pub roles: Vec<String>,
    /// Numeric authentication assurance level, derived from [`Self::acr`] and
    /// always consistent with it. Retained for consumers that read `aal`
    /// rather than the OIDC `acr`; pre-MFA tokens deserialize as level 1 so a
    /// rolling deploy keeps verifying older tokens.
    #[serde(default = "default_auth_level")]
    pub aal: u8,
    /// Authentication methods used for this token. Missing legacy claims decode
    /// as an empty list and therefore never satisfy an explicit method policy.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub amr: Vec<String>,
    /// Authentication context. Missing legacy claims fail closed for LOA2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acr: Option<String>,
    /// Time of the user authentication ceremony. Delegation preserves this
    /// value instead of making a token exchange look like a fresh step-up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_time: Option<u64>,
    /// Space-delimited OAuth scopes. Base identity tokens intentionally carry
    /// no product scopes; delegated tokens receive only an allow-listed subset.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub scope: String,
    /// OAuth authorized party. Present only on a delegated product token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub azp: Option<String>,
    /// Parent token identifier. This provides lineage without embedding or
    /// logging the parent bearer token itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_jti: Option<String>,
}

impl OreClaims {
    pub fn has_acr(&self, required: &str) -> bool {
        self.acr.as_deref() == Some(required)
    }

    pub fn used_method(&self, method: &str) -> bool {
        self.amr.iter().any(|candidate| candidate == method)
    }

    pub fn has_scope(&self, required: &str) -> bool {
        self.scope
            .split_ascii_whitespace()
            .any(|candidate| candidate == required)
    }

    pub fn is_delegated(&self) -> bool {
        self.azp.is_some() || self.parent_jti.is_some() || !self.scope.is_empty()
    }
}

fn default_auth_level() -> u8 {
    1
}
