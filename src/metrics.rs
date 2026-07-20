use std::sync::atomic::AtomicU64;

#[derive(Default)]
pub(crate) struct Metrics {
    pub(crate) plan_requests_total: AtomicU64,
    pub(crate) analysis_requests_total: AtomicU64,
    pub(crate) learning_requests_total: AtomicU64,
    pub(crate) generated_programs_total: AtomicU64,
    pub(crate) validation_findings_total: AtomicU64,
    pub(crate) failure_boundaries_total: AtomicU64,
    pub(crate) operator_actions_total: AtomicU64,
    pub(crate) fixture_release_blockers_total: AtomicU64,
    pub(crate) split_combine_reviews_total: AtomicU64,
    pub(crate) errors_total: AtomicU64,
    pub(crate) nats_messages_total: AtomicU64,
    pub(crate) nats_published_total: AtomicU64,
    pub(crate) nats_publish_failures_total: AtomicU64,
    pub(crate) nats_results_published_total: AtomicU64,
    pub(crate) mdp_published_total: AtomicU64,
    pub(crate) jobs_stored_total: AtomicU64,
    /// Stored jobs that replaced an existing job of the same id. Expected on a
    /// NATS redelivery; a sustained nonzero rate means distinct jobs are
    /// colliding and artifacts are being lost.
    pub(crate) jobs_displaced_total: AtomicU64,
    pub(crate) artifacts_stored_total: AtomicU64,
    pub(crate) artifact_requests_total: AtomicU64,
    pub(crate) learning_events_stored_total: AtomicU64,
    pub(crate) costing_result_reviews_total: AtomicU64,
}
