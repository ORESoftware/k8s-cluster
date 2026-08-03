//! Fail-closed Shared Auth runtime contract for the billing service.
//!
//! Quaestor's existing JWT verifier is intentionally issuer-agnostic: it pins
//! `iss`, `aud`, algorithms, and JWKS keys, then maps the signed claims into the
//! tenant/step-up authorization model. That verifier does not need a second
//! implementation for Shared Auth. What it does need is an explicit production
//! contract proving operators pointed it at Shared Auth rather than at one
//! tenant's Supabase project or a legacy HS256 secret.

use crate::config::Config;
use crate::supabase_auth::SupabaseConfig;

const DEFAULT_SHARED_AUTH_BASE_URL: &str = "https://auth.oresoftware.dev";
const DEFAULT_SHARED_AUTH_AUDIENCE: &str = "oresoftware";

#[derive(Clone, Debug, PartialEq, Eq)]
struct ExpectedSharedAuth {
    base_url: String,
    issuer: String,
    jwks_url: String,
    audience: String,
}

impl ExpectedSharedAuth {
    fn from_env() -> anyhow::Result<Option<Self>> {
        let insecure_dev = env_bool("BILLING_ALLOW_INSECURE_DEV", false)?;
        let required = env_bool("BILLING_SHARED_AUTH_REQUIRED", !insecure_dev)?;
        if !required {
            return Ok(None);
        }

        let base_url = env_or(
            "BILLING_SHARED_AUTH_BASE_URL",
            DEFAULT_SHARED_AUTH_BASE_URL,
        );
        let issuer = env_or("BILLING_SHARED_AUTH_ISSUER", &base_url);
        let jwks_url = env_or(
            "BILLING_SHARED_AUTH_JWKS_URL",
            &format!(
                "{}/.well-known/jwks.json",
                base_url.trim_end_matches('/')
            ),
        );
        let audience = env_or(
            "BILLING_SHARED_AUTH_AUDIENCE",
            DEFAULT_SHARED_AUTH_AUDIENCE,
        );

        let expected = Self {
            base_url: canonical_url(&base_url),
            issuer: canonical_url(&issuer),
            jwks_url: canonical_url(&jwks_url),
            audience: audience.trim().to_string(),
        };
        expected.validate_urls(insecure_dev)?;
        if expected.audience.is_empty() {
            anyhow::bail!("BILLING_SHARED_AUTH_AUDIENCE must not be empty");
        }
        Ok(Some(expected))
    }

    fn validate_urls(&self, insecure_dev: bool) -> anyhow::Result<()> {
        for (name, value) in [
            ("BILLING_SHARED_AUTH_BASE_URL", self.base_url.as_str()),
            ("BILLING_SHARED_AUTH_ISSUER", self.issuer.as_str()),
            ("BILLING_SHARED_AUTH_JWKS_URL", self.jwks_url.as_str()),
        ] {
            let parsed = url::Url::parse(value)
                .map_err(|error| anyhow::anyhow!("{name} is not a valid URL: {error}"))?;
            if parsed.scheme() != "https" && !insecure_dev {
                anyhow::bail!(
                    "{name} must use https in production (set BILLING_ALLOW_INSECURE_DEV=1 only for local development)"
                );
            }
            if parsed.host_str().is_none() || parsed.fragment().is_some() {
                anyhow::bail!("{name} must be an absolute URL without a fragment");
            }
        }
        Ok(())
    }
}

/// Validate that the generic JWT verifier is wired to Shared Auth and that the
/// authorization gates needed for customer billing are enabled.
///
/// Production defaults to requiring this contract. Local development opts out
/// implicitly only when `BILLING_ALLOW_INSECURE_DEV=1`; it can still exercise
/// the production contract by setting `BILLING_SHARED_AUTH_REQUIRED=true`.
pub fn validate_runtime_contract(cfg: &Config) -> anyhow::Result<()> {
    let Some(expected) = ExpectedSharedAuth::from_env()? else {
        tracing::warn!(
            "BILLING_SHARED_AUTH_REQUIRED=false — direct/legacy authentication is enabled for development only"
        );
        return Ok(());
    };

    validate_verifier_contract(
        &cfg.supabase,
        cfg.tenant_routes_require_user_jwt,
        cfg.step_up_required_for_mutations,
        &expected,
    )?;

    tracing::info!(
        auth.issuer = %expected.issuer,
        auth.audience = %expected.audience,
        "Shared Auth is the enforced billing authentication authority"
    );
    Ok(())
}

fn validate_verifier_contract(
    verifier: &SupabaseConfig,
    require_user_jwt: bool,
    require_step_up: bool,
    expected: &ExpectedSharedAuth,
) -> anyhow::Result<()> {
    let actual_base = verifier
        .url
        .as_deref()
        .map(canonical_url)
        .ok_or_else(|| anyhow::anyhow!(missing_mapping("BILLING_SUPABASE_URL")))?;
    let actual_issuer = verifier
        .issuer
        .as_deref()
        .map(canonical_url)
        .ok_or_else(|| anyhow::anyhow!(missing_mapping("BILLING_SUPABASE_JWT_ISS")))?;
    let actual_jwks = verifier
        .jwks_url
        .as_deref()
        .map(canonical_url)
        .ok_or_else(|| anyhow::anyhow!(missing_mapping("BILLING_SUPABASE_JWKS_URL")))?;

    require_equal(
        "BILLING_SUPABASE_URL",
        &actual_base,
        "BILLING_SHARED_AUTH_BASE_URL",
        &expected.base_url,
    )?;
    require_equal(
        "BILLING_SUPABASE_JWT_ISS",
        &actual_issuer,
        "BILLING_SHARED_AUTH_ISSUER",
        &expected.issuer,
    )?;
    require_equal(
        "BILLING_SUPABASE_JWKS_URL",
        &actual_jwks,
        "BILLING_SHARED_AUTH_JWKS_URL",
        &expected.jwks_url,
    )?;
    require_equal(
        "BILLING_SUPABASE_JWT_AUD",
        verifier.audience.trim(),
        "BILLING_SHARED_AUTH_AUDIENCE",
        &expected.audience,
    )?;

    if verifier.jwt_secret.is_some() {
        anyhow::bail!(
            "BILLING_SUPABASE_JWT_SECRET must be unset when Shared Auth is required; billing must verify Shared Auth's asymmetric JWKS, never an HS256 secret"
        );
    }
    if !require_user_jwt {
        anyhow::bail!(
            "BILLING_TENANT_ROUTES_REQUIRE_USER_JWT must be true when Shared Auth is required; otherwise the shared service bearer can bypass per-tenant identity"
        );
    }
    if !require_step_up {
        anyhow::bail!(
            "BILLING_REQUIRE_STEP_UP_FOR_MUTATIONS must be true when Shared Auth is required; customer-billing mutations require fresh AAL2 plus billing:write"
        );
    }
    Ok(())
}

fn require_equal(
    actual_name: &str,
    actual: &str,
    expected_name: &str,
    expected: &str,
) -> anyhow::Result<()> {
    if actual != expected {
        anyhow::bail!(
            "{actual_name} must equal {expected_name} when BILLING_SHARED_AUTH_REQUIRED=true (got {actual:?}, expected {expected:?})"
        );
    }
    Ok(())
}

fn missing_mapping(name: &str) -> String {
    format!(
        "{name} is required when BILLING_SHARED_AUTH_REQUIRED=true; map the existing BILLING_SUPABASE_* verifier settings to the corresponding BILLING_SHARED_AUTH_* values"
    )
}

fn canonical_url(raw: &str) -> String {
    raw.trim().trim_end_matches('/').to_string()
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn env_bool(name: &str, default: bool) -> anyhow::Result<bool> {
    let Ok(raw) = std::env::var(name) else {
        return Ok(default);
    };
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => anyhow::bail!("{name} must be a boolean (true/false or 1/0)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expected() -> ExpectedSharedAuth {
        ExpectedSharedAuth {
            base_url: DEFAULT_SHARED_AUTH_BASE_URL.into(),
            issuer: DEFAULT_SHARED_AUTH_BASE_URL.into(),
            jwks_url: format!("{DEFAULT_SHARED_AUTH_BASE_URL}/.well-known/jwks.json"),
            audience: DEFAULT_SHARED_AUTH_AUDIENCE.into(),
        }
    }

    fn configured() -> SupabaseConfig {
        let expected = expected();
        SupabaseConfig {
            url: Some(expected.base_url),
            audience: expected.audience,
            issuer: Some(expected.issuer),
            jwks_url: Some(expected.jwks_url),
            jwt_secret: None,
        }
    }

    #[test]
    fn accepts_exact_shared_auth_mapping_with_all_financial_gates() {
        validate_verifier_contract(&configured(), true, true, &expected()).unwrap();
    }

    #[test]
    fn rejects_direct_supabase_issuer() {
        let mut verifier = configured();
        verifier.issuer = Some("https://project.supabase.co/auth/v1".into());
        let error = validate_verifier_contract(&verifier, true, true, &expected()).unwrap_err();
        assert!(error.to_string().contains("BILLING_SUPABASE_JWT_ISS"));
    }

    #[test]
    fn rejects_hs256_and_migration_bypasses() {
        let mut verifier = configured();
        verifier.jwt_secret = Some("must-not-be-used".into());
        assert!(validate_verifier_contract(&verifier, true, true, &expected()).is_err());

        let verifier = configured();
        assert!(validate_verifier_contract(&verifier, false, true, &expected()).is_err());
        assert!(validate_verifier_contract(&verifier, true, false, &expected()).is_err());
    }
}
