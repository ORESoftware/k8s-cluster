use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use utoipa::ToSchema;

use crate::contracts::{ContractVersion, TraceMetadata};

const FINGERPRINT_HEX_LENGTH: usize = 24;

/// Optional non-push providers supported by the notification service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContactProviderKind {
    Sendgrid,
    Twilio,
}

impl ContactProviderKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sendgrid => "sendgrid",
            Self::Twilio => "twilio",
        }
    }
}

/// Recipient capability. Raw addresses and phone numbers must never be logged or
/// copied into normalized outcomes; use [`ContactTarget::fingerprint`] instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContactTarget {
    Email {
        address: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    Sms {
        e164: String,
    },
}

impl ContactTarget {
    pub const fn provider(&self) -> ContactProviderKind {
        match self {
            Self::Email { .. } => ContactProviderKind::Sendgrid,
            Self::Sms { .. } => ContactProviderKind::Twilio,
        }
    }

    pub fn fingerprint(&self) -> ContactTargetFingerprint {
        let (provider, parts): (&str, Vec<&str>) = match self {
            Self::Email { address, .. } => ("email", vec![address.as_str()]),
            Self::Sms { e164 } => ("sms", vec![e164.as_str()]),
        };
        let mut hasher = Sha256::new();
        hasher.update(b"contact-target-v1\0");
        hasher.update(provider.as_bytes());
        for part in parts {
            hasher.update(b"\0");
            hasher.update(part.as_bytes());
        }
        let digest = hex::encode(hasher.finalize());
        ContactTargetFingerprint(format!("{provider}:{}", &digest[..FINGERPRINT_HEX_LENGTH]))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(transparent)]
pub struct ContactTargetFingerprint(String);

impl ContactTargetFingerprint {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ContactTargetFingerprint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Channel-specific content. Senders and provider credentials are server-side
/// configuration and can never be supplied by producers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "channel", rename_all = "snake_case")]
pub enum ContactContent {
    Email {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subject: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        html: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        template_id: Option<String>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        dynamic_template_data: BTreeMap<String, Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reply_to: Option<String>,
    },
    Sms {
        body: String,
    },
}

impl ContactContent {
    pub const fn provider(&self) -> ContactProviderKind {
        match self {
            Self::Email { .. } => ContactProviderKind::Sendgrid,
            Self::Sms { .. } => ContactProviderKind::Twilio,
        }
    }
}

/// Versioned email/SMS request contract. This is deliberately separate from
/// `PushJob` so adding fallback channels cannot weaken push-target validation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ContactJob {
    #[serde(default)]
    pub version: ContractVersion,
    pub job_id: String,
    pub tenant_id: String,
    pub application_id: String,
    pub idempotency_key: String,
    pub provider: ContactProviderKind,
    pub target: ContactTarget,
    pub content: ContactContent,
    #[serde(default)]
    pub trace: TraceMetadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContactOutcomeClass {
    Accepted,
    InvalidTarget,
    InvalidPayload,
    Throttled,
    TransientProviderFailure,
    PermanentProviderFailure,
    InternalFailure,
}

impl ContactOutcomeClass {
    pub const fn is_retryable(self) -> bool {
        matches!(
            self,
            Self::Throttled | Self::TransientProviderFailure | Self::InternalFailure
        )
    }
}

/// Capability-safe result suitable for HTTP responses, NATS events, and audit
/// persistence. The provider code is an opaque SendGrid message ID, Twilio SID,
/// or bounded provider error code, never the recipient.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ContactOutcome {
    pub version: ContractVersion,
    pub job_id: String,
    pub provider: ContactProviderKind,
    pub target_fingerprint: ContactTargetFingerprint,
    pub class: ContactOutcomeClass,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safe_detail: Option<String>,
}

impl ContactOutcome {
    pub fn accepted(job: &ContactJob, provider_code: Option<String>) -> Self {
        Self {
            version: job.version,
            job_id: job.job_id.clone(),
            provider: job.provider,
            target_fingerprint: job.target.fingerprint(),
            class: ContactOutcomeClass::Accepted,
            provider_code,
            retry_after_ms: None,
            safe_detail: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contact_target_fingerprints_do_not_expose_capabilities() {
        let email = ContactTarget::Email {
            address: "person@example.com".to_owned(),
            name: None,
        };
        let sms = ContactTarget::Sms {
            e164: "+15551234567".to_owned(),
        };
        assert!(!email.fingerprint().as_str().contains("person"));
        assert!(!sms.fingerprint().as_str().contains("5551234567"));
        assert_ne!(email.fingerprint(), sms.fingerprint());
    }

    #[test]
    fn providers_are_derived_from_target_and_content() {
        let target = ContactTarget::Sms {
            e164: "+15551234567".to_owned(),
        };
        let content = ContactContent::Sms {
            body: "Hello".to_owned(),
        };
        assert_eq!(target.provider(), ContactProviderKind::Twilio);
        assert_eq!(content.provider(), ContactProviderKind::Twilio);
    }
}
