//! Device registration and revocation. A device is the unit that holds a sync
//! token; revoking one invalidates its token without touching the account.

use crate::auth;
use crate::error::ApiError;
use sqlx::PgPool;
use uuid::Uuid;

/// Insert a new device for an account and return `(device_id, raw_token)`.
/// The raw token is shown to the client exactly once.
pub async fn register(
    pool: &PgPool,
    account_id: Uuid,
    device_name: &str,
) -> Result<(Uuid, String), ApiError> {
    let (token, token_hash) = auth::issue_token();
    let device_id: Uuid = sqlx::query_scalar(
        "INSERT INTO devices (account_id, device_name, sync_token_hash) \
         VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(account_id)
    .bind(device_name)
    .bind(&token_hash)
    .fetch_one(pool)
    .await?;
    Ok((device_id, token))
}

/// Revoke a device (its sync token stops working immediately).
pub async fn revoke(pool: &PgPool, account_id: Uuid, device_id: Uuid) -> Result<(), ApiError> {
    let res = sqlx::query("UPDATE devices SET revoked = TRUE WHERE id = $1 AND account_id = $2")
        .bind(device_id)
        .bind(account_id)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(ApiError::BadRequest);
    }
    Ok(())
}
