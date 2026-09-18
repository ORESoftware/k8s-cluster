use axum::Json;
use axum::extract::{Path, State};
use uuid::Uuid;

use crate::error::AppResult;
use crate::state::AppState;
use crate::tenants::{CreateTenant, Tenant};

pub async fn create(
    State(state): State<AppState>,
    Json(input): Json<CreateTenant>,
) -> AppResult<Json<Tenant>> {
    let t = state.tenants.create(input).await?;
    Ok(Json(t))
}

pub async fn get_by_id(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Tenant>> {
    let t = state.tenants.by_id(id).await?;
    Ok(Json(t))
}
