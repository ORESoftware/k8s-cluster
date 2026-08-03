use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::redaction::{TargetFingerprint, fingerprint_target};

/// The wire-contract version accepted by this service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ContractVersion {
    V1,
}

impl Default for ContractVersion {
    fn default() -> Self {
        Self::V1
    }
}

/// Supported upstream delivery providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Fcm,
    Apns,
    Expo,
    WebPush,
}

impl ProviderKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fcm => "fcm",
            Self::Apns => "apns",
            Self::Expo => "expo",
            Self::WebPush => "web_push",
        }
    }
}

/// Provider environment where a target is valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderEnvironment {
    Production,
    Sandbox,
}

/// A provider-specific capability target.
///
/// Values in this enum are secrets or capabilities. They must never be logged
/// or copied into result events. Use [`PushTarget::fingerprint`] instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PushTarget {
    Fcm {
        token: String,
    },
    Apns {
        token: String,
        environment: ProviderEnvironment,
    },
    Expo {
        token: String,
    },
    WebPush {
        endpoint: String,
        p256dh: String,
        auth: String,
    },
}

impl PushTarget {
    pub const fn provider(&self) -> ProviderKind {
        match self {
            Self::Fcm { .. } => ProviderKind::Fcm,
            Self::Apns { .. } => ProviderKind::Apns,
            Self::Expo { .. } => ProviderKind::Expo,
            Self::WebPush { .. } => ProviderKind::WebPush,
        }
    }

    pub fn fingerprint(&self) -> TargetFingerprint {
        fingerprint_target(self)
    }

    pub(crate) fn fingerprint_material(&self) -> (&'static str, Vec<&str>) {
        match self {
            Self::Fcm { token } => ("fcm", vec![token.as_str()]),
            Self::Apns { token, environment } => {
                let environment = match environment {
                    ProviderEnvironment::Production => "production",
                    ProviderEnvironment::Sandbox => "sandbox",
                };
                ("apns", vec![environment, token.as_str()])
            }
            Self::Expo { token } => ("expo", vec![token.as_str()]),
            Self::WebPush {
                endpoint,
                p256dh,
                auth,
            } => (
                "web_push",
                vec![endpoint.as_str(), p256dh.as_str(), auth.as_str()],
            ),
        }
    }
}

/// User-visible notification content and provider-neutral application data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Default)]
pub struct Notification {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub data: BTreeMap<String, Value>,
}

/// Delivery urgency requested by the producer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum PushPriority {
    #[default]
    Normal,
    High,
}

/// Provider-neutral delivery controls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema, Default)]
pub struct PushOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_seconds: Option<u32>,
    #[serde(default)]
    pub priority: PushPriority,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collapse_key: Option<String>,
    #[serde(default)]
    pub dry_run: bool,
}

/// Trace and correlation data safe to propagate across service boundaries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema, Default)]
pub struct TraceMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traceparent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracestate: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
}

/// Versioned provider-neutral input consumed by all delivery adapters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PushJob {
    #[serde(default)]
    pub version: ContractVersion,
    pub job_id: String,
    pub tenant_id: String,
    pub application_id: String,
    pub idempotency_key: String,
    pub provider: ProviderKind,
    pub target: PushTarget,
    pub notification: Notification,
    #[serde(default)]
    pub options: PushOptions,
    #[serde(default)]
    pub trace: TraceMetadata,
}

/// Stable outcome categories shared by HTTP, NATS, and database integrations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeClass {
    Accepted,
    InvalidToken,
    InvalidPayload,
    Throttled,
    TransientProviderFailure,
    PermanentProviderFailure,
    InternalFailure,
}

impl OutcomeClass {
    pub const fn is_retryable(self) -> bool {
        matches!(
            self,
            Self::Throttled | Self::TransientProviderFailure | Self::InternalFailure
        )
    }
}

/// Redacted provider-neutral result safe to publish or persist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PushOutcome {
    pub version: ContractVersion,
    pub job_id: String,
    pub provider: ProviderKind,
    pub target_fingerprint: TargetFingerprint,
    pub class: OutcomeClass,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safe_detail: Option<String>,
}

impl PushOutcome {
    pub fn accepted(job: &PushJob) -> Self {
        Self {
            version: job.version,
            job_id: job.job_id.clone(),
            provider: job.provider,
            target_fingerprint: job.target.fingerprint(),
            class: OutcomeClass::Accepted,
            provider_code: None,
            retry_after_ms: None,
            safe_detail: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_version_defaults_to_v1() {
        let encoded = r#"{
            "job_id":"job-1",
            "tenant_id":"tenant-1",
            "application_id":"app-1",
            "idempotency_key":"event-1:user-1",
            "provider":"fcm",
            "target":{"type":"fcm","token":"abc:def_123"},
            "notification":{"title":"Hello"}
        }"#;

        let job: PushJob = serde_json::from_str(encoded).expect("valid job");
        assert_eq!(job.version, ContractVersion::V1);
    }

    #[test]
    fn target_provider_is_derived_without_exposing_secret() {
        let target = PushTarget::Expo {
            token: "ExponentPushToken[secret]".to_owned(),
        };

        assert_eq!(target.provider(), ProviderKind::Expo);
        assert!(!target.fingerprint().as_str().contains("secret"));
    }

    #[test]
    fn accepted_outcome_is_redacted() {
        let job = PushJob {
            version: ContractVersion::V1,
            job_id: "job-1".to_owned(),
            tenant_id: "tenant-1".to_owned(),
            application_id: "app-1".to_owned(),
            idempotency_key: "event-1:user-1".to_owned(),
            provider: ProviderKind::Fcm,
            target: PushTarget::Fcm {
                token: "very-secret-token".to_owned(),
            },
            notification: Notification {
                title: Some("Hello".to_owned()),
                ..Notification::default()
            },
            options: PushOptions::default(),
            trace: TraceMetadata::default(),
        };

        let serialized = serde_json::to_string(&PushOutcome::accepted(&job)).expect("serialize");
        assert!(!serialized.contains("very-secret-token"));
        assert!(serialized.contains("target_fingerprint"));
    }
}
