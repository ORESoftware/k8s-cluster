//! Device registration and revocation. A device is the unit that holds a sync
//! token; revoking one invalidates its token without touching the account.

use crate::auth;
use crate::error::ApiError;
use serde::Serialize;
use sqlx::{Executor, PgPool, Postgres};
use time::OffsetDateTime;
use uuid::Uuid;

/// Per-account cap on live (non-revoked) devices. Bounds the token/attack-surface
/// growth from repeated logins (each login enrolls a device). A user hitting this
/// should revoke a stale device via `GET`/`POST /v1/devices`.
pub const MAX_DEVICES_PER_ACCOUNT: i64 = 25;

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

/// Insert a new device for an account and return `(device_id, raw_token)`.
/// The raw token is shown to the client exactly once.
pub async fn register<'e, E>(
    executor: E,
    account_id: Uuid,
    device_name: &str,
) -> Result<(Uuid, String), ApiError>
where
    E: Executor<'e, Database = Postgres>,
{
    let (token, token_hash) = auth::issue_token();
    let device_id: Uuid = sqlx::query_scalar(
        "INSERT INTO threefa.devices (account_id, device_name, sync_token_hash) \
         VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(account_id)
    .bind(device_name)
    .bind(&token_hash)
    .fetch_one(executor)
    .await?;
    Ok((device_id, token))
}

/// List every device (revoked and live) for an account, newest first, so the
/// owner can audit enrollments and pick a `device_id` to revoke.
pub async fn list(pool: &PgPool, account_id: Uuid) -> Result<Vec<DeviceInfo>, ApiError> {
    let rows = sqlx::query_as::<_, (Uuid, String, bool, OffsetDateTime, Option<OffsetDateTime>)>(
        "SELECT id, device_name, revoked, created_at, last_seen_at \
         FROM threefa.devices WHERE account_id = $1 ORDER BY created_at DESC",
    )
    .bind(account_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(device_id, device_name, revoked, created_at, last_seen_at)| DeviceInfo {
                device_id,
                device_name,
                revoked,
                created_at,
                last_seen_at,
            },
        )
        .collect())
}

/// Count an account's live (non-revoked) devices, to enforce the per-account cap
/// before enrolling another.
pub async fn live_count<'e, E>(executor: E, account_id: Uuid) -> Result<i64, ApiError>
where
    E: Executor<'e, Database = Postgres>,
{
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM threefa.devices WHERE account_id = $1 AND revoked = FALSE",
    )
    .bind(account_id)
    .fetch_one(executor)
    .await?;
    Ok(count)
}

/// Revoke a device (its sync token stops working immediately).
pub async fn revoke(pool: &PgPool, account_id: Uuid, device_id: Uuid) -> Result<(), ApiError> {
    let res =
        sqlx::query("UPDATE threefa.devices SET revoked = TRUE WHERE id = $1 AND account_id = $2")
            .bind(device_id)
            .bind(account_id)
            .execute(pool)
            .await?;
    if res.rows_affected() == 0 {
        return Err(ApiError::BadRequest);
    }
    Ok(())
}
