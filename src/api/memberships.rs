use axum::Json;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use uuid::Uuid;

use crate::api::auth::{self, Principal};
use crate::error::AppResult;
use crate::memberships::{TenantGrant, UpsertMembership};
use crate::state::AppState;

pub async fn list(
    State(state): State<AppState>,
    Path(tenant_id): Path<Uuid>,
    Extension(principal): Extension<Principal>,
) -> AppResult<Json<Vec<TenantGrant>>> {
    auth::require_embedded_tenant_admin(&state, &principal, tenant_id).await?;
    Ok(Json(state.memberships.list(tenant_id).await?))
}

pub async fn upsert(
    State(state): State<AppState>,
    Path((tenant_id, shared_user_id)): Path<(Uuid, String)>,
    Extension(principal): Extension<Principal>,
    Json(input): Json<UpsertMembership>,
) -> AppResult<Json<TenantGrant>> {
    auth::require_embedded_tenant_admin(&state, &principal, tenant_id).await?;
    let actor = principal.user()?.identity.subject.as_str();
    Ok(Json(
        state
            .memberships
            .upsert(tenant_id, &shared_user_id, input, actor)
            .await?,
    ))
}

pub async fn revoke(
    State(state): State<AppState>,
    Path((tenant_id, shared_user_id)): Path<(Uuid, String)>,
    Extension(principal): Extension<Principal>,
) -> AppResult<StatusCode> {
    auth::require_embedded_tenant_admin(&state, &principal, tenant_id).await?;
    let actor = principal.user()?.identity.subject.as_str();
    state
        .memberships
        .revoke(tenant_id, &shared_user_id, actor)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
