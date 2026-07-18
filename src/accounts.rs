//! Account registration and login HTTP handlers.

use crate::entity::account;
use crate::error::ApiError;
use crate::state::AppState;
use crate::{auth, devices};
use axum::extract::State;
use axum::Json;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set, SqlErr, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const MAX_USERNAME_LEN: usize = 256;
const MAX_DEVICE_NAME_LEN: usize = 200;
const MAX_PASSWORD_LEN: usize = 1024;

#[derive(Deserialize)]
pub(crate) struct CredsRequest {
    username: String,
    /// The account password. Used only to derive the Argon2id verifier (and, in
    /// the OPAQUE upgrade, never sent at all). Never stored in plaintext.
    password: String,
    device_name: String,
}

#[derive(Serialize)]
pub(crate) struct TokenResponse {
    account_id: Uuid,
    device_id: Uuid,
    /// Bearer token — shown once. Lost tokens require re-login.
    sync_token: String,
}

pub(crate) async fn register(
    State(state): State<AppState>,
    Json(request): Json<CredsRequest>,
) -> Result<Json<TokenResponse>, ApiError> {
    if !credentials_are_valid(&request, true) {
        return Err(ApiError::BadRequest);
    }
    let auth_secret = hash_password_bounded(&state, request.password).await?;

    let transaction = state.database().begin().await?;
    let account_id = Uuid::new_v4();
    let inserted = account::ActiveModel {
        id: Set(account_id),
        username: Set(request.username),
        auth_secret: Set(auth_secret),
        ..Default::default()
    }
    .insert(&transaction)
    .await;
    if let Err(error) = inserted {
        if matches!(error.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) {
            return Err(ApiError::Conflict);
        }
        return Err(error.into());
    }

    let (device_id, token) =
        devices::register(&transaction, account_id, &request.device_name).await?;
    transaction.commit().await?;
    Ok(Json(TokenResponse {
        account_id,
        device_id,
        sync_token: token,
    }))
}

pub(crate) async fn login(
    State(state): State<AppState>,
    Json(request): Json<CredsRequest>,
) -> Result<Json<TokenResponse>, ApiError> {
    if !credentials_are_valid(&request, false) {
        return Err(ApiError::BadRequest);
    }
    let row = account::Entity::find()
        .filter(account::Column::Username.eq(&request.username))
        .one(state.database())
        .await?;

    // Always run exactly one Argon2 verify to avoid a username-enumeration
    // timing oracle, whether or not the account exists.
    let (account_id, secret) = match row {
        Some(account) => (account.id, Some(account.auth_secret)),
        None => (Uuid::nil(), None),
    };
    let ok = verify_password_bounded(&state, request.password, secret).await?;
    if !ok || account_id.is_nil() {
        return Err(ApiError::Unauthorized);
    }

    let (device_id, token) =
        devices::register(state.database(), account_id, &request.device_name).await?;
    Ok(Json(TokenResponse {
        account_id,
        device_id,
        sync_token: token,
    }))
}

fn credentials_are_valid(request: &CredsRequest, require_minimum_password: bool) -> bool {
    let password_len_valid = if require_minimum_password {
        (8..=MAX_PASSWORD_LEN).contains(&request.password.len())
    } else {
        request.password.len() <= MAX_PASSWORD_LEN
    };
    !request.username.trim().is_empty()
        && request.username == request.username.trim()
        && request.username.len() <= MAX_USERNAME_LEN
        && !request.device_name.trim().is_empty()
        && request.device_name == request.device_name.trim()
        && request.device_name.len() <= MAX_DEVICE_NAME_LEN
        && password_len_valid
}

async fn hash_password_bounded(state: &AppState, password: String) -> Result<String, ApiError> {
    let permit = state
        .auth_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::TooManyRequests)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        auth::hash_password(password.as_bytes())
    })
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "Argon2 registration worker failed");
        ApiError::Internal
    })?
}

async fn verify_password_bounded(
    state: &AppState,
    password: String,
    secret: Option<String>,
) -> Result<bool, ApiError> {
    let permit = state
        .auth_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::TooManyRequests)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let secret = secret.unwrap_or_else(|| dummy_phc().to_owned());
        auth::verify_password(password.as_bytes(), &secret)
    })
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "Argon2 login worker failed");
        ApiError::Internal
    })
}

/// A valid Argon2id PHC string used when a username is unknown.
fn dummy_phc() -> &'static str {
    use std::sync::OnceLock;
    static DUMMY: OnceLock<String> = OnceLock::new();
    DUMMY.get_or_init(|| auth::hash_password(b"3fa-dummy-account-not-real").expect("dummy hash"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(username: &str, password: &str, device_name: &str) -> CredsRequest {
        CredsRequest {
            username: username.to_owned(),
            password: password.to_owned(),
            device_name: device_name.to_owned(),
        }
    }

    #[test]
    fn registration_enforces_password_and_contract_boundaries() {
        assert!(credentials_are_valid(
            &request("alice", "12345678", "desktop"),
            true
        ));
        assert!(!credentials_are_valid(
            &request("alice", "1234567", "desktop"),
            true
        ));
        assert!(!credentials_are_valid(
            &request("alice", &"x".repeat(MAX_PASSWORD_LEN + 1), "desktop"),
            true
        ));
    }

    #[test]
    fn identifiers_must_be_trimmed_nonempty_and_bounded() {
        for invalid in [
            request(" alice", "12345678", "desktop"),
            request("", "12345678", "desktop"),
            request("alice", "12345678", " desktop"),
            request("alice", "12345678", &"d".repeat(MAX_DEVICE_NAME_LEN + 1)),
            request(&"u".repeat(MAX_USERNAME_LEN + 1), "12345678", "desktop"),
        ] {
            assert!(!credentials_are_valid(&invalid, true));
        }
    }

    #[test]
    fn login_keeps_account_independent_password_validation() {
        // Login accepts short values so every syntactically bounded attempt can
        // run the same Argon2 path, whether or not the username exists.
        assert!(credentials_are_valid(
            &request("unknown", "", "desktop"),
            false
        ));
        assert!(!credentials_are_valid(
            &request("unknown", &"x".repeat(MAX_PASSWORD_LEN + 1), "desktop"),
            false
        ));
    }
}
