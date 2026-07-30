//! Claims for the unified OreSoftware JWT.

use serde::{Deserialize, Serialize};

/// The token this server mints. `sub` is the stable OreSoftware `shared_user_id`
/// (not the Supabase `sub`), so downstream services get one identity namespace
/// regardless of which Supabase project the user came from.
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
    /// Authentication assurance level. Existing pre-MFA tokens deserialize as
    /// AAL1 for rolling-deploy compatibility.
    #[serde(default = "default_auth_level")]
    pub aal: u8,
    #[serde(default)]
    pub amr: Vec<String>,
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
}

fn default_auth_level() -> u8 {
    1
}
