//! Authentication-method and context normalization for minted shared-auth tokens.
//!
//! Assurance describes how authentication happened. It never replaces tenant,
//! role, or resource authorization checks.

use std::time::{SystemTime, UNIX_EPOCH};

pub const ACR_LOA1: &str = "urn:oresoftware:loa:1";
pub const ACR_LOA2: &str = "urn:oresoftware:loa:2";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuthenticationAssurance {
    pub amr: Vec<String>,
    pub acr: Option<String>,
    /// Unix time at which the authentication ceremony represented by this
    /// assurance completed. For LOA2 this is the second-factor/passkey time,
    /// not the time a downstream access token happened to be minted.
    pub auth_time: Option<u64>,
}

impl AuthenticationAssurance {
    pub fn local_password() -> Self {
        Self {
            amr: vec!["pwd".to_owned()],
            acr: Some(ACR_LOA1.to_owned()),
            auth_time: Some(now_secs()),
        }
    }

    /// Refresh proves possession of the rotating refresh secret, but does not
    /// silently retain a prior step-up ceremony across session rotation.
    pub fn refresh_token() -> Self {
        Self {
            amr: vec!["refresh_token".to_owned()],
            acr: Some(ACR_LOA1.to_owned()),
            auth_time: None,
        }
    }

    /// Assurance after a completed step-up ceremony on an existing session.
    ///
    /// The prior methods are carried forward so the token records the full
    /// chain (for example `pwd` then `totp`) rather than only the last hop.
    pub fn step_up(previous: &[String], method: &str) -> Self {
        let mut amr: Vec<String> = previous
            .iter()
            .filter_map(|method| normalize_method(method))
            .collect();
        if amr.is_empty() {
            amr.push("federated".to_owned());
        }
        if let Some(method) = normalize_method(method) {
            if !amr.contains(&method) {
                amr.push(method);
            }
        }
        amr.truncate(16);
        Self {
            amr,
            acr: Some(ACR_LOA2.to_owned()),
            auth_time: Some(now_secs()),
        }
    }

    /// Bridge from the numeric `(level, methods)` vocabulary used by the local
    /// password/MFA flows into the OIDC one. The caller must already have
    /// verified the ceremony; this only normalizes the representation.
    ///
    /// Level 2 maps to [`ACR_LOA2`], anything else fails closed to
    /// [`ACR_LOA1`], so the numeric and OIDC forms cannot diverge.
    pub fn from_level_and_methods(level: u8, methods: &[String]) -> Self {
        let mut amr: Vec<String> = Vec::new();
        for method in methods.iter().filter_map(|method| normalize_method(method)) {
            if !amr.contains(&method) {
                amr.push(method);
            }
            if amr.len() == 16 {
                break;
            }
        }
        if amr.is_empty() {
            amr.push("federated".to_owned());
        }
        Self {
            amr,
            acr: Some(if level >= 2 { ACR_LOA2 } else { ACR_LOA1 }.to_owned()),
            auth_time: Some(now_secs()),
        }
    }

    /// Numeric assurance level for the `aal` claim, kept in lockstep with
    /// [`Self::acr`] so the two vocabularies can never disagree. Unknown or
    /// absent ACR fails closed to level 1.
    pub fn level(&self) -> u8 {
        match self.acr.as_deref() {
            Some(ACR_LOA2) => 2,
            _ => 1,
        }
    }

    /// Normalize only claims extracted after the Supabase JWT signature,
    /// issuer, audience, and expiry have verified. Missing or unknown AAL never
    /// satisfies an explicit LOA policy.
    pub fn from_supabase(aal: Option<&str>, methods: &[String]) -> Self {
        Self::from_supabase_at(aal, methods, None)
    }

    /// Supabase bridge that also carries the latest signed AMR timestamp. A
    /// caller must derive `auth_time` from the same provider token whose
    /// signature/issuer/audience/expiry were verified; request JSON must never
    /// supply it independently.
    pub fn from_supabase_at(
        aal: Option<&str>,
        methods: &[String],
        auth_time: Option<u64>,
    ) -> Self {
        let mut amr = vec!["federated".to_owned()];
        for method in methods.iter().filter_map(|method| normalize_method(method)) {
            if !amr.contains(&method) {
                amr.push(method);
            }
            if amr.len() == 16 {
                break;
            }
        }
        let acr = match aal {
            Some("aal1") => Some(ACR_LOA1.to_owned()),
            Some("aal2") => Some(ACR_LOA2.to_owned()),
            _ => None,
        };
        Self {
            amr,
            acr,
            auth_time,
        }
    }
}

fn normalize_method(input: &str) -> Option<String> {
    let method = input.trim().to_ascii_lowercase();
    if method.is_empty() || method.len() > 64 {
        return None;
    }
    let canonical = match method.as_str() {
        "password" | "pwd" => "pwd",
        "totp" => "totp",
        "otp" => "otp",
        "email" | "email_otp" => "email_otp",
        "sms" | "sms_otp" => "sms_otp",
        "webauthn" | "passkey" => "passkey",
        "oauth" | "sso" | "federated" => "federated",
        _ if method.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_:-.".contains(&byte)
        }) =>
        {
            return Some(method);
        }
        _ => return None,
    };
    Some(canonical.to_owned())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_and_refresh_sessions_are_explicitly_base_assurance() {
        let local = AuthenticationAssurance::local_password();
        assert_eq!(local.amr, ["pwd"]);
        assert!(local.auth_time.is_some());
        let refresh = AuthenticationAssurance::refresh_token();
        assert_eq!(refresh.acr.as_deref(), Some(ACR_LOA1));
        assert_eq!(refresh.auth_time, None);
    }

    #[test]
    fn signed_supabase_aal2_maps_to_loa2_and_normalizes_methods() {
        let assurance = AuthenticationAssurance::from_supabase_at(
            Some("aal2"),
            &["password".into(), "totp".into(), "totp".into()],
            Some(123),
        );
        assert_eq!(assurance.amr, ["federated", "pwd", "totp"]);
        assert_eq!(assurance.acr.as_deref(), Some(ACR_LOA2));
        assert_eq!(assurance.auth_time, Some(123));
    }

    #[test]
    fn missing_or_unknown_aal_fails_closed() {
        assert_eq!(AuthenticationAssurance::from_supabase(None, &[]).acr, None);
        assert_eq!(
            AuthenticationAssurance::from_supabase(Some("aal3"), &[]).acr,
            None
        );
    }
}