//! Provider-neutral contracts and safety boundaries for push delivery.
//!
//! Provider adapters implement [`provider::PushProvider`] and consume the
//! versioned types in [`contracts`]. HTTP, NATS, and database integrations must
//! depend on these contracts rather than provider-specific request shapes.

pub mod contracts;
pub mod provider;
pub mod providers;
pub mod redaction;
pub mod retry;
pub mod validation;

pub use contracts::{
    ContractVersion, Notification, ProviderEnvironment, ProviderKind, PushJob, PushOptions,
    PushOutcome, PushPriority, PushTarget, TraceMetadata,
};
pub use provider::{ProviderError, ProviderReadiness, PushProvider};
pub use providers::apns::{ApnsConfig, ApnsConfigError, ApnsProvider};
pub use providers::fcm::{FcmConfig, FcmConfigError, FcmProvider};
pub use redaction::{TargetFingerprint, fingerprint_target, truncate_utf8};
pub use retry::{RetryDecision, classify_http_status, parse_retry_after};
pub use validation::{ValidationError, validate_push_job};
