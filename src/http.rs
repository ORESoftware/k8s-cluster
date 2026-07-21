//! HTTP-only adapters for service health, readiness, and Prometheus metrics.
//!
//! Domain planning remains in the crate root for now; this module keeps Axum
//! response construction from leaking into configuration, persistence, and
//! metrics state modules.

use std::sync::atomic::Ordering;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

use crate::{AppState, SERVICE_NAME};

pub(super) async fn healthz() -> impl IntoResponse {
    Json(json!({ "ok": true, "service": SERVICE_NAME }))
}

pub(super) async fn readyz(State(state): State<AppState>) -> Response {
    let database_ready = state.persistence.is_ready().await;
    let status = if database_ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(json!({
            "ok": database_ready,
            "service": SERVICE_NAME,
            "checks": {
                "database": {
                    "client": "seaorm",
                    "enabled": state.persistence.is_enabled(),
                    "ready": database_ready,
                }
            }
        })),
    )
        .into_response()
}

pub(super) async fn metrics(State(state): State<AppState>) -> Response {
    // Gauges, not counters: a store read that fails reports zero rather than
    // failing the scrape, because /metrics going dark is worse than one stale
    // gauge — the counters below are what alerting keys off.
    let (current_jobs, current_artifacts) = state.jobs.counts().await.unwrap_or((0, 0));
    let current_learning_outcomes = state
        .learning
        .recent(crate::MAX_LEARNING_OUTCOMES)
        .await
        .map(|outcomes| outcomes.len())
        .unwrap_or(0);
    let mut body = format!(
        "# HELP dd_fabrication_server_plan_requests_total Fabrication plan requests received.\n\
         # TYPE dd_fabrication_server_plan_requests_total counter\n\
         dd_fabrication_server_plan_requests_total {}\n\
         # HELP dd_fabrication_server_analysis_requests_total Instruction analysis requests received.\n\
         # TYPE dd_fabrication_server_analysis_requests_total counter\n\
         dd_fabrication_server_analysis_requests_total {}\n\
         # HELP dd_fabrication_server_learning_requests_total Learning outcome requests received.\n\
         # TYPE dd_fabrication_server_learning_requests_total counter\n\
         dd_fabrication_server_learning_requests_total {}\n\
         # HELP dd_fabrication_server_generated_programs_total Draft machine programs generated.\n\
         # TYPE dd_fabrication_server_generated_programs_total counter\n\
         dd_fabrication_server_generated_programs_total {}\n\
         # HELP dd_fabrication_server_validation_findings_total Validation findings emitted.\n\
         # TYPE dd_fabrication_server_validation_findings_total counter\n\
         dd_fabrication_server_validation_findings_total {}\n\
         # HELP dd_fabrication_server_failure_boundaries_total Failure boundaries emitted.\n\
         # TYPE dd_fabrication_server_failure_boundaries_total counter\n\
         dd_fabrication_server_failure_boundaries_total {}\n\
         # HELP dd_fabrication_server_operator_actions_total Required operator intervention actions emitted by plan and instruction-analysis responses.\n\
         # TYPE dd_fabrication_server_operator_actions_total counter\n\
         dd_fabrication_server_operator_actions_total {}\n\
         # HELP dd_fabrication_server_fixture_release_blockers_total Fixture/setup release blockers emitted by plan responses.\n\
         # TYPE dd_fabrication_server_fixture_release_blockers_total counter\n\
         dd_fabrication_server_fixture_release_blockers_total {}\n\
         # HELP dd_fabrication_server_split_combine_reviews_total Split/combine decision or review records emitted before machine-ready release.\n\
         # TYPE dd_fabrication_server_split_combine_reviews_total counter\n\
         dd_fabrication_server_split_combine_reviews_total {}\n\
         # HELP dd_fabrication_server_errors_total Requests or background events that failed.\n\
         # TYPE dd_fabrication_server_errors_total counter\n\
         dd_fabrication_server_errors_total {}\n\
         # HELP dd_fabrication_server_nats_messages_total Fabrication requests received from NATS.\n\
         # TYPE dd_fabrication_server_nats_messages_total counter\n\
         dd_fabrication_server_nats_messages_total {}\n\
         # HELP dd_fabrication_server_nats_published_total NATS messages published by the fabrication server.\n\
         # TYPE dd_fabrication_server_nats_published_total counter\n\
         dd_fabrication_server_nats_published_total {}\n\
         # HELP dd_fabrication_server_nats_publish_failures_total NATS publishes or broker flushes that failed.\n\
         # TYPE dd_fabrication_server_nats_publish_failures_total counter\n\
         dd_fabrication_server_nats_publish_failures_total {}\n\
         # HELP dd_fabrication_server_nats_results_published_total Fabrication result messages published to NATS.\n\
         # TYPE dd_fabrication_server_nats_results_published_total counter\n\
         dd_fabrication_server_nats_results_published_total {}\n\
         # HELP dd_fabrication_server_mdp_published_total MDP optimization requests published for fabrication policy learning.\n\
         # TYPE dd_fabrication_server_mdp_published_total counter\n\
         dd_fabrication_server_mdp_published_total {}\n\
         # HELP dd_fabrication_server_jobs_stored_total Fabrication jobs written to the shared job ledger (daedalus.fab_jobs, or the in-process store when no database is configured).\n\
         # TYPE dd_fabrication_server_jobs_stored_total counter\n\
         dd_fabrication_server_jobs_stored_total {}\n\
         # HELP dd_fabrication_server_jobs_displaced_total Stored jobs that replaced an existing job of the same id (expected on NATS redelivery; sustained nonzero means distinct jobs are colliding).\n\
         # TYPE dd_fabrication_server_jobs_displaced_total counter\n\
         dd_fabrication_server_jobs_displaced_total {}\n\
         # HELP dd_fabrication_server_artifacts_stored_total Fabrication artifacts written to the shared job ledger.\n\
         # TYPE dd_fabrication_server_artifacts_stored_total counter\n\
         dd_fabrication_server_artifacts_stored_total {}\n\
         # HELP dd_fabrication_server_artifact_requests_total Artifact detail requests served by the fabrication server.\n\
         # TYPE dd_fabrication_server_artifact_requests_total counter\n\
         dd_fabrication_server_artifact_requests_total {}\n\
         # HELP dd_fabrication_server_learning_events_stored_total Distinct learning outcomes written to the shared outcome store; a redelivered outcome upserts and is not counted twice.\n\
         # TYPE dd_fabrication_server_learning_events_stored_total counter\n\
         dd_fabrication_server_learning_events_stored_total {}\n\
         # HELP dd_fabrication_server_costing_result_reviews_total Costing result review submissions accepted for cost, yield, scrap, and split/combine route learning.\n\
         # TYPE dd_fabrication_server_costing_result_reviews_total counter\n\
         dd_fabrication_server_costing_result_reviews_total {}\n\
         # HELP dd_fabrication_server_current_jobs Jobs currently inside the ledger's retention window.\n\
         # TYPE dd_fabrication_server_current_jobs gauge\n\
         dd_fabrication_server_current_jobs {}\n\
         # HELP dd_fabrication_server_current_artifacts Artifacts currently inside the ledger's retention window.\n\
         # TYPE dd_fabrication_server_current_artifacts gauge\n\
         dd_fabrication_server_current_artifacts {}\n\
         # HELP dd_fabrication_server_current_learning_outcomes Outcomes currently inside the learning store's retention window.\n\
         # TYPE dd_fabrication_server_current_learning_outcomes gauge\n\
         dd_fabrication_server_current_learning_outcomes {}\n",
        state.metrics.plan_requests_total.load(Ordering::Relaxed),
        state
            .metrics
            .analysis_requests_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .learning_requests_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .generated_programs_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .validation_findings_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .failure_boundaries_total
            .load(Ordering::Relaxed),
        state.metrics.operator_actions_total.load(Ordering::Relaxed),
        state
            .metrics
            .fixture_release_blockers_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .split_combine_reviews_total
            .load(Ordering::Relaxed),
        state.metrics.errors_total.load(Ordering::Relaxed),
        state.metrics.nats_messages_total.load(Ordering::Relaxed),
        state.metrics.nats_published_total.load(Ordering::Relaxed),
        state
            .metrics
            .nats_publish_failures_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .nats_results_published_total
            .load(Ordering::Relaxed),
        state.metrics.mdp_published_total.load(Ordering::Relaxed),
        state.metrics.jobs_stored_total.load(Ordering::Relaxed),
        state.metrics.jobs_displaced_total.load(Ordering::Relaxed),
        state.metrics.artifacts_stored_total.load(Ordering::Relaxed),
        state
            .metrics
            .artifact_requests_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .learning_events_stored_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .costing_result_reviews_total
            .load(Ordering::Relaxed),
        current_jobs,
        current_artifacts,
        current_learning_outcomes,
    );
    body.push_str(&format!(
        "# HELP dd_fabrication_server_persistence_enabled Whether the SeaORM Postgres persistence connection is configured.\n\
         # TYPE dd_fabrication_server_persistence_enabled gauge\n\
         dd_fabrication_server_persistence_enabled {}\n\
         # HELP dd_fabrication_server_realtime_events_published_total Events published to HTML, JSON, TCP, and NATS realtime adapters.\n\
         # TYPE dd_fabrication_server_realtime_events_published_total counter\n\
         dd_fabrication_server_realtime_events_published_total {}\n\
         # HELP dd_fabrication_server_realtime_subscribers Current in-process realtime adapter subscribers.\n\
         # TYPE dd_fabrication_server_realtime_subscribers gauge\n\
         dd_fabrication_server_realtime_subscribers {}\n",
        u8::from(state.persistence.is_enabled()),
        state.realtime.published_total(),
        state.realtime.subscriber_count(),
    ));
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use std::sync::Arc;
    use tokio::sync::Semaphore;

    use crate::{
        coordination::{Coordination, NoopCoordination, DEFAULT_LEASE_TTL_MS},
        metrics::Metrics,
        persistence::Persistence,
        realtime::{EventHub, ServiceSurface},
        FABRICATION_REQUESTS_QUEUE_GROUP, FABRICATION_REQUESTS_SUBJECT,
        FABRICATION_RESULTS_SUBJECT, MDP_OPTIMIZE_SUBJECT, RUNTIME_EVENTS_SUBJECT,
    };

    fn test_state() -> AppState {
        AppState {
            verifier: None,
            http: reqwest::Client::new(),
            nats: None,
            persistence: Persistence::Disabled,
            realtime: EventHub::new(ServiceSurface::Fabrication, 8),
            request_subject: FABRICATION_REQUESTS_SUBJECT.to_string(),
            queue_group: FABRICATION_REQUESTS_QUEUE_GROUP.to_string(),
            result_subject: FABRICATION_RESULTS_SUBJECT.to_string(),
            event_subject: RUNTIME_EVENTS_SUBJECT.to_string(),
            mdp_subject: MDP_OPTIMIZE_SUBJECT.to_string(),
            mdp_autopublish: false,
            nats_inflight: Arc::new(Semaphore::new(1)),
            coordination: Arc::new(NoopCoordination::default()) as Arc<dyn Coordination>,
            lease_ttl: std::time::Duration::from_millis(DEFAULT_LEASE_TTL_MS),
            metrics: Arc::new(Metrics::default()),
            jobs: Arc::new(crate::stores::InMemoryJobStore::default()),
            learning: Arc::new(crate::stores::InMemoryLearningStore::default()),
        }
    }

    async fn response_body(response: Response) -> String {
        String::from_utf8(
            to_bytes(response.into_body(), 64 * 1024)
                .await
                .expect("read bounded response body")
                .to_vec(),
        )
        .expect("response body is UTF-8")
    }

    #[tokio::test]
    async fn health_response_is_json_and_successful() {
        let response = healthz().await.into_response();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(axum::http::header::CONTENT_TYPE),
            Some(&axum::http::HeaderValue::from_static("application/json"))
        );
        let body = response_body(response).await;
        assert!(body.contains("\"ok\":true"));
        assert!(body.contains(SERVICE_NAME));
    }

    #[tokio::test]
    async fn disabled_database_is_ready_without_hiding_seaorm_contract() {
        let response = readyz(State(test_state())).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_body(response).await;
        assert!(body.contains("\"client\":\"seaorm\""));
        assert!(body.contains("\"enabled\":false"));
        assert!(body.contains("\"ready\":true"));
    }

    #[tokio::test]
    async fn prometheus_response_exposes_counters_and_persistence_gauge() {
        let state = test_state();
        state
            .metrics
            .plan_requests_total
            .store(7, Ordering::Relaxed);
        state.realtime.publish_payload(
            "test",
            "printer.preflight.completed",
            json!({"releaseReady": true}),
        );
        let response = metrics(State(state)).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(axum::http::header::CONTENT_TYPE),
            Some(&axum::http::HeaderValue::from_static(
                "text/plain; version=0.0.4"
            ))
        );
        let body = response_body(response).await;
        assert!(body.contains("dd_fabrication_server_plan_requests_total 7"));
        assert!(body.contains("dd_fabrication_server_persistence_enabled 0"));
        assert!(body.contains("dd_fabrication_server_realtime_events_published_total 1"));
    }
}
