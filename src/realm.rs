//! Runtime boundary between privileged admin auth and customer auth.
//!
//! The same binary is deployed twice. Production startup fails unless the
//! selected realm, issuer, RDS endpoint/reference, secret paths, signing-key
//! reference, cookie namespace, and dedicated Supabase project all agree.
//! `AUTH_ALLOW_DBLESS=true` remains the explicit development/test escape hatch.

use std::{env, fmt};

use url::Url;

use crate::{config::AppConfig, error::ConfigError};

const MAX_REFERENCE_BYTES: usize = 256;
const MAX_COOKIE_NAME_BYTES: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Realm {
    Admin,
    Customer,
}

impl Realm {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Customer => "customer",
        }
    }

    fn parse(value: &str) -> Result<Self, ConfigError> {
        match value {
            "admin" => Ok(Self::Admin),
            "customer" => Ok(Self::Customer),
            _ => Err(ConfigError::Invalid(
                "AUTH_REALM must be exactly admin or customer",
            )),
        }
    }
}

impl fmt::Display for Realm {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone)]
pub struct RealmConfig {
    pub realm: Realm,
    pub deployment: String,
    pub database_resource_ref: String,
    pub database_secret_ref: String,
    pub signing_key_ref: String,
    pub session_cookie_name: String,
    pub supabase_project_ref: Option<String>,
    pub development_dbless: bool,
}

struct RealmInput {
    realm: Realm,
    deployment: String,
    issuer: String,
    database_url: String,
    database_resource_ref: String,
    database_secret_ref: String,
    signing_key_ref: String,
    session_cookie_name: String,
    supabase_project_ref: String,
}

impl RealmConfig {
    pub fn from_env(config: &AppConfig) -> Result<Self, ConfigError> {
        if optional_env("AUTH_APPLICATION_DATABASE_URL").is_some() {
            return Err(ConfigError::Invalid(
                "AUTH_APPLICATION_DATABASE_URL is forbidden in shared-auth",
            ));
        }

        // DB-less mode is already guarded by AUTH_ALLOW_DBLESS and exists only
        // for local tests. Keep it compatible without weakening a DB-backed
        // production deployment: every DB-backed process must provide the full
        // realm contract below.
        let Some(db) = config.db.as_ref() else {
            let realm = optional_env("AUTH_REALM")
                .as_deref()
                .map(Realm::parse)
                .transpose()?
                .unwrap_or(Realm::Customer);
            return Ok(Self {
                realm,
                deployment: format!("shared-auth-{}-dbless", realm.as_str()),
                database_resource_ref: "development:dbless".to_owned(),
                database_secret_ref: "development:dbless".to_owned(),
                signing_key_ref: "development:ephemeral".to_owned(),
                session_cookie_name: format!("__Host-shared-auth-{}", realm.as_str()),
                supabase_project_ref: config
                    .projects
                    .first()
                    .map(|project| project.project_ref.clone()),
                development_dbless: true,
            });
        };

        let input = RealmInput {
            realm: Realm::parse(&required_env("AUTH_REALM")?)?,
            deployment: required_env("AUTH_REALM_DEPLOYMENT")?,
            issuer: config.signing.issuer.clone(),
            database_url: db.url.clone(),
            database_resource_ref: required_env("AUTH_DATABASE_RESOURCE_REF")?,
            database_secret_ref: required_env("AUTH_DATABASE_SECRET_REF")?,
            signing_key_ref: required_env("AUTH_SIGNING_KEY_REF")?,
            session_cookie_name: required_env("AUTH_SESSION_COOKIE_NAME")?,
            supabase_project_ref: required_env("AUTH_REALM_SUPABASE_PROJECT_REF")?,
        };
        let provider_refs: Vec<&str> = config
            .projects
            .iter()
            .map(|project| project.project_ref.as_str())
            .collect();
        Self::validate(input, &provider_refs)
    }

    fn validate(input: RealmInput, provider_refs: &[&str]) -> Result<Self, ConfigError> {
        let realm = input.realm.as_str();
        validate_reference(&input.deployment, "AUTH_REALM_DEPLOYMENT")?;
        validate_reference(
            &input.database_resource_ref,
            "AUTH_DATABASE_RESOURCE_REF",
        )?;
        validate_reference(&input.database_secret_ref, "AUTH_DATABASE_SECRET_REF")?;
        validate_reference(&input.signing_key_ref, "AUTH_SIGNING_KEY_REF")?;

        if !input.deployment.contains(realm) {
            return Err(ConfigError::Invalid(
                "AUTH_REALM_DEPLOYMENT must visibly name AUTH_REALM",
            ));
        }
        if !input.database_resource_ref.contains(realm) {
            return Err(ConfigError::Invalid(
                "AUTH_DATABASE_RESOURCE_REF must visibly name AUTH_REALM",
            ));
        }
        if !input
            .database_secret_ref
            .contains(&format!("/{realm}/"))
        {
            return Err(ConfigError::Invalid(
                "AUTH_DATABASE_SECRET_REF must be scoped to AUTH_REALM",
            ));
        }
        if !input.signing_key_ref.contains(&format!("/{realm}/")) {
            return Err(ConfigError::Invalid(
                "AUTH_SIGNING_KEY_REF must be scoped to AUTH_REALM",
            ));
        }

        validate_cookie_name(&input.session_cookie_name, input.realm)?;
        validate_issuer(&input.issuer, input.realm)?;
        validate_database_url(&input.database_url, input.realm)?;
        validate_project_ref(&input.supabase_project_ref)?;

        if provider_refs.len() != 1 || provider_refs[0] != input.supabase_project_ref {
            return Err(ConfigError::Invalid(
                "AUTH_SUPABASE_PROJECTS must contain exactly the realm Supabase project",
            ));
        }

        Ok(Self {
            realm: input.realm,
            deployment: input.deployment,
            database_resource_ref: input.database_resource_ref,
            database_secret_ref: input.database_secret_ref,
            signing_key_ref: input.signing_key_ref,
            session_cookie_name: input.session_cookie_name,
            supabase_project_ref: Some(input.supabase_project_ref),
            development_dbless: false,
        })
    }
}

fn required_env(key: &'static str) -> Result<String, ConfigError> {
    optional_env(key).ok_or(ConfigError::Missing(key))
}

fn optional_env(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn validate_reference(value: &str, key: &'static str) -> Result<(), ConfigError> {
    if value.is_empty()
        || value.len() > MAX_REFERENCE_BYTES
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/' | b':')
        })
    {
        return Err(ConfigError::Invalid(key));
    }
    Ok(())
}

fn validate_cookie_name(value: &str, realm: Realm) -> Result<(), ConfigError> {
    if !value.starts_with("__Host-")
        || !value.contains(realm.as_str())
        || value.len() > MAX_COOKIE_NAME_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(ConfigError::Invalid(
            "AUTH_SESSION_COOKIE_NAME must be a realm-specific __Host- cookie name",
        ));
    }
    Ok(())
}

fn validate_issuer(value: &str, realm: Realm) -> Result<(), ConfigError> {
    let parsed = Url::parse(value).map_err(|_| ConfigError::Invalid("AUTH_ISSUER"))?;
    if parsed.scheme() != "https"
        || parsed.username() != ""
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(ConfigError::Invalid(
            "AUTH_ISSUER must be a credential-free HTTPS origin/path",
        ));
    }
    let first_label = parsed
        .host_str()
        .and_then(|host| host.split('.').next())
        .ok_or(ConfigError::Invalid("AUTH_ISSUER must contain a host"))?;
    match realm {
        Realm::Admin if !first_label.contains("admin-auth") => Err(ConfigError::Invalid(
            "admin AUTH_ISSUER must use an admin-auth host",
        )),
        Realm::Customer if first_label.contains("admin-auth") => Err(ConfigError::Invalid(
            "customer AUTH_ISSUER must not use an admin-auth host",
        )),
        _ => Ok(()),
    }
}

fn validate_database_url(value: &str, realm: Realm) -> Result<(), ConfigError> {
    let parsed = Url::parse(value).map_err(|_| ConfigError::Invalid("AUTH_DATABASE_URL"))?;
    if !matches!(parsed.scheme(), "postgres" | "postgresql")
        || !parsed
            .host_str()
            .is_some_and(|host| host.contains(realm.as_str()))
    {
        return Err(ConfigError::Invalid(
            "AUTH_DATABASE_URL must target the selected realm PostgreSQL endpoint",
        ));
    }
    Ok(())
}

fn validate_project_ref(value: &str) -> Result<(), ConfigError> {
    if !(6..=64).contains(&value.len())
        || !value.bytes().all(|byte| byte.is_ascii_alphanumeric())
    {
        return Err(ConfigError::Invalid(
            "AUTH_REALM_SUPABASE_PROJECT_REF must be an alphanumeric project ref",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(realm: Realm) -> RealmInput {
        let name = realm.as_str();
        RealmInput {
            realm,
            deployment: format!("shared-auth-{name}"),
            issuer: match realm {
                Realm::Admin => "https://admin-auth.example.test".to_owned(),
                Realm::Customer => "https://auth.example.test".to_owned(),
            },
            database_url: format!(
                "postgres://runtime:secret@shared-auth-{name}-prod.abc.us-east-1.rds.amazonaws.com/shared_auth?sslmode=require"
            ),
            database_resource_ref: format!("aws:rds:shared-auth-{name}-prod"),
            database_secret_ref: format!("dd/shared-auth/{name}/database-url"),
            signing_key_ref: format!("dd/shared-auth/{name}/signing-key"),
            session_cookie_name: format!("__Host-shared-auth-{name}"),
            supabase_project_ref: format!("{name}projectref01"),
        }
    }

    #[test]
    fn accepts_independent_admin_and_customer_profiles() {
        for realm in [Realm::Admin, Realm::Customer] {
            let candidate = input(realm);
            let provider_ref = candidate.supabase_project_ref.clone();
            let profile = RealmConfig::validate(candidate, &[provider_ref.as_str()]).unwrap();
            assert_eq!(profile.realm, realm);
            assert!(!profile.development_dbless);
        }
    }

    #[test]
    fn rejects_application_database_endpoint_cross_wiring() {
        let mut candidate = input(Realm::Customer);
        candidate.database_url =
            "postgres://runtime:secret@app-prod.abc.rds.amazonaws.com/application".to_owned();
        assert!(RealmConfig::validate(candidate, &["customerprojectref01"]).is_err());
    }

    #[test]
    fn rejects_wrong_realm_secret_and_resource_references() {
        let mut candidate = input(Realm::Customer);
        candidate.database_resource_ref = "aws:rds:shared-auth-admin-prod".to_owned();
        candidate.database_secret_ref = "dd/shared-auth/admin/database-url".to_owned();
        candidate.signing_key_ref = "dd/shared-auth/admin/signing-key".to_owned();
        assert!(RealmConfig::validate(candidate, &["customerprojectref01"]).is_err());
    }

    #[test]
    fn requires_exactly_one_matching_supabase_project() {
        let candidate = input(Realm::Admin);
        assert!(RealmConfig::validate(candidate, &[]).is_err());

        let candidate = input(Realm::Admin);
        assert!(RealmConfig::validate(candidate, &["differentprojectref"]).is_err());

        let candidate = input(Realm::Admin);
        assert!(RealmConfig::validate(
            candidate,
            &["adminprojectref01", "legacyprojectref01"],
        )
        .is_err());
    }

    #[test]
    fn admin_issuer_must_use_the_admin_host() {
        let mut candidate = input(Realm::Admin);
        candidate.issuer = "https://auth.example.test".to_owned();
        assert!(RealmConfig::validate(candidate, &["adminprojectref01"]).is_err());
    }

    #[test]
    fn cookie_namespace_is_realm_specific() {
        let mut candidate = input(Realm::Customer);
        candidate.session_cookie_name = "__Host-shared-auth-admin".to_owned();
        assert!(RealmConfig::validate(candidate, &["customerprojectref01"]).is_err());
    }
}
