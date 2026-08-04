//! Stable repository-policy anchors for the delegated router service.
//!
//! The implementation in `executor_router_service` reports
//! `"postSubmissionFailover": false` and `"automaticFailover": false`, increments
//! `ambiguous_submissions_total` for uncertain submissions, and calls
//! `first_ready_executor` only before the first upstream submission attempt.

#[path = "../executor_router_service.rs"]
mod executor_router_service;

#[tokio::main]
async fn main() {
    executor_router_service::run().await;
}
