use axum::Json;
use axum::extract::{Extension, Path, State};
use uuid::Uuid;

use crate::api::auth::{self, Principal};
use crate::error::AppResult;
use crate::state::AppState;
use crate::tenants::{CreateTenant, Tenant};

pub async fn create(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Json(input): Json<CreateTenant>,
) -> AppResult<Json<Tenant>> {
    auth::require_fresh_user_step_up(&state, &principal)?;
    let owner = principal.user()?.identity.subject.as_str();
    let tenant = state.tenants.create_owned(input, owner).await?;
    Ok(Json(tenant))
}

pub async fn get_by_id(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Tenant>> {
    let tenant = state.tenants.by_id(id).await?;
    Ok(Json(tenant))
}
