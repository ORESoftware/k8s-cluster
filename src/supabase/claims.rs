//! Supabase token claims and the verified identity we extract from them.

use serde::Deserialize;

/// The subset of a Supabase access token we read. Supabase signs these; we only
/// trust fields *after* the signature verifies.
#[derive(Debug, Deserialize)]
pub struct SupabaseClaims {
    /// Supabase user id (a UUID string).
    pub sub: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub phone: Option<String>,
    /// Present on current tokens; older tokens carry it under `user_metadata`.
    #[serde(default)]
    pub email_verified: Option<bool>,
    /// Supabase role, e.g. `authenticated`.
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub user_metadata: Option<serde_json::Value>,
    #[serde(default)]
    pub app_metadata: Option<serde_json::Value>,
}

impl SupabaseClaims {
    /// Whether Supabase confirmed the caller controls this address. Reads the
    /// top-level claim and the legacy `user_metadata.email_verified`.
    pub fn email_is_confirmed(&self) -> bool {
        if self.email_verified == Some(true) {
            return true;
        }
        self.user_metadata
            .as_ref()
            .and_then(|m| m.get("email_verified"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    }
}

/// A cryptographically verified Supabase identity, tagged with which project
/// vouched for it. This is the hand-off type between [`crate::supabase`] and the
/// rest of the server; constructing it is the authentication event.
#[derive(Clone, Debug)]
pub struct VerifiedIdentity {
    /// The configured project slug that signed the token (e.g. `fiducia-cloud`).
    pub project: String,
    /// Supabase user id (`sub`).
    pub supabase_user_id: String,
    pub email: Option<String>,
    pub email_verified: bool,
    pub phone: Option<String>,
    pub role: Option<String>,
    pub user_metadata: serde_json::Value,
    pub app_metadata: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_confirmation_reads_toplevel_and_legacy_metadata() {
        let top: SupabaseClaims = serde_json::from_value(serde_json::json!({
            "sub": "x", "email_verified": true
        }))
        .unwrap();
        assert!(top.email_is_confirmed());

        let legacy: SupabaseClaims = serde_json::from_value(serde_json::json!({
            "sub": "x", "user_metadata": { "email_verified": true }
        }))
        .unwrap();
        assert!(legacy.email_is_confirmed());

        let neither: SupabaseClaims =
            serde_json::from_value(serde_json::json!({ "sub": "x" })).unwrap();
        assert!(!neither.email_is_confirmed());
    }
}
