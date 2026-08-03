//! Device registration and revocation. A device is the unit that holds a sync
//! token; revoking one invalidates its token without touching the account.

use crate::auth::{self, AuthedDevice};
use crate::entity::device;
use crate::error::ApiError;
use crate::json::JsonBody;
use crate::signal_api::map_store_error;
use crate::signal_store;
use crate::state::AppState;
use axum::extract::State;
use axum::Json;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, Set,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

/// Per-account cap on live (non-revoked) devices. Bounds the token/attack-surface
/// growth from repeated logins (each login enrolls a device). A user hitting this
/// should revoke a stale device via `GET`/`POST /v1/devices`.
pub const MAX_DEVICES_PER_ACCOUNT: u64 = 25;

/// A device as surfaced to its owner so they can recognize and revoke it. The
/// sync-token hash is deliberately never exposed.
#[derive(Debug, Serialize)]
pub struct DeviceInfo {
    pub device_id: Uuid,
    pub device_name: String,
    pub revoked: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_seen_at: Option<OffsetDateTime>,
}

pub(crate) async fn list_handler(
    State(state): State<AppState>,
    who: AuthedDevice,
) -> Result<Json<Vec<DeviceInfo>>, ApiError> {
    Ok(Json(list(state.database(), who.account_id).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevokeRequest {
    device_id: Uuid,
    #[serde(default)]
    expected_revision: Option<i64>,
}

// Credential first, body second — see `vault_blob::push_handler`.
pub(crate) async fn revoke_handler(
    State(state): State<AppState>,
    who: AuthedDevice,
    JsonBody(request): JsonBody<RevokeRequest>,
) -> Result<(), ApiError> {
    signal_store::revoke_device(
        state.database(),
        who.account_id,
        who.device_id,
        request.device_id,
        request.expected_revision,
    )
    .await
    .map(|_| ())
    .map_err(map_store_error)
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

/// List every device (revoked and live) for an account, newest first, so the
/// owner can audit enrollments and pick a `device_id` to revoke.
pub async fn list<C>(db: &C, account_id: Uuid) -> Result<Vec<DeviceInfo>, ApiError>
where
    C: ConnectionTrait,
{
    let models = device::Entity::find()
        .filter(device::Column::AccountId.eq(account_id))
        .order_by_desc(device::Column::CreatedAt)
        .all(db)
        .await?;
    Ok(models
        .into_iter()
        .map(|model| DeviceInfo {
            device_id: model.id,
            device_name: model.device_name,
            revoked: model.revoked,
            created_at: model.created_at,
            last_seen_at: model.last_seen_at,
        })
        .collect())
}

/// Count an account's live (non-revoked) devices, to enforce the per-account cap
/// before enrolling another.
pub async fn live_count<C>(db: &C, account_id: Uuid) -> Result<u64, ApiError>
where
    C: ConnectionTrait,
{
    Ok(device::Entity::find()
        .filter(device::Column::AccountId.eq(account_id))
        .filter(device::Column::Revoked.eq(false))
        .count(db)
        .await?)
}
