use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use uuid::Uuid;

use crate::error::AppResult;
use crate::locks::{AcquireRequest, Lease, LeaseRow, ReleaseRequest, RenewRequest};
use crate::state::AppState;

pub async fn acquire(
    State(state): State<AppState>,
    Path(tenant_id): Path<Uuid>,
    Json(req): Json<AcquireRequest>,
) -> AppResult<(StatusCode, Json<Lease>)> {
    let tenant = state.tenants.by_id(tenant_id).await?;
    let region = tenant.region()?;
    let lease = state.locks.acquire(tenant_id, region, None, req).await?;
    Ok((StatusCode::CREATED, Json(lease)))
}

pub async fn renew(
    State(state): State<AppState>,
    Path((tenant_id, resource)): Path<(Uuid, String)>,
    Json(req): Json<RenewRequest>,
) -> AppResult<Json<Lease>> {
    let tenant = state.tenants.by_id(tenant_id).await?;
    let region = tenant.region()?;
    let lease = state.locks.renew(tenant_id, region, None, &resource, req).await?;
    Ok(Json(lease))
}

pub async fn release(
    State(state): State<AppState>,
    Path((tenant_id, resource)): Path<(Uuid, String)>,
    Json(req): Json<ReleaseRequest>,
) -> AppResult<StatusCode> {
    let tenant = state.tenants.by_id(tenant_id).await?;
    let region = tenant.region()?;
    state.locks.release(tenant_id, region, None, &resource, req).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list(
    State(state): State<AppState>,
    Path(tenant_id): Path<Uuid>,
) -> AppResult<Json<Vec<LeaseRow>>> {
    let leases = state.locks.list(tenant_id).await?;
    Ok(Json(leases))
}
