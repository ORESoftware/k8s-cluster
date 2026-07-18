//! Device registration and revocation. A device is the unit that holds a sync
//! token; revoking one invalidates its token without touching the account.

use crate::auth;
use crate::entity::device;
use crate::error::ApiError;
use crate::state::AppState;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter,
    Set,
};
use serde::Deserialize;
use uuid::Uuid;

#[derive(Deserialize)]
pub(crate) struct RevokeRequest {
    device_id: Uuid,
}

pub(crate) async fn revoke_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<RevokeRequest>,
) -> Result<(), ApiError> {
    let who = auth::authenticate(state.database(), &headers).await?;
    revoke(state.database(), who.account_id, request.device_id).await
}

/// Insert a new device for an account and return `(device_id, raw_token)`.
/// The raw token is shown to the client exactly once.
pub async fn register<C>(
    db: &C,
    account_id: Uuid,
    device_name: &str,
) -> Result<(Uuid, String), ApiError>
where
    C: ConnectionTrait,
{
    let (token, token_hash) = auth::issue_token();
    let model = device::ActiveModel {
        id: Set(Uuid::new_v4()),
        account_id: Set(account_id),
        device_name: Set(device_name.to_owned()),
        sync_token_hash: Set(token_hash),
        ..Default::default()
    }
    .insert(db)
    .await?;
    Ok((model.id, token))
}

/// Revoke a device (its sync token stops working immediately).
pub async fn revoke(
    db: &DatabaseConnection,
    account_id: Uuid,
    device_id: Uuid,
) -> Result<(), ApiError> {
    let result = device::Entity::update_many()
        .col_expr(device::Column::Revoked, Expr::value(true))
        .filter(device::Column::Id.eq(device_id))
        .filter(device::Column::AccountId.eq(account_id))
        .exec(db)
        .await?;
    if result.rows_affected == 0 {
        return Err(ApiError::BadRequest);
    }
    Ok(())
}
