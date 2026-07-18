//! Supabase-authenticated account mapping and device enrollment.

use crate::accounts::{self, TokenResponse};
use crate::entity::account;
use crate::error::ApiError;
use crate::state::AppState;
use crate::{auth, devices};
use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QuerySelect, Set, TransactionTrait,
};
use serde::Deserialize;
use uuid::Uuid;

#[derive(Deserialize)]
pub(crate) struct SupabaseEnrollRequest {
    device_name: String,
}

/// Verify a Supabase access JWT, map its subject onto a local account, and
/// issue the enrolled device a long-lived sync token.
pub(crate) async fn enroll(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SupabaseEnrollRequest>,
) -> Result<Json<TokenResponse>, ApiError> {
    if !accounts::device_name_is_valid(&request.device_name) {
        return Err(ApiError::BadRequest);
    }
    let verifier = state.supabase().ok_or(ApiError::NotImplemented)?;
    let identity = verifier.verify(auth::bearer(&headers)?).await?;

    let transaction = state.database().begin().await?;
    // Target-less DO NOTHING handles the migration's partial unique index
    // without relying on Postgres to infer its predicate from a column target.
    account::Entity::insert(account::ActiveModel {
        id: Set(Uuid::new_v4()),
        username: Set(None),
        auth_secret: Set(None),
        supabase_user_id: Set(Some(identity.user_id)),
        email: Set(identity.email.clone()),
        ..Default::default()
    })
    .on_conflict(OnConflict::new().do_nothing().to_owned())
    .exec_without_returning(&transaction)
    .await?;

    // Lock the identity row while enforcing the device cap and enrolling, so
    // concurrent logins cannot race past the per-account limit.
    let account = account::Entity::find()
        .filter(account::Column::SupabaseUserId.eq(identity.user_id))
        .lock_exclusive()
        .one(&transaction)
        .await?
        .ok_or(ApiError::Internal)?;
    let account_id = account.id;
    if account.email != identity.email {
        let mut active: account::ActiveModel = account.into();
        active.email = Set(identity.email);
        active.update(&transaction).await?;
    }

    if devices::live_count(&transaction, account_id).await? >= devices::MAX_DEVICES_PER_ACCOUNT {
        return Err(ApiError::TooManyRequests);
    }
    let (device_id, sync_token) =
        devices::register(&transaction, account_id, &request.device_name).await?;
    transaction.commit().await?;

    Ok(Json(TokenResponse {
        account_id,
        device_id,
        sync_token,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enrollment_reuses_the_shared_device_name_contract() {
        assert!(accounts::device_name_is_valid("primary desktop"));
        for invalid in ["", " laptop", "laptop "] {
            assert!(!accounts::device_name_is_valid(invalid));
        }
        assert!(!accounts::device_name_is_valid(
            &"d".repeat(accounts::MAX_DEVICE_NAME_LEN + 1)
        ));
    }
}
