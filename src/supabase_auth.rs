//! Compatibility shim for the former in-process Supabase verifier.
//!
//! Quaestor now authenticates through the canonical Shared Auth control plane
//! (`crate::shared_auth`). Keeping this small config type avoids a flag-day
//! change in existing deployment parsing while callers move from
//! `BILLING_SUPABASE_*` to `BILLING_SHARED_AUTH_*`. No request-path code in this
//! module verifies provider tokens anymore.

use std::fmt;

pub use crate::shared_auth::{
    Aal, AuthError, SharedAuthIdentity as SupabaseIdentity, SharedAuthVerifier as SupabaseVerifier,
    bearer_token,
};

/// Legacy deployment fields retained only so old configuration files continue
/// to deserialize during the migration. Shared Auth configuration is loaded
/// from `BILLING_SHARED_AUTH_*` by `SharedAuthVerifier::from_env`.
#[derive(Clone, Default)]
pub struct SupabaseConfig {
    pub url: Option<String>,
    pub audience: String,
    pub issuer: Option<String>,
    pub jwks_url: Option<String>,
    pub jwt_secret: Option<String>,
}

impl fmt::Debug for SupabaseConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SupabaseConfig(legacy)")
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

    /// During the rolling migration either a complete Shared Auth setup or the
    /// former direct-Supabase setup counts as configured for the legacy
    /// `Config` boot guard. `ApiAuth::from_state` performs the final fail-closed
    /// check and refuses production boot unless Shared Auth itself is present.
    pub fn is_enabled(&self) -> bool {
        let shared_auth_enabled = matches!(
            crate::shared_auth::SharedAuthConfig::from_env(),
            Ok(Some(_))
        );
        shared_auth_enabled
            || (self.url.is_some()
                && self.issuer.is_some()
                && (self.jwks_url.is_some() || self.jwt_secret.is_some()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_debug_surface_still_redacts_symmetric_secret() {
        let config = SupabaseConfig {
            url: Some("https://project.supabase.co".to_owned()),
            audience: "authenticated".to_owned(),
            issuer: Some("https://project.supabase.co/auth/v1".to_owned()),
            jwks_url: None,
            jwt_secret: Some("do-not-log-me".to_owned()),
        };
        let rendered = format!("{config:?}");
        assert!(!rendered.contains("do-not-log-me"));
        assert!(rendered.contains("<redacted>"));
    }
}
