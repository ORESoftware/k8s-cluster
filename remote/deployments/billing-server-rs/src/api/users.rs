use axum::extract::{Path, State};
use axum::Json;
use uuid::Uuid;

use crate::error::AppResult;
use crate::state::AppState;
use crate::users::{CreateUser, User};

pub async fn upsert(
    State(state): State<AppState>,
    Path(tenant_id): Path<Uuid>,
    Json(input): Json<CreateUser>,
) -> AppResult<Json<User>> {
    let tenant = state.tenants.by_id(tenant_id).await?;
    let region = tenant.region()?;
    let user = state.users.upsert(tenant_id, region, input).await?;
    Ok(Json(user))
}

pub async fn get_by_email(
    State(state): State<AppState>,
    Path((tenant_id, email)): Path<(Uuid, String)>,
) -> AppResult<Json<User>> {
    let user = state.users.by_email(tenant_id, &email).await?;
    Ok(Json(user))
}
