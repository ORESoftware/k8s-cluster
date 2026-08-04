//! Fail-closed Shared Auth cutover validation that runs before any database,
//! sealing key, provider credential, scheduler, or messaging resource is opened.
//!
//! The runtime uses revocation-aware Shared Auth introspection for identity and
//! Quaestor Postgres for tenant grants. During the compatibility window the
//! existing generic JWT-verifier environment variables remain populated, but
//! they must point at the same Shared Auth authority and must never enable an
//! HS256 secret.

use std::env;

use crate::shared_auth::SharedAuthConfig;

pub fn validate_before_resources() -> anyhow::Result<()> {
    let allow_insecure_dev = env_bool("BILLING_ALLOW_INSECURE_DEV", false);
    let required = env_bool("BILLING_SHARED_AUTH_REQUIRED", !allow_insecure_dev);
    if !required {
        return Ok(());
    }

    let config = SharedAuthConfig::from_env()?.ok_or_else(|| {
        anyhow::anyhow!(
            "BILLING_SHARED_AUTH_REQUIRED=true requires BILLING_SHARED_AUTH_BASE_URL and \
             BILLING_SHARED_AUTH_INTROSPECT_SECRET"
        )
    })?;

    let compatibility = CompatibilityVerifierConfig {
        url: optional_env("BILLING_SUPABASE_URL"),
        issuer: optional_env("BILLING_SUPABASE_JWT_ISS"),
        jwks_url: optional_env("BILLING_SUPABASE_JWKS_URL"),
        audience: optional_env("BILLING_SUPABASE_JWT_AUD"),
        hs256_secret_configured: optional_env("BILLING_SUPABASE_JWT_SECRET").is_some(),
    };

    validate_contract(
        &config,
        &compatibility,
        env_bool("BILLING_TENANT_ROUTES_REQUIRE_USER_JWT", true),
        env_bool("BILLING_REQUIRE_STEP_UP_FOR_MUTATIONS", false),
    )
}

#[derive(Debug, Default)]
struct CompatibilityVerifierConfig {
    url: Option<String>,
    issuer: Option<String>,
    jwks_url: Option<String>,
    audience: Option<String>,
    hs256_secret_configured: bool,
}

fn validate_contract(
    shared_auth: &SharedAuthConfig,
    compatibility: &CompatibilityVerifierConfig,
    require_user_jwt: bool,
    require_step_up: bool,
) -> anyhow::Result<()> {
    if !require_user_jwt {
        anyhow::bail!(
            "BILLING_SHARED_AUTH_REQUIRED=true requires \
             BILLING_TENANT_ROUTES_REQUIRE_USER_JWT=true"
        );
    }
    if !require_step_up {
        anyhow::bail!(
            "BILLING_SHARED_AUTH_REQUIRED=true requires \
             BILLING_REQUIRE_STEP_UP_FOR_MUTATIONS=true"
        );
    }
    if compatibility.hs256_secret_configured {
        anyhow::bail!(
            "BILLING_SUPABASE_JWT_SECRET must be unset when Shared Auth is required; \
             production authentication uses the revocation-aware introspection authority"
        );
    }

    let expected_jwks = optional_env("BILLING_SHARED_AUTH_JWKS_URL")
        .unwrap_or_else(|| format!("{}/.well-known/jwks.json", shared_auth.base_url));
    require_exact(
        "BILLING_SUPABASE_URL",
        compatibility.url.as_deref(),
        &shared_auth.base_url,
    )?;
    require_exact(
        "BILLING_SUPABASE_JWT_ISS",
        compatibility.issuer.as_deref(),
        &shared_auth.issuer,
    )?;
    require_exact(
        "BILLING_SUPABASE_JWKS_URL",
        compatibility.jwks_url.as_deref(),
        &expected_jwks,
    )?;
    require_exact(
        "BILLING_SUPABASE_JWT_AUD",
        compatibility.audience.as_deref(),
        &shared_auth.audience,
    )?;
    Ok(())
}

fn require_exact(name: &str, actual: Option<&str>, expected: &str) -> anyhow::Result<()> {
    match actual {
        Some(value) if value == expected => Ok(()),
        Some(_) => anyhow::bail!(
            "{name} must exactly match the configured Shared Auth authority during cutover"
        ),
        None => anyhow::bail!(
            "{name} is required while the existing verifier configuration is mapped to Shared Auth"
        ),
    }
}

fn optional_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn env_bool(name: &str, default: bool) -> bool {
    env::var(name)
        .ok()
        .map(|value| matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{CompatibilityVerifierConfig, validate_contract};
    use crate::shared_auth::SharedAuthConfig;

    fn shared_auth() -> SharedAuthConfig {
        SharedAuthConfig {
            base_url: "https://auth.example.test".to_owned(),
            introspect_secret: "a".repeat(48),
            issuer: "https://auth.example.test".to_owned(),
            audience: "oresoftware".to_owned(),
            request_timeout: Duration::from_millis(1_500),
            require_session_id: true,
        }
    }

    fn compatibility() -> CompatibilityVerifierConfig {
        CompatibilityVerifierConfig {
            url: Some("https://auth.example.test".to_owned()),
            issuer: Some("https://auth.example.test".to_owned()),
            jwks_url: Some("https://auth.example.test/.well-known/jwks.json".to_owned()),
            audience: Some("oresoftware".to_owned()),
            hs256_secret_configured: false,
        }
    }

    #[test]
    fn exact_shared_auth_mapping_is_accepted() {
        validate_contract(&shared_auth(), &compatibility(), true, true).unwrap();
    }

    #[test]
    fn production_gates_must_remain_enabled() {
        assert!(validate_contract(&shared_auth(), &compatibility(), false, true).is_err());
        assert!(validate_contract(&shared_auth(), &compatibility(), true, false).is_err());
    }

    #[test]
    fn hs256_and_authority_drift_fail_closed() {
        let mut config = compatibility();
        config.hs256_secret_configured = true;
        assert!(validate_contract(&shared_auth(), &config, true, true).is_err());

        let mut config = compatibility();
        config.issuer = Some("https://different.example.test".to_owned());
        assert!(validate_contract(&shared_auth(), &config, true, true).is_err());

        let mut config = compatibility();
        config.jwks_url = None;
        assert!(validate_contract(&shared_auth(), &config, true, true).is_err());
    }
}
