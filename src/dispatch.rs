use std::sync::Arc;

use serde::Serialize;
use thiserror::Error;
use utoipa::ToSchema;

use crate::contracts::{ProviderEnvironment, ProviderKind, PushJob, PushOutcome, PushTarget};
use crate::provider::{ProviderError, ProviderReadiness, PushProvider};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderSlot {
    Fcm,
    ApnsProduction,
    ApnsSandbox,
    Expo,
    WebPush,
}

impl ProviderSlot {
    const fn expected_kind(self) -> ProviderKind {
        match self {
            Self::Fcm => ProviderKind::Fcm,
            Self::ApnsProduction | Self::ApnsSandbox => ProviderKind::Apns,
            Self::Expo => ProviderKind::Expo,
            Self::WebPush => ProviderKind::WebPush,
        }
    }
}

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("provider kind does not match registry slot")]
    KindMismatch,
}

#[derive(Clone, Default)]
pub struct ProviderRegistry {
    fcm: Option<Arc<dyn PushProvider>>,
    apns_production: Option<Arc<dyn PushProvider>>,
    apns_sandbox: Option<Arc<dyn PushProvider>>,
    expo: Option<Arc<dyn PushProvider>>,
    web_push: Option<Arc<dyn PushProvider>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_provider(
        mut self,
        slot: ProviderSlot,
        provider: Arc<dyn PushProvider>,
    ) -> Result<Self, RegistryError> {
        if provider.kind() != slot.expected_kind() {
            return Err(RegistryError::KindMismatch);
        }
        match slot {
            ProviderSlot::Fcm => self.fcm = Some(provider),
            ProviderSlot::ApnsProduction => self.apns_production = Some(provider),
            ProviderSlot::ApnsSandbox => self.apns_sandbox = Some(provider),
            ProviderSlot::Expo => self.expo = Some(provider),
            ProviderSlot::WebPush => self.web_push = Some(provider),
        }
        Ok(self)
    }

    pub async fn dispatch(&self, job: &PushJob) -> Result<PushOutcome, ProviderError> {
        let provider = self.provider_for(job).ok_or_else(|| {
            ProviderError::not_configured(format!(
                "{} provider is not configured for this target environment",
                job.provider.as_str()
            ))
        })?;
        provider.send(job).await
    }

    pub fn readiness(&self) -> RegistryReadiness {
        RegistryReadiness {
            fcm: readiness(self.fcm.as_ref()),
            apns_production: readiness(self.apns_production.as_ref()),
            apns_sandbox: readiness(self.apns_sandbox.as_ref()),
            expo: readiness(self.expo.as_ref()),
            web_push: readiness(self.web_push.as_ref()),
        }
    }

    pub fn has_configured_provider(&self) -> bool {
        [
            self.fcm.as_ref(),
            self.apns_production.as_ref(),
            self.apns_sandbox.as_ref(),
            self.expo.as_ref(),
            self.web_push.as_ref(),
        ]
        .into_iter()
        .flatten()
        .any(|provider| provider.readiness().configured)
    }

    fn provider_for(&self, job: &PushJob) -> Option<&Arc<dyn PushProvider>> {
        match &job.target {
            PushTarget::Fcm { .. } => self.fcm.as_ref(),
            PushTarget::Apns { environment, .. } => match environment {
                ProviderEnvironment::Production => self.apns_production.as_ref(),
                ProviderEnvironment::Sandbox => self.apns_sandbox.as_ref(),
            },
            PushTarget::Expo { .. } => self.expo.as_ref(),
            PushTarget::WebPush { .. } => self.web_push.as_ref(),
        }
    }
}

fn readiness(provider: Option<&Arc<dyn PushProvider>>) -> ProviderReadinessView {
    provider
        .map(|provider| provider.readiness())
        .unwrap_or_else(|| ProviderReadiness::not_ready("provider is not configured"))
        .into()
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RegistryReadiness {
    pub fcm: ProviderReadinessView,
    pub apns_production: ProviderReadinessView,
    pub apns_sandbox: ProviderReadinessView,
    pub expo: ProviderReadinessView,
    pub web_push: ProviderReadinessView,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ProviderReadinessView {
    pub configured: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safe_reason: Option<String>,
}

impl From<ProviderReadiness> for ProviderReadinessView {
    fn from(value: ProviderReadiness) -> Self {
        Self {
            configured: value.configured,
            safe_reason: value.safe_reason,
        }
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;

    use super::*;
    use crate::contracts::{ContractVersion, Notification, PushOptions, TraceMetadata};

    struct AcceptingProvider(ProviderKind);

    #[async_trait]
    impl PushProvider for AcceptingProvider {
        fn kind(&self) -> ProviderKind {
            self.0
        }

        fn readiness(&self) -> ProviderReadiness {
            ProviderReadiness::ready()
        }

        async fn send(&self, job: &PushJob) -> Result<PushOutcome, ProviderError> {
            Ok(PushOutcome::accepted(job))
        }
    }

    fn fcm_job() -> PushJob {
        PushJob {
            version: ContractVersion::V1,
            job_id: "job-1".to_owned(),
            tenant_id: "tenant-1".to_owned(),
            application_id: "app-1".to_owned(),
            idempotency_key: "event-1".to_owned(),
            provider: ProviderKind::Fcm,
            target: PushTarget::Fcm {
                token: "fixture-token".to_owned(),
            },
            notification: Notification {
                title: Some("Hello".to_owned()),
                ..Notification::default()
            },
            options: PushOptions::default(),
            trace: TraceMetadata::default(),
        }
    }

    #[tokio::test]
    async fn dispatches_to_the_registered_provider() {
        let registry = ProviderRegistry::new()
            .with_provider(
                ProviderSlot::Fcm,
                Arc::new(AcceptingProvider(ProviderKind::Fcm)),
            )
            .expect("registry");
        let outcome = registry.dispatch(&fcm_job()).await.expect("outcome");
        assert_eq!(outcome.provider, ProviderKind::Fcm);
    }

    #[test]
    fn rejects_provider_slot_mismatches() {
        let result = ProviderRegistry::new().with_provider(
            ProviderSlot::Fcm,
            Arc::new(AcceptingProvider(ProviderKind::Expo)),
        );
        assert!(matches!(result, Err(RegistryError::KindMismatch)));
    }
}
