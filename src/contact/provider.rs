use std::time::Duration;

use async_trait::async_trait;
use thiserror::Error;

use super::contracts::{ContactJob, ContactOutcome, ContactOutcomeClass, ContactProviderKind};
use crate::provider::ProviderReadiness;
use crate::redaction::truncate_utf8;

const MAX_SAFE_ERROR_BYTES: usize = 512;

#[derive(Debug, Error)]
pub enum ContactProviderError {
    #[error("provider is not configured: {safe_reason}")]
    NotConfigured { safe_reason: String },

    #[error("provider delivery failed ({class:?}): {safe_detail}")]
    Delivery {
        class: ContactOutcomeClass,
        safe_detail: String,
        retry_after: Option<Duration>,
        provider_code: Option<String>,
    },

    #[error("internal contact provider failure: {safe_detail}")]
    Internal { safe_detail: String },
}

impl ContactProviderError {
    pub fn not_configured(reason: impl AsRef<str>) -> Self {
        Self::NotConfigured {
            safe_reason: truncate_utf8(reason.as_ref(), MAX_SAFE_ERROR_BYTES),
        }
    }

    pub fn delivery(
        class: ContactOutcomeClass,
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
pub trait ContactProvider: Send + Sync {
    fn kind(&self) -> ContactProviderKind;

    fn readiness(&self) -> ProviderReadiness;

    async fn send(&self, job: &ContactJob) -> Result<ContactOutcome, ContactProviderError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_errors_bound_safe_text() {
        let long = "é".repeat(600);
        let error = ContactProviderError::internal(&long).to_string();
        assert!(error.len() < long.len());
        assert!(std::str::from_utf8(error.as_bytes()).is_ok());
    }
}
