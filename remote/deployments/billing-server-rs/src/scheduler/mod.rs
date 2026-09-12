//! Durable, PG-backed scheduler ("bulletproof cron").
//!
//! Pattern:
//!   - `scheduled_jobs` is the catalog of *what should run when*.
//!   - The runner loop atomically claims due rows via FOR UPDATE SKIP LOCKED,
//!     ensuring exactly-one execution per due tick across N pods.
//!   - Each attempt is recorded in `job_runs` (the durable history).
//!   - Failures are retried with exponential backoff; after `max_attempts`
//!     the failure is copied into `dead_letter_jobs` for ops visibility.
//!
//! Handlers are registered by job `kind` string. System jobs (lock sweeper,
//! anchor sweeper, notification rule evaluator) are registered at startup
//! via `register_builtins`. Tenants register custom jobs (e.g.
//! `tenant.payroll_run`) — those are dispatched as signed outbound webhooks
//! to the tenant's registered endpoint.

pub mod handler;
pub mod runner;
pub mod service;
pub mod types;

pub use handler::{HandlerRegistry, HandlerRegistryBuilder, JobContext, JobHandler, JobOutput};
pub use runner::SchedulerRunner;
pub use service::SchedulerService;
pub use types::*;
