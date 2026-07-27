use std::time::Duration;

use async_trait::async_trait;
use thiserror::Error;

use crate::contracts::{OutcomeClass, ProviderKind, PushJob, PushOutcome};
use crate::redaction::truncate_utf8;

const MAX_SAFE_ERROR_BYTES: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderReadiness {
    pub configured: bool,
    pub safe_reason: Option<String>,
}

impl ProviderReadiness {
    pub const fn ready() -> Self {
        Self {
            configured: true,
            safe_reason: None,
        }
    }

    pub fn not_ready(reason: impl AsRef<str>) -> Self {
        Self {
            configured: false,
            safe_reason: Some(truncate_utf8(reason.as_ref(), MAX_SAFE_ERROR_BYTES)),
        }
    }
}

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("provider is not configured: {safe_reason}")]
    NotConfigured { safe_reason: String },

    #[error("provider delivery failed ({class:?}): {safe_detail}")]
    Delivery {
        class: OutcomeClass,
        safe_detail: String,
        retry_after: Option<Duration>,
        provider_code: Option<String>,
    },

    #[error("internal provider adapter failure: {safe_detail}")]
    Internal { safe_detail: String },
}

impl ProviderError {
    pub fn not_configured(reason: impl AsRef<str>) -> Self {
        Self::NotConfigured {
            safe_reason: truncate_utf8(reason.as_ref(), MAX_SAFE_ERROR_BYTES),
        }
    }

    pub fn delivery(
        class: OutcomeClass,
        detail: impl AsRef<str>,
        retry_after: Option<Duration>,
        provider_code: Option<String>,
    ) -> Self {
        Self::Delivery {
            class,
            safe_detail: truncate_utf8(detail.as_ref(), MAX_SAFE_ERROR_BYTES),
            retry_after,
            provider_code,
        }
    }

    pub fn internal(detail: impl AsRef<str>) -> Self {
        Self::Internal {
            safe_detail: truncate_utf8(detail.as_ref(), MAX_SAFE_ERROR_BYTES),
        }
    }
}

#[async_trait]
pub trait PushProvider: Send + Sync {
    fn kind(&self) -> ProviderKind;

    fn readiness(&self) -> ProviderReadiness;

    async fn send(&self, job: &PushJob) -> Result<PushOutcome, ProviderError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_and_errors_bound_safe_text() {
        let long = "é".repeat(600);
        let readiness = ProviderReadiness::not_ready(&long);
        let reason = readiness.safe_reason.expect("reason");
        assert!(reason.len() <= MAX_SAFE_ERROR_BYTES);
        assert!(std::str::from_utf8(reason.as_bytes()).is_ok());

        let error = ProviderError::internal(&long).to_string();
        assert!(error.len() < long.len());
    }
}
