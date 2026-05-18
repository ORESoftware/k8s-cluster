use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AppResult;
use crate::providers::connection::{CreateConnection, ProviderConnection};
use crate::scheduler::ScheduledJob;
use crate::state::AppState;

pub async fn list(
    State(state): State<AppState>,
    Path(tenant_id): Path<Uuid>,
) -> AppResult<Json<Vec<ProviderConnection>>> {
    let rows = state.connections.list_for_tenant(tenant_id).await?;
    Ok(Json(rows))
}

pub async fn create(
    State(state): State<AppState>,
    Path(tenant_id): Path<Uuid>,
    Json(input): Json<CreateConnection>,
) -> AppResult<Json<ProviderConnection>> {
    let tenant = state.tenants.by_id(tenant_id).await?;
    let region = tenant.region()?;
    let conn = state.connections.create(tenant_id, region, input).await?;
    Ok(Json(conn))
}

#[derive(Debug, Default, Deserialize)]
pub struct SyncNowRequest {
    #[serde(default)]
    pub cursor: Option<String>,
    /// "user", "webhook", "api", etc. Recorded on the lease + run for audit.
    #[serde(default)]
    pub trigger: Option<String>,
}

#[derive(Serialize)]
pub struct SyncNowResponse {
    /// The one-shot scheduled job that will execute the sync.
    pub job: ScheduledJob,
    /// Convenience hint for clients: poll `runs_url` to see the result.
    pub runs_url: String,
}

/// On-demand sync trigger. This is the *primary* sync mechanism — the
/// backstop poller (default 5x/day) only catches what this missed. Returns
/// quickly with a job handle the client can poll for results.
pub async fn sync_now(
    State(state): State<AppState>,
    Path((tenant_id, connection_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<SyncNowRequest>,
) -> AppResult<(StatusCode, Json<SyncNowResponse>)> {
    // Validate the connection exists for this tenant up front so we don't
    // queue garbage. The job handler also validates.
    let _conn = state.connections.get(tenant_id, connection_id).await?;
    let tenant = state.tenants.by_id(tenant_id).await?;
    let region = tenant.region()?;

    let payload = serde_json::json!({
        "connection_id": connection_id,
        "cursor": req.cursor,
        "trigger": req.trigger.unwrap_or_else(|| "on_demand".into()),
    });

    let job = state
        .scheduler
        .enqueue_one_shot(
            tenant_id,
            region,
            "sync.connection",
            format!("on-demand-conn-{}", connection_id),
            payload,
        )
        .await?;

    let runs_url = format!(
        "/v1/tenants/{tenant_id}/scheduled-jobs/{}/runs?limit=1",
        job.id
    );
    Ok((StatusCode::ACCEPTED, Json(SyncNowResponse { job, runs_url })))
}
