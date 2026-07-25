use std::{net::SocketAddr, sync::Arc, time::Instant};

use axum::{
    extract::DefaultBodyLimit,
    routing::{get, patch, post},
    Router,
};
use sea_orm::Database;
use tokio::sync::Mutex;
use tower_http::{limit::RequestBodyLimitLayer, trace::TraceLayer};
use tracing::{error, info};

mod analytics;
mod campaigns;
mod clients;
mod collaboration;
mod finance;
mod integrations;
mod leads;
mod outreach;
mod pipeline;
mod platform;
mod redis_support;
mod shared;
mod state;

use crate::analytics::*;
use crate::campaigns::*;
use crate::clients::*;
use crate::collaboration::*;
use crate::finance::*;
use crate::integrations::*;
use crate::leads::*;
use crate::outreach::*;
use crate::pipeline::*;
use crate::platform::*;
use crate::state::{AppState, Config, Metrics, MAX_HTTP_BODY_BYTES, SERVICE_NAME};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _otel = dd_telemetry::init(SERVICE_NAME);
    let cfg = Config::from_env()?;
    let addr: SocketAddr = format!("{}:{}", cfg.host, cfg.port).parse()?;
    let db = Database::connect(&cfg.database_url).await?;
    let redis = cfg
        .redis_url
        .as_deref()
        .map(redis::Client::open)
        .transpose()?;
    if redis.is_some() {
        info!("redis integration enabled for benefactor marketing runtime");
    }
    let state = AppState {
        cfg: Arc::new(cfg),
        db,
        redis,
        redis_connection: Arc::new(Mutex::new(None)),
        metrics: Arc::new(Metrics::default()),
        started_at: Instant::now(),
    };

    let app = build_router(state.clone());
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(%addr, "benefactor marketing backend listening");
    axum::serve(listener, app.layer(dd_telemetry::http_trace_layer()))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    info!("benefactor marketing backend shut down cleanly");
    Ok(())
}

fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(descriptor))
        .route("/descriptor", get(descriptor))
        .route("/capabilities", get(capabilities))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
        .route("/runtime/redis", get(redis_status))
        .route(
            "/service-packages",
            get(list_service_packages).post(create_service_package),
        )
        .route("/clients", get(list_clients).post(create_client))
        .route("/clients/{client_id}/overview", get(client_dashboard))
        .route("/clients/{client_id}/dashboard", get(client_dashboard))
        .route(
            "/clients/{client_id}/lead-intelligence",
            get(client_lead_intelligence),
        )
        .route(
            "/clients/{client_id}/revenue-attribution",
            get(client_revenue_attribution),
        )
        .route(
            "/clients/{client_id}/operations",
            get(client_operations_summary),
        )
        .route(
            "/clients/{client_id}/profitability",
            get(client_profitability_summary),
        )
        .route(
            "/clients/{client_id}/team-allocations",
            get(list_client_team_allocations).post(create_team_allocation),
        )
        .route(
            "/clients/{client_id}/portal/members",
            get(list_client_portal_members).post(create_portal_member),
        )
        .route(
            "/clients/{client_id}/documents",
            get(list_client_documents).post(create_shared_document),
        )
        .route(
            "/clients/{client_id}/comments",
            get(list_client_comments).post(create_comment),
        )
        .route(
            "/clients/{client_id}/notifications",
            get(list_client_notifications).post(create_notification),
        )
        .route(
            "/clients/{client_id}/time-entries",
            get(list_client_time_entries).post(create_time_entry),
        )
        .route(
            "/clients/{client_id}/vendor-costs",
            get(list_client_vendor_costs).post(create_vendor_cost),
        )
        .route(
            "/clients/{client_id}/commissions",
            get(list_client_commissions).post(create_commission_entry),
        )
        .route(
            "/clients/{client_id}/budget-forecasts",
            get(list_client_budget_forecasts).post(create_budget_forecast),
        )
        .route(
            "/clients/{client_id}/call-insights",
            get(list_client_call_insights).post(create_client_call_insight),
        )
        .route("/clients/{client_id}/sync-runs", get(list_client_sync_runs))
        .route(
            "/clients/{client_id}/outreach",
            get(client_outreach_summary),
        )
        .route(
            "/clients/{client_id}/outreach/sequences",
            get(list_client_outreach_sequences),
        )
        .route(
            "/clients/{client_id}/research/briefs",
            get(list_client_research_briefs),
        )
        .route(
            "/clients/{client_id}/conversion-events",
            get(list_client_conversion_events),
        )
        .route("/clients/{client_id}/contacts", post(create_contact))
        .route("/clients/{client_id}/contracts", post(create_contract))
        .route("/clients/{client_id}/invoices", post(create_invoice))
        .route(
            "/clients/{client_id}/integrations",
            post(create_integration),
        )
        .route("/clients/{client_id}/leads", get(list_client_leads))
        .route("/clients/{client_id}/campaigns", get(list_client_campaigns))
        .route(
            "/integrations/{integration_id}/sync-runs",
            post(create_integration_sync_run),
        )
        .route("/leads/import", post(import_leads))
        .route(
            "/leads/{lead_id}/enrichment-jobs",
            post(queue_enrichment_job),
        )
        .route("/leads/{lead_id}/score", post(score_lead))
        .route("/campaigns", post(create_campaign))
        .route(
            "/campaigns/{campaign_id}/channels",
            post(create_campaign_channel),
        )
        .route(
            "/campaigns/{campaign_id}/experiments",
            post(create_campaign_experiment),
        )
        .route("/outreach/sequences", post(create_outreach_sequence))
        .route(
            "/outreach/sequences/{sequence_id}/steps",
            post(create_outreach_step),
        )
        .route("/outreach/enrollments", post(create_outreach_enrollment))
        .route("/outreach/touchpoints", post(record_outreach_touchpoint))
        .route("/automation/workflows", post(create_automation_workflow))
        .route("/automation/events", post(record_automation_event))
        .route("/reports/snapshots", post(create_report_snapshot))
        .route("/attribution/events", post(record_attribution_event))
        .route("/opportunities", post(create_opportunity))
        .route("/content/assets", post(create_content_asset))
        .route("/research/briefs", post(create_research_brief))
        .route("/conversion/events", post(record_conversion_event))
        .route("/projects/tasks", post(create_project_task))
        .route("/approvals", post(create_approval))
        .route("/approvals/{approval_id}/decision", patch(decide_approval))
        .route("/tickets", post(create_ticket))
        .route("/meetings", post(create_meeting))
        .route(
            "/meetings/{meeting_id}/call-insights",
            post(create_meeting_call_insight),
        )
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .layer(RequestBodyLimitLayer::new(MAX_HTTP_BODY_BYTES))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            error!(error = %err, "failed to install ctrl_c handler");
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(err) => {
                error!(error = %err, "failed to install SIGTERM handler");
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => info!("ctrl_c received, shutting down"),
        _ = terminate => info!("SIGTERM received, shutting down"),
    }
}
