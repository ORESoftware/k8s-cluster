use std::sync::atomic::Ordering;

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    Json,
};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use serde_json::{json, Value};

use crate::redis_support::redis_ping;
use crate::shared::{require_auth, unix_seconds};
use crate::state::{AppResult, AppState, DEFAULT_JOB_STREAM, SERVICE_NAME};
pub(crate) async fn descriptor() -> Json<Value> {
    Json(json!({
        "service": SERVICE_NAME,
        "version": env!("CARGO_PKG_VERSION"),
        "docs": "/docs/api",
        "health": "/healthz",
        "ready": "/readyz",
        "capabilities": "/capabilities"
    }))
}

pub(crate) async fn capabilities() -> Json<Value> {
    Json(json!({
        "service": SERVICE_NAME,
        "modules": [
            "clientManagement",
            "leadGeneration",
            "campaignManagement",
            "crmSync",
            "outreachSequencing",
            "marketingAutomation",
            "analyticsReporting",
            "conversionTracking",
            "salesPipeline",
            "prospectResearch",
            "contentOperations",
            "projectManagement",
            "clientCommunication",
            "clientPortal",
            "documentSharing",
            "agencyOperations",
            "profitabilityTracking",
            "callIntelligence"
        ],
        "channels": ["socialMedia", "seoAeo", "email", "linkedin", "sms", "paidAds", "content"],
        "integrations": [
            "salesforce",
            "hubspot",
            "apollo",
            "zoominfo",
            "googleAnalytics",
            "googleAds",
            "linkedinAds",
            "metaAds",
            "mailchimp",
            "sendgrid",
            "externalScraper"
        ],
        "storage": {
            "database": "postgres",
            "orm": "sea-orm via remote/libs/pg-defs/generated/rust/sea-orm",
            "tablePrefix": "benefactor_marketing_",
            "cache": "redis dashboard cache",
            "rateLimits": "redis per-actor counters",
            "jobStream": DEFAULT_JOB_STREAM
        }
    }))
}

pub(crate) async fn healthz() -> Json<Value> {
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "timeUnix": unix_seconds()
    }))
}

pub(crate) async fn readyz(State(state): State<AppState>) -> Response {
    let database_ready = state
        .db
        .execute(Statement::from_string(
            DatabaseBackend::Postgres,
            "select 1".to_string(),
        ))
        .await
        .is_ok();
    let redis_configured = state.redis.is_some();
    let redis_ready = if redis_configured {
        redis_ping(&state).await
    } else {
        false
    };
    let ready = database_ready
        && (!state.cfg.redis_required_for_ready || (redis_configured && redis_ready));
    let status = if ready {
        StatusCode::OK
    } else {
        if !database_ready {
            state
                .metrics
                .db_errors_total
                .fetch_add(1, Ordering::Relaxed);
        }
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(json!({
            "ok": ready,
            "service": SERVICE_NAME,
            "database": if database_ready { "ready" } else { "unavailable" },
            "redis": {
                "configured": redis_configured,
                "requiredForReady": state.cfg.redis_required_for_ready,
                "status": if redis_configured {
                    if redis_ready { "ready" } else { "unavailable" }
                } else {
                    "disabled"
                }
            }
        })),
    )
        .into_response()
}

pub(crate) async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    let uptime = state.started_at.elapsed().as_secs();
    let body = format!(
        "# HELP benefactor_marketing_uptime_seconds Process uptime in seconds.\n\
# TYPE benefactor_marketing_uptime_seconds gauge\n\
benefactor_marketing_uptime_seconds {}\n\
# HELP benefactor_marketing_mutations_total Domain mutations accepted by the backend.\n\
# TYPE benefactor_marketing_mutations_total counter\n\
benefactor_marketing_mutations_total {}\n\
# HELP benefactor_marketing_enrichment_jobs_total Lead enrichment or scraper handoff jobs queued.\n\
# TYPE benefactor_marketing_enrichment_jobs_total counter\n\
benefactor_marketing_enrichment_jobs_total {}\n\
# HELP benefactor_marketing_lead_imports_total Lead import requests accepted.\n\
# TYPE benefactor_marketing_lead_imports_total counter\n\
benefactor_marketing_lead_imports_total {}\n\
# HELP benefactor_marketing_auth_failures_total Authentication failures.\n\
# TYPE benefactor_marketing_auth_failures_total counter\n\
benefactor_marketing_auth_failures_total {}\n\
	# HELP benefactor_marketing_db_errors_total Database readiness or query failures.\n\
	# TYPE benefactor_marketing_db_errors_total counter\n\
	benefactor_marketing_db_errors_total {}\n\
	# HELP benefactor_marketing_redis_errors_total Redis readiness, cache, rate-limit, or stream failures.\n\
	# TYPE benefactor_marketing_redis_errors_total counter\n\
	benefactor_marketing_redis_errors_total {}\n\
	# HELP benefactor_marketing_cache_hits_total Redis dashboard cache hits.\n\
	# TYPE benefactor_marketing_cache_hits_total counter\n\
	benefactor_marketing_cache_hits_total {}\n\
	# HELP benefactor_marketing_cache_misses_total Redis dashboard cache misses.\n\
	# TYPE benefactor_marketing_cache_misses_total counter\n\
	benefactor_marketing_cache_misses_total {}\n\
	# HELP benefactor_marketing_cache_invalidations_total Redis cache keys invalidated after mutations.\n\
	# TYPE benefactor_marketing_cache_invalidations_total counter\n\
	benefactor_marketing_cache_invalidations_total {}\n\
	# HELP benefactor_marketing_rate_limit_rejections_total Write requests rejected by Redis-backed rate limits.\n\
	# TYPE benefactor_marketing_rate_limit_rejections_total counter\n\
	benefactor_marketing_rate_limit_rejections_total {}\n\
		# HELP benefactor_marketing_redis_jobs_published_total Marketing job handoff events published to Redis streams.\n\
		# TYPE benefactor_marketing_redis_jobs_published_total counter\n\
		benefactor_marketing_redis_jobs_published_total {}\n\
		# HELP benefactor_marketing_integration_sync_runs_total CRM or analytics sync runs recorded.\n\
		# TYPE benefactor_marketing_integration_sync_runs_total counter\n\
		benefactor_marketing_integration_sync_runs_total {}\n\
		# HELP benefactor_marketing_outreach_touchpoints_total Outreach touchpoints recorded.\n\
		# TYPE benefactor_marketing_outreach_touchpoints_total counter\n\
		benefactor_marketing_outreach_touchpoints_total {}\n\
		# HELP benefactor_marketing_research_briefs_total Prospect research briefs created.\n\
		# TYPE benefactor_marketing_research_briefs_total counter\n\
		benefactor_marketing_research_briefs_total {}\n\
		# HELP benefactor_marketing_conversion_events_total Conversion events recorded.\n\
		# TYPE benefactor_marketing_conversion_events_total counter\n\
		benefactor_marketing_conversion_events_total {}\n\
		# HELP benefactor_marketing_client_collaboration_events_total Portal, document, comment, and notification records accepted.\n\
		# TYPE benefactor_marketing_client_collaboration_events_total counter\n\
		benefactor_marketing_client_collaboration_events_total {}\n\
		# HELP benefactor_marketing_agency_finance_records_total Time, cost, commission, and budget records accepted.\n\
		# TYPE benefactor_marketing_agency_finance_records_total counter\n\
		benefactor_marketing_agency_finance_records_total {}\n\
		# HELP benefactor_marketing_call_insights_total Call insight records accepted.\n\
		# TYPE benefactor_marketing_call_insights_total counter\n\
		benefactor_marketing_call_insights_total {}\n",
        uptime,
        state.metrics.mutations_total.load(Ordering::Relaxed),
        state.metrics.enrichment_jobs_total.load(Ordering::Relaxed),
        state.metrics.lead_imports_total.load(Ordering::Relaxed),
        state.metrics.auth_failures_total.load(Ordering::Relaxed),
        state.metrics.db_errors_total.load(Ordering::Relaxed),
        state.metrics.redis_errors_total.load(Ordering::Relaxed),
        state.metrics.cache_hits_total.load(Ordering::Relaxed),
        state.metrics.cache_misses_total.load(Ordering::Relaxed),
        state
            .metrics
            .cache_invalidations_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .rate_limit_rejections_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .redis_jobs_published_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .integration_sync_runs_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .outreach_touchpoints_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .research_briefs_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .conversion_events_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .client_collaboration_events_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .agency_finance_records_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .call_insights_total
            .load(Ordering::Relaxed)
    );
    ([("content-type", "text/plain; version=0.0.4")], body)
}

pub(crate) async fn redis_status(State(state): State<AppState>, headers: HeaderMap) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    let configured = state.redis.is_some();
    let ready = if configured {
        redis_ping(&state).await
    } else {
        false
    };
    Ok(Json(json!({
        "configured": configured,
        "ready": ready,
        "requiredForReady": state.cfg.redis_required_for_ready,
        "cacheTtlSeconds": state.cfg.cache_ttl_seconds,
        "rateLimitPerMinute": state.cfg.rate_limit_per_minute,
        "jobStream": state.cfg.job_stream,
        "keyPrefix": "benefactor:marketing"
    })))
}

pub(crate) async fn api_docs_html() -> Html<&'static str> {
    Html(include_str!("../generated/api-docs.html"))
}

pub(crate) async fn api_docs_json() -> impl IntoResponse {
    (
        [("content-type", "application/json; charset=utf-8")],
        include_str!("../generated/api-docs.json"),
    )
}
