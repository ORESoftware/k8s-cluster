use std::env;
use std::sync::Arc;

use thiserror::Error;

use crate::contracts::ProviderEnvironment;
use crate::dispatch::{ProviderRegistry, ProviderSlot, RegistryError};
use crate::http_api::{
    AuthConfigError, DenyAllAuthenticator, RequestAuthenticator, SharedSecretAuthenticator,
};
use crate::providers::{
    ApnsConfig, ApnsProvider, ExpoConfig, ExpoProvider, FcmConfig, FcmProvider, WebPushConfig,
    WebPushHostPolicy, WebPushProvider,
};

#[derive(Debug, Error)]
pub enum RuntimeConfigError {
    #[error("{0} configuration is incomplete")]
    IncompleteProvider(&'static str),

    #[error("{provider} configuration is invalid: {safe_detail}")]
    InvalidProvider {
        provider: &'static str,
        safe_detail: String,
    },

    #[error("APNS_ENVIRONMENT must be production or sandbox")]
    InvalidApnsEnvironment,

    #[error("WEBPUSH_TTL_SECONDS must be a valid non-negative integer")]
    InvalidWebPushTtl,

    #[error("ENABLE_SHARED_SECRET_AUTH is true but SERVER_AUTH_SECRET is absent")]
    MissingSharedSecret,

    #[error(transparent)]
    Auth(#[from] AuthConfigError),

    #[error(transparent)]
    Registry(#[from] RegistryError),
}

pub fn provider_registry_from_env() -> Result<ProviderRegistry, RuntimeConfigError> {
    let mut registry = ProviderRegistry::new();

    let expo = ExpoProvider::new(
        ExpoConfig::new(non_empty_env("EXPO_ACCESS_TOKEN"))
            .map_err(|error| invalid_provider("expo", error))?,
    )
    .map_err(|error| invalid_provider("expo", error))?;
    registry = registry.with_provider(ProviderSlot::Expo, Arc::new(expo))?;

    if let Some(service_account) = non_empty_env("FCM_SERVICE_ACCOUNT_JSON") {
        let config = FcmConfig::from_service_account_json(
            &service_account,
            non_empty_env("FCM_PROJECT_ID").as_deref(),
        )
        .map_err(|error| invalid_provider("fcm", error))?;
        let provider = FcmProvider::new(config).map_err(|error| invalid_provider("fcm", error))?;
        registry = registry.with_provider(ProviderSlot::Fcm, Arc::new(provider))?;
    }

    registry = configure_apns(registry)?;
    registry = configure_web_push(registry)?;
    Ok(registry)
}

pub fn request_authenticator_from_env() -> Result<Arc<dyn RequestAuthenticator>, RuntimeConfigError>
{
    if !environment_flag("ENABLE_SHARED_SECRET_AUTH") {
        return Ok(Arc::new(DenyAllAuthenticator));
    }
    let secret =
        non_empty_env("SERVER_AUTH_SECRET").ok_or(RuntimeConfigError::MissingSharedSecret)?;
    Ok(Arc::new(SharedSecretAuthenticator::new(secret)?))
}

fn configure_apns(registry: ProviderRegistry) -> Result<ProviderRegistry, RuntimeConfigError> {
    let key = non_empty_env("APNS_KEY_P8");
    let key_id = non_empty_env("APNS_KEY_ID");
    let team_id = non_empty_env("APNS_TEAM_ID");
    let topic = non_empty_env("APNS_TOPIC");
    let values_present = [
        key.is_some(),
        key_id.is_some(),
        team_id.is_some(),
        topic.is_some(),
    ];
    if !values_present.iter().any(|value| *value) {
        return Ok(registry);
    }
    if !values_present.iter().all(|value| *value) {
        return Err(RuntimeConfigError::IncompleteProvider("apns"));
    }

    let environment = match non_empty_env("APNS_ENVIRONMENT")
        .unwrap_or_else(|| "production".to_owned())
        .to_ascii_lowercase()
        .as_str()
    {
        "production" => ProviderEnvironment::Production,
        "sandbox" | "development" => ProviderEnvironment::Sandbox,
        _ => return Err(RuntimeConfigError::InvalidApnsEnvironment),
    };
    let config = ApnsConfig::new(
        key_id.expect("validated APNs key ID"),
        team_id.expect("validated APNs team ID"),
        topic.expect("validated APNs topic"),
        &key.expect("validated APNs private key"),
        environment,
    )
    .map_err(|error| invalid_provider("apns", error))?;
    let provider = ApnsProvider::new(config).map_err(|error| invalid_provider("apns", error))?;
    let slot = match environment {
        ProviderEnvironment::Production => ProviderSlot::ApnsProduction,
        ProviderEnvironment::Sandbox => ProviderSlot::ApnsSandbox,
    };
    Ok(registry.with_provider(slot, Arc::new(provider))?)
}

fn configure_web_push(registry: ProviderRegistry) -> Result<ProviderRegistry, RuntimeConfigError> {
    let Some(private_key) = non_empty_env("VAPID_PRIVATE_KEY") else {
        return Ok(registry);
    };
    let subject =
        non_empty_env("VAPID_SUBJECT").ok_or(RuntimeConfigError::IncompleteProvider("web_push"))?;
    let policy = match non_empty_env("WEBPUSH_ALLOWED_HOSTS") {
        None => WebPushHostPolicy::strict_default(),
        Some(value) if value == "*" => WebPushHostPolicy::any_public(),
        Some(value) => WebPushHostPolicy::allowlist(
            value
                .split(',')
                .map(str::trim)
                .filter(|host| !host.is_empty()),
        )
        .map_err(|error| invalid_provider("web_push", error))?,
    };
    let mut config = WebPushConfig::new(private_key, subject, policy)
        .map_err(|error| invalid_provider("web_push", error))?;
    if let Some(ttl) = non_empty_env("WEBPUSH_TTL_SECONDS") {
        let ttl = ttl
            .parse::<u32>()
            .map_err(|_| RuntimeConfigError::InvalidWebPushTtl)?;
        config = config
            .with_default_ttl(ttl)
            .map_err(|error| invalid_provider("web_push", error))?;
    }
    let provider =
        WebPushProvider::new(config).map_err(|error| invalid_provider("web_push", error))?;
    Ok(registry.with_provider(ProviderSlot::WebPush, Arc::new(provider))?)
}

fn non_empty_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn environment_flag(name: &str) -> bool {
    env::var(name).ok().is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn invalid_provider(provider: &'static str, error: impl std::fmt::Display) -> RuntimeConfigError {
    RuntimeConfigError::InvalidProvider {
        provider,
        safe_detail: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_flag_is_false_for_absent_values() {
        assert!(!environment_flag(
            "PUSH_NOTIFICATION_TEST_FLAG_THAT_IS_NOT_SET"
        ));
    }

    #[test]
    fn provider_errors_do_not_require_secret_debug_output() {
        let error = invalid_provider("fcm", "invalid service-account document");
        assert!(error.to_string().contains("fcm"));
        assert!(!error.to_string().contains("PRIVATE KEY"));
    }
}
