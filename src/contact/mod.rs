//! Optional email and SMS delivery lanes.
//!
//! These contracts are intentionally separate from `PushJob`: adding SendGrid
//! or Twilio fallback channels must not broaden push-target validation or allow
//! producers to supply provider credentials and sender identities.

pub mod contracts;
pub mod dispatch;
pub mod http;
pub mod provider;
pub mod runtime;
pub mod sendgrid;
pub mod twilio;
pub mod validation;

pub use contracts::{
    ContactContent, ContactJob, ContactOutcome, ContactOutcomeClass, ContactProviderKind,
    ContactTarget, ContactTargetFingerprint,
};
pub use dispatch::{
    ContactProviderReadinessView, ContactProviderRegistry, ContactRegistryReadiness,
};
pub use http::{ContactApiState, contact_router};
pub use provider::{ContactProvider, ContactProviderError};
pub use runtime::{ContactRuntimeConfigError, contact_registry_from_env};
pub use sendgrid::{
    SendGridConfig, SendGridConfigError, SendGridProvider, SendGridRegion,
};
pub use twilio::{
    TwilioConfig, TwilioConfigError, TwilioCredentials, TwilioProvider, TwilioSender,
};
pub use validation::{
    ContactValidationError, valid_e164, valid_email_address, validate_contact_job,
};
