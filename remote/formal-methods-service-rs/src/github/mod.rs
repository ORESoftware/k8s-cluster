//! GitHub integration: webhook event types and a small outbound API client.

pub mod client;
pub mod types;

pub use client::{redact_url, GithubClient};
pub use types::{PullRequestAction, PullRequestEvent};
