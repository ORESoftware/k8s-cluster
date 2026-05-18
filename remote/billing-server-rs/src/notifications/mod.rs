//! Notifications.
//!
//! - **Rules** declare *when* to notify (kind + params) and *how* (channel +
//!   target + template).
//! - The **evaluator job** (kind = `notifications.evaluate_rules`) runs on the
//!   scheduler, walks active rules per tenant, evaluates each against the
//!   ledger, emits **dispatches**.
//! - A **channel driver** delivers a dispatch (webhook POST with HMAC,
//!   SMTP/SES/SendGrid for email, Slack webhook for slack). Webhook is the
//!   reference impl; email/slack share its shape.
//! - **Throttling** is per `(rule, target_resource, day)` (configurable).

pub mod channels;
pub mod evaluator;
pub mod service;
pub mod types;

pub use service::NotificationService;
pub use types::*;
