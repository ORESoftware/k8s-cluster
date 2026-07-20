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
    /// Which Supabase project/org the identity originated from.
    pub project: String,
    /// The upstream Supabase user id, for traceability back to the source.
    pub supabase_user_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    pub email_verified: bool,
}
