//! Environment-driven service configuration.
//!
//! Every knob has a `DAEDALUS_API_` prefix so this service's settings can never
//! be confused with a co-located service's when both are templated into the
//! same Kubernetes manifest.

use std::env;

#[derive(Debug, Clone)]
pub(crate) struct ServiceConfig {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) supabase: SupabaseConfig,
}

impl ServiceConfig {
    pub(crate) fn from_env() -> Result<Self, std::num::ParseIntError> {
        Ok(Self {
            host: env_value("HOST", "0.0.0.0"),
            port: env_value("PORT", "8114").parse::<u16>()?,
            supabase: SupabaseConfig::from_env(),
        })
    }
}

/// Supabase access-token verification settings.
///
/// `allowed_emails` is the org's access gate: the Daedalus surfaces are
/// single-operator today, so a token that verifies cryptographically is still
/// rejected unless its `email` claim is on the list. An empty list means the
/// gate is unconfigured, which [`SupabaseConfig::is_enabled`] treats as
/// auth-disabled rather than allow-all — failing closed on misconfiguration.
#[derive(Debug, Clone)]
pub(crate) struct SupabaseConfig {
    pub(crate) audience: String,
    pub(crate) issuer: Option<String>,
    pub(crate) jwt_secret: Option<String>,
    pub(crate) jwks_url: Option<String>,
    pub(crate) allowed_emails: Vec<String>,
}

impl SupabaseConfig {
    pub(crate) fn from_env() -> Self {
        Self {
            audience: env_value("DAEDALUS_API_SUPABASE_AUDIENCE", "authenticated"),
            issuer: optional_env("DAEDALUS_API_SUPABASE_ISSUER"),
            jwt_secret: optional_env("DAEDALUS_API_SUPABASE_JWT_SECRET"),
            jwks_url: optional_env("DAEDALUS_API_SUPABASE_JWKS_URL"),
            allowed_emails: optional_env("DAEDALUS_API_ALLOWED_EMAILS")
                .map(|raw| {
                    raw.split(',')
                        .map(|entry| entry.trim().to_ascii_lowercase())
                        .filter(|entry| !entry.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// Auth is live only when a verification key AND an allow-list are both
    /// configured. Verifying a signature without gating on identity would let
    /// any Supabase user in the project through.
    pub(crate) fn is_enabled(&self) -> bool {
        (self.jwt_secret.is_some() || self.jwks_url.is_some()) && !self.allowed_emails.is_empty()
    }

    /// Case-insensitive allow-list membership. Callers must pass an email that
    /// came from a verified token, never a client-asserted header.
    pub(crate) fn permits(&self, email: Option<&str>) -> bool {
        match email {
            Some(email) => {
                let email = email.trim().to_ascii_lowercase();
                self.allowed_emails.iter().any(|allowed| *allowed == email)
            }
            // A token with no email claim can never satisfy an email gate.
            None => false,
        }
    }
}

pub(crate) fn env_value(key: &str, fallback: &str) -> String {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

pub(crate) fn env_bool(key: &str, fallback: bool) -> bool {
    env::var(key)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(fallback)
}

pub(crate) fn env_u64(key: &str, fallback: u64, min: u64, max: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(fallback)
        .clamp(min, max)
}

pub(crate) fn optional_env(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with(allowed: &[&str], secret: Option<&str>) -> SupabaseConfig {
        SupabaseConfig {
            audience: "authenticated".to_string(),
            issuer: None,
            jwt_secret: secret.map(str::to_string),
            jwks_url: None,
            allowed_emails: allowed.iter().map(|e| e.to_string()).collect(),
        }
    }

    #[test]
    fn env_helpers_fall_back_when_unset() {
        assert_eq!(env_value("DAEDALUS_MISSING_TEST_VALUE", "fallback"), "fallback");
        assert!(!env_bool("DAEDALUS_MISSING_TEST_BOOL", false));
        assert_eq!(env_u64("DAEDALUS_MISSING_TEST_U64", 8, 1, 128), 8);
        assert_eq!(optional_env("DAEDALUS_MISSING_TEST_OPT"), None);
    }

    #[test]
    fn auth_requires_both_a_key_and_an_allow_list() {
        // A key with no allow-list would authenticate any project user.
        assert!(!config_with(&[], Some("secret")).is_enabled());
        // An allow-list with no key cannot verify anything.
        assert!(!config_with(&["a@b.com"], None).is_enabled());
        assert!(config_with(&["a@b.com"], Some("secret")).is_enabled());
    }

    #[test]
    fn allow_list_is_case_insensitive_and_rejects_missing_emails() {
        let config = config_with(&["alexander.d.mills@gmail.com"], Some("secret"));
        assert!(config.permits(Some("alexander.d.mills@gmail.com")));
        assert!(config.permits(Some("  Alexander.D.Mills@GMAIL.com  ")));
        assert!(!config.permits(Some("someone.else@gmail.com")));
        assert!(!config.permits(None));
        // Substring/prefix confusion must not pass the gate.
        assert!(!config.permits(Some("alexander.d.mills@gmail.com.evil.tld")));
        assert!(!config.permits(Some("alexander.d.mills@gmail.co")));
    }
}
