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
    /// Supabase Authenticator Assurance Level (`aal1` or `aal2`).
    #[serde(default)]
    pub aal: Option<String>,
    /// Supabase emits AMR entries as objects containing a `method`; accepting a
    /// string as well keeps the bridge compatible with older/custom issuers.
    #[serde(default)]
    pub amr: Vec<serde_json::Value>,
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

    /// Translate the verified Supabase assurance claims into shared-auth's
    /// provider-neutral AAL/AMR representation.
    pub fn assurance(&self) -> (u8, Vec<String>) {
        let auth_level = u8::from(self.aal.as_deref() == Some("aal2")) + 1;
        let mut auth_methods = vec!["federated".to_owned()];
        for entry in &self.amr {
            let method = entry
                .as_str()
                .or_else(|| entry.get("method").and_then(serde_json::Value::as_str))
                .map(str::trim)
                .filter(|method| {
                    !method.is_empty()
                        && method.len() <= 64
                        && method
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                });
            if let Some(method) = method {
                let method = method.to_ascii_lowercase();
                if !auth_methods.contains(&method) && auth_methods.len() < 16 {
                    auth_methods.push(method);
                }
            }
        }
        if auth_level == 2 && auth_methods.len() == 1 {
            auth_methods.push("mfa".to_owned());
        }
        (auth_level, auth_methods)
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
    /// Assurance established by Supabase after its JWT signature was verified.
    pub auth_level: u8,
    pub auth_methods: Vec<String>,
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

    #[test]
    fn assurance_preserves_verified_aal_and_methods() {
        let aal2: SupabaseClaims = serde_json::from_value(serde_json::json!({
            "sub": "x",
            "aal": "aal2",
            "amr": [
                {"method": "password", "timestamp": 1_700_000_000},
                {"method": "otp", "timestamp": 1_700_000_001},
                {"method": "otp", "timestamp": 1_700_000_002}
            ]
        }))
        .unwrap();
        assert_eq!(
            aal2.assurance(),
            (
                2,
                vec![
                    "federated".to_owned(),
                    "password".to_owned(),
                    "otp".to_owned()
                ]
            )
        );

        let legacy: SupabaseClaims = serde_json::from_value(serde_json::json!({
            "sub": "x", "amr": ["oauth"]
        }))
        .unwrap();
        assert_eq!(
            legacy.assurance(),
            (1, vec!["federated".to_owned(), "oauth".to_owned()])
        );
    }
}
