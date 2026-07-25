//! Human-identity mapping and device enrollment.
//!
//! `/v1/auth/shared` accepts a shared-auth access token. The legacy-named
//! `/v1/auth/supabase` route remains as a compatibility bridge, but delegates
//! provider-token verification and exchange to shared-auth.

use crate::accounts::{self, TokenResponse};
use crate::entity::account;
use crate::error::ApiError;
use crate::json::JsonBody;
use crate::shared_auth::{SharedAuthError, VerifiedIdentity};
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
pub(crate) struct EnrollRequest {
    device_name: String,
}

#[tracing::instrument(
    name = "auth.enroll.shared",
    skip_all,
    fields(auth.authority = "shared-auth", auth.credential = "access_token")
)]
pub(crate) async fn enroll_shared(
    State(state): State<AppState>,
    headers: HeaderMap,
    JsonBody(request): JsonBody<EnrollRequest>,
) -> Result<Json<TokenResponse>, ApiError> {
    let verifier = state.shared_auth().ok_or(ApiError::NotImplemented)?;
    let identity = verifier
        .introspect_access_token(auth::bearer(&headers)?)
        .await
        .map_err(map_auth_error)?;
    enroll_identity(&state, request, identity).await
}

#[tracing::instrument(
    name = "auth.enroll.provider",
    skip_all,
    fields(auth.authority = "shared-auth", auth.provider = "supabase")
)]
pub(crate) async fn enroll_provider(
    State(state): State<AppState>,
    headers: HeaderMap,
    JsonBody(request): JsonBody<EnrollRequest>,
) -> Result<Json<TokenResponse>, ApiError> {
    let verifier = state.shared_auth().ok_or(ApiError::NotImplemented)?;
    let identity = verifier
        .exchange_provider_token(auth::bearer(&headers)?)
        .await
        .map_err(map_auth_error)?;
    enroll_identity(&state, request, identity).await
}

fn map_auth_error(error: SharedAuthError) -> ApiError {
    match error {
        SharedAuthError::Unauthenticated => ApiError::Unauthorized,
        SharedAuthError::Unavailable => ApiError::Unavailable,
    }
}

async fn enroll_identity(
    state: &AppState,
    request: EnrollRequest,
    identity: VerifiedIdentity,
) -> Result<Json<TokenResponse>, ApiError> {
    if !accounts::device_name_is_valid(&request.device_name) {
        return Err(ApiError::BadRequest);
    }

    let transaction = state.database().begin().await?;
    // Target-less DO NOTHING handles both partial identity indexes without
    // relying on Postgres to infer either predicate from a column target.
    account::Entity::insert(account::ActiveModel {
        id: Set(Uuid::new_v4()),
        username: Set(None),
        auth_secret: Set(None),
        shared_auth_user_id: Set(Some(identity.shared_auth_user_id)),
        supabase_user_id: Set(identity.supabase_user_id),
        email: Set(identity.email.clone()),
        ..Default::default()
    })
    .on_conflict(OnConflict::new().do_nothing().to_owned())
    .exec_without_returning(&transaction)
    .await?;

    // Lock the stable shared-auth identity while enforcing the device cap, so
    // concurrent enrollments cannot race past the per-account limit.
    let mut account = account::Entity::find()
        .filter(account::Column::SharedAuthUserId.eq(identity.shared_auth_user_id))
        .lock_exclusive()
        .one(&transaction)
        .await?;
    // Upgrade an existing Supabase-linked row in place the first time it is
    // seen through shared-auth. This preserves every existing vault/account id.
    if account.is_none() {
        if let Some(supabase_user_id) = identity.supabase_user_id {
            account = account::Entity::find()
                .filter(account::Column::SupabaseUserId.eq(supabase_user_id))
                .lock_exclusive()
                .one(&transaction)
                .await?;
        }
    }
    let account = account.ok_or(ApiError::Internal)?;
    if account
        .shared_auth_user_id
        .is_some_and(|id| id != identity.shared_auth_user_id)
        || account
            .supabase_user_id
            .zip(identity.supabase_user_id)
            .is_some_and(|(stored, verified)| stored != verified)
    {
        tracing::error!(
            auth.outcome = "identity_mapping_conflict",
            "verified identity conflicts with an existing account mapping"
        );
        return Err(ApiError::Internal);
    }
    let account_id = account.id;
    if account.shared_auth_user_id != Some(identity.shared_auth_user_id)
        || (identity.supabase_user_id.is_some()
            && account.supabase_user_id != identity.supabase_user_id)
        || (identity.email.is_some() && account.email != identity.email)
    {
        let mut active: account::ActiveModel = account.into();
        active.shared_auth_user_id = Set(Some(identity.shared_auth_user_id));
        if identity.supabase_user_id.is_some() {
            active.supabase_user_id = Set(identity.supabase_user_id);
        }
        if identity.email.is_some() {
            active.email = Set(identity.email);
        }
        active.update(&transaction).await?;
    }

    if devices::live_count(&transaction, account_id).await? >= devices::MAX_DEVICES_PER_ACCOUNT {
        return Err(ApiError::TooManyRequests);
    }
    let (device_id, sync_token) =
        devices::register(&transaction, account_id, &request.device_name).await?;
    transaction.commit().await?;

    tracing::info!(
        auth.authority = "shared-auth",
        auth.outcome = "device_enrolled",
        "verified identity enrolled a sync device"
    );

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

    #[test]
    fn shared_auth_failure_mapping_preserves_degraded_state() {
        assert!(matches!(
            map_auth_error(SharedAuthError::Unauthenticated),
            ApiError::Unauthorized
        ));
        assert!(matches!(
            map_auth_error(SharedAuthError::Unavailable),
            ApiError::Unavailable
        ));
    }
}
