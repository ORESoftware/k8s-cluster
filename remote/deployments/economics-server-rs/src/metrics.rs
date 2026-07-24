use std::sync::atomic::{AtomicU64, Ordering};

use axum::{
    extract::State,
    response::{IntoResponse, Response},
};

use crate::state::*;

#[derive(Default)]
pub(crate) struct Metrics {
    pub(crate) http_requests_total: AtomicU64,
    pub(crate) forecasts_total: AtomicU64,
    pub(crate) ingest_requests_total: AtomicU64,
    pub(crate) source_pull_total: AtomicU64,
    pub(crate) source_pull_success_total: AtomicU64,
    pub(crate) source_pull_failure_total: AtomicU64,
    pub(crate) source_pull_bytes_total: AtomicU64,
    pub(crate) source_pull_stored_points_total: AtomicU64,
    pub(crate) source_pull_last_success_unix_seconds: AtomicU64,
    pub(crate) sentiment_requests_total: AtomicU64,
    pub(crate) recommendation_requests_total: AtomicU64,
    pub(crate) pipeline_plan_requests_total: AtomicU64,
    pub(crate) pipeline_submit_requests_total: AtomicU64,
    pub(crate) pipeline_publish_attempts_total: AtomicU64,
    pub(crate) pipeline_publish_success_total: AtomicU64,
    pub(crate) pipeline_publish_failure_total: AtomicU64,
    pub(crate) pipeline_submit_success_total: AtomicU64,
    pub(crate) pipeline_submit_failure_total: AtomicU64,
    pub(crate) integration_health_requests_total: AtomicU64,
    pub(crate) observability_requests_total: AtomicU64,
    pub(crate) auth_failures_total: AtomicU64,
    pub(crate) errors_total: AtomicU64,
    pub(crate) nats_messages_total: AtomicU64,
    pub(crate) nats_published_total: AtomicU64,
}

pub(crate) async fn metrics(State(state): State<AppState>) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let body = format!(
        "# HELP dd_economics_server_http_requests_total HTTP requests observed by the economics service.\n\
         # TYPE dd_economics_server_http_requests_total counter\n\
         dd_economics_server_http_requests_total {}\n\
         # HELP dd_economics_server_forecasts_total Forecasts generated.\n\
         # TYPE dd_economics_server_forecasts_total counter\n\
         dd_economics_server_forecasts_total {}\n\
         # HELP dd_economics_server_ingest_requests_total Ingest requests accepted.\n\
         # TYPE dd_economics_server_ingest_requests_total counter\n\
         dd_economics_server_ingest_requests_total {}\n\
         # HELP dd_economics_server_source_pull_total Source pull requests attempted.\n\
         # TYPE dd_economics_server_source_pull_total counter\n\
         dd_economics_server_source_pull_total {}\n\
         # HELP dd_economics_server_source_pull_success_total Source pull requests that fetched and parsed/stored or fetched successfully.\n\
         # TYPE dd_economics_server_source_pull_success_total counter\n\
         dd_economics_server_source_pull_success_total {}\n\
         # HELP dd_economics_server_source_pull_failure_total Source pull requests rejected or failed before a successful response.\n\
         # TYPE dd_economics_server_source_pull_failure_total counter\n\
         dd_economics_server_source_pull_failure_total {}\n\
         # HELP dd_economics_server_source_pull_bytes_total Total response bytes fetched by successful source pulls.\n\
         # TYPE dd_economics_server_source_pull_bytes_total counter\n\
         dd_economics_server_source_pull_bytes_total {}\n\
         # HELP dd_economics_server_source_pull_stored_points_total Total normalized observations stored by source pulls.\n\
         # TYPE dd_economics_server_source_pull_stored_points_total counter\n\
         dd_economics_server_source_pull_stored_points_total {}\n\
         # HELP dd_economics_server_source_pull_last_success_unix_seconds Unix timestamp of the latest successful source pull.\n\
         # TYPE dd_economics_server_source_pull_last_success_unix_seconds gauge\n\
         dd_economics_server_source_pull_last_success_unix_seconds {}\n\
         # HELP dd_economics_server_sentiment_requests_total Sentiment analysis requests accepted.\n\
         # TYPE dd_economics_server_sentiment_requests_total counter\n\
         dd_economics_server_sentiment_requests_total {}\n\
         # HELP dd_economics_server_recommendation_requests_total Recommendation requests accepted.\n\
         # TYPE dd_economics_server_recommendation_requests_total counter\n\
         dd_economics_server_recommendation_requests_total {}\n\
         # HELP dd_economics_server_pipeline_plan_requests_total Pipeline plan requests accepted.\n\
         # TYPE dd_economics_server_pipeline_plan_requests_total counter\n\
         dd_economics_server_pipeline_plan_requests_total {}\n\
         # HELP dd_economics_server_pipeline_submit_requests_total Pipeline submit requests accepted.\n\
         # TYPE dd_economics_server_pipeline_submit_requests_total counter\n\
         dd_economics_server_pipeline_submit_requests_total {}\n\
         # HELP dd_economics_server_pipeline_publish_attempts_total Pipeline plan NATS publish attempts requested.\n\
         # TYPE dd_economics_server_pipeline_publish_attempts_total counter\n\
         dd_economics_server_pipeline_publish_attempts_total {}\n\
         # HELP dd_economics_server_pipeline_publish_success_total Pipeline plans published to NATS successfully.\n\
         # TYPE dd_economics_server_pipeline_publish_success_total counter\n\
         dd_economics_server_pipeline_publish_success_total {}\n\
         # HELP dd_economics_server_pipeline_publish_failure_total Pipeline plan publish attempts skipped or failed.\n\
         # TYPE dd_economics_server_pipeline_publish_failure_total counter\n\
         dd_economics_server_pipeline_publish_failure_total {}\n\
         # HELP dd_economics_server_pipeline_submit_success_total Spark pipeline jobs accepted by the pipeline server.\n\
         # TYPE dd_economics_server_pipeline_submit_success_total counter\n\
         dd_economics_server_pipeline_submit_success_total {}\n\
         # HELP dd_economics_server_pipeline_submit_failure_total Spark pipeline job submits rejected or failed before submit.\n\
         # TYPE dd_economics_server_pipeline_submit_failure_total counter\n\
         dd_economics_server_pipeline_submit_failure_total {}\n\
         # HELP dd_economics_server_integration_health_requests_total Integration health requests served.\n\
         # TYPE dd_economics_server_integration_health_requests_total counter\n\
         dd_economics_server_integration_health_requests_total {}\n\
         # HELP dd_economics_server_observability_requests_total Observability descriptor requests served.\n\
         # TYPE dd_economics_server_observability_requests_total counter\n\
         dd_economics_server_observability_requests_total {}\n\
         # HELP dd_economics_server_auth_failures_total Rejected requests with missing or invalid auth.\n\
         # TYPE dd_economics_server_auth_failures_total counter\n\
         dd_economics_server_auth_failures_total {}\n\
         # HELP dd_economics_server_errors_total Forecast, ingest, source, or publish errors.\n\
         # TYPE dd_economics_server_errors_total counter\n\
         dd_economics_server_errors_total {}\n\
         # HELP dd_economics_server_nats_messages_total NATS forecast requests consumed.\n\
         # TYPE dd_economics_server_nats_messages_total counter\n\
         dd_economics_server_nats_messages_total {}\n\
         # HELP dd_economics_server_nats_published_total NATS messages published.\n\
         # TYPE dd_economics_server_nats_published_total counter\n\
         dd_economics_server_nats_published_total {}\n",
        state.metrics.http_requests_total.load(Ordering::Relaxed),
        state.metrics.forecasts_total.load(Ordering::Relaxed),
        state.metrics.ingest_requests_total.load(Ordering::Relaxed),
        state.metrics.source_pull_total.load(Ordering::Relaxed),
        state
            .metrics
            .source_pull_success_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .source_pull_failure_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .source_pull_bytes_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .source_pull_stored_points_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .source_pull_last_success_unix_seconds
            .load(Ordering::Relaxed),
        state.metrics.sentiment_requests_total.load(Ordering::Relaxed),
        state
            .metrics
            .recommendation_requests_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .pipeline_plan_requests_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .pipeline_submit_requests_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .pipeline_publish_attempts_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .pipeline_publish_success_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .pipeline_publish_failure_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .pipeline_submit_success_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .pipeline_submit_failure_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .integration_health_requests_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .observability_requests_total
            .load(Ordering::Relaxed),
        state.metrics.auth_failures_total.load(Ordering::Relaxed),
        state.metrics.errors_total.load(Ordering::Relaxed),
        state.metrics.nats_messages_total.load(Ordering::Relaxed),
        state.metrics.nats_published_total.load(Ordering::Relaxed),
    );
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        body,
    )
        .into_response()
}
