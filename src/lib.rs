//! Provider-neutral contracts, provider adapters, and authenticated ingestion boundaries.
//!
//! Provider adapters implement [`provider::PushProvider`] and consume the
//! versioned types in [`contracts`]. HTTP, NATS, and database integrations depend
//! on these contracts rather than provider-specific request shapes.

pub mod contracts;
pub mod dispatch;
pub mod http_api;
pub mod nats_ingestion;
pub mod provider;
pub mod providers;
pub mod redaction;
pub mod retry;
pub mod runtime;
pub mod validation;

pub use contracts::{
    ContractVersion, Notification, OutcomeClass, ProviderEnvironment, ProviderKind, PushJob,
    PushOptions, PushOutcome, PushPriority, PushTarget, TraceMetadata,
};
pub use dispatch::{
    ProviderReadinessView, ProviderRegistry, ProviderSlot, RegistryError, RegistryReadiness,
};
pub use http_api::{
    ApiState, AuthConfigError, BatchRequest, BatchResponse, DenyAllAuthenticator,
    RequestAuthenticator, SharedSecretAuthenticator, router,
};
pub use nats_ingestion::{
    NatsConfig, NatsConfigError, NatsDisposition, PushDeadLetterEventV1, PushJobEnvelopeV1,
    PushResultEventV1, disposition_for_outcome, run_nats_consumer,
};
pub use provider::{ProviderError, ProviderReadiness, PushProvider};
pub use providers::apns::{ApnsConfig, ApnsConfigError, ApnsProvider};
pub use providers::expo::{ExpoConfig, ExpoConfigError, ExpoProvider, ExpoReceiptRequest};
pub use providers::fcm::{FcmConfig, FcmConfigError, FcmProvider};
pub use providers::web_push::{
    WebPushConfig, WebPushConfigError, WebPushHostPolicy, WebPushProvider, redact_web_push_endpoint,
};
pub use redaction::{TargetFingerprint, fingerprint_target, truncate_utf8};
pub use retry::{RetryDecision, classify_http_status, parse_retry_after};
pub use runtime::{RuntimeConfigError, provider_registry_from_env, request_authenticator_from_env};
pub use validation::{ValidationError, validate_push_job};
