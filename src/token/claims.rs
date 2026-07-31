//! Claims for the unified OreSoftware JWT.

use serde::{Deserialize, Serialize};

pub const ACR_BASE: &str = "urn:oresoftware:loa:1";
pub const ACR_STEP_UP: &str = "urn:oresoftware:loa:2";

/// The token this server mints. `sub` is the stable OreSoftware `shared_user_id`
/// (not the Supabase `sub`), so downstream services get one identity namespace
/// regardless of which provider project the user came from.
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
    /// Authentication Methods References. Legacy tokens decode to an empty list
    /// and therefore never satisfy an explicit high-assurance policy.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub amr: Vec<String>,
    /// Authentication Context Class Reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acr: Option<String>,
}

impl OreClaims {
    pub fn has_acr(&self, required: &str) -> bool {
        self.acr.as_deref() == Some(required)
    }

    pub fn used_method(&self, method: &str) -> bool {
        self.amr.iter().any(|candidate| candidate == method)
    }
}
