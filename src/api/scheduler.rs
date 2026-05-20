use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use crate::error::AppResult;
use crate::scheduler::{CreateScheduledJob, JobRun, ScheduledJob};
use crate::state::AppState;

pub async fn create(
    State(state): State<AppState>,
    Path(tenant_id): Path<Uuid>,
    Json(input): Json<CreateScheduledJob>,
) -> AppResult<(StatusCode, Json<ScheduledJob>)> {
    let tenant = state.tenants.by_id(tenant_id).await?;
    let region = tenant.region()?;
    let job = state.scheduler.create(Some(tenant_id), Some(region), input).await?;
    Ok((StatusCode::CREATED, Json(job)))
}

pub async fn list(
    State(state): State<AppState>,
    Path(tenant_id): Path<Uuid>,
) -> AppResult<Json<Vec<ScheduledJob>>> {
    let jobs = state.scheduler.list(Some(tenant_id)).await?;
    Ok(Json(jobs))
}

pub async fn get_one(
    State(state): State<AppState>,
    Path((_tenant_id, id)): Path<(Uuid, Uuid)>,
) -> AppResult<Json<ScheduledJob>> {
    let job = state.scheduler.get(id).await?;
    Ok(Json(job))
}

#[derive(Deserialize)]
pub struct RunsQuery {
    #[serde(default = "default_runs_limit")]
    pub limit: i64,
}
fn default_runs_limit() -> i64 { 50 }

pub async fn list_runs(
    State(state): State<AppState>,
    Path((_tenant_id, id)): Path<(Uuid, Uuid)>,
    Query(q): Query<RunsQuery>,
) -> AppResult<Json<Vec<JobRun>>> {
    let runs = state.scheduler.list_runs(id, q.limit.clamp(1, 500)).await?;
    Ok(Json(runs))
}

pub async fn run_now(
    State(state): State<AppState>,
    Path((_tenant_id, id)): Path<(Uuid, Uuid)>,
) -> AppResult<StatusCode> {
    state.scheduler.run_now(id).await?;
    Ok(StatusCode::ACCEPTED)
}

pub async fn enable(
    State(state): State<AppState>,
    Path((_tenant_id, id)): Path<(Uuid, Uuid)>,
) -> AppResult<StatusCode> {
    state.scheduler.set_enabled(id, true).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn disable(
    State(state): State<AppState>,
    Path((_tenant_id, id)): Path<(Uuid, Uuid)>,
) -> AppResult<StatusCode> {
    state.scheduler.set_enabled(id, false).await?;
    Ok(StatusCode::NO_CONTENT)
}
