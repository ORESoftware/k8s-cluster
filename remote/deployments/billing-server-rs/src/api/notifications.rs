use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::notifications::{CreateNotificationRule, NotificationDispatch, NotificationRule};
use crate::state::AppState;

pub async fn create_rule(
    State(state): State<AppState>,
    Path(tenant_id): Path<Uuid>,
    Json(input): Json<CreateNotificationRule>,
) -> AppResult<(StatusCode, Json<NotificationRule>)> {
    // The `credential_plaintext_b64` field is reserved for future
    // per-rule webhook signing / SendGrid keys, but the storage path
    // does not seal it yet (no `sealed_credential` column on
    // `notification_rules`). Accepting it here would silently drop a
    // secret on the floor — worse than rejecting it. Refuse loudly
    // until the schema + sealing path is in place.
    if input.credential_plaintext_b64.is_some() {
        return Err(AppError::BadRequest(
            "credential_plaintext_b64 is not yet supported on notification \
             rules; the storage layer would silently drop the secret. \
             Use a tenant-level provider connection for signing secrets \
             instead, or wait for the per-rule seal migration."
                .into(),
        ));
    }
    let tenant = state.tenants.by_id(tenant_id).await?;
    let region = tenant.region()?;
    let rule = state
        .notifications
        .create_rule(tenant_id, region, input)
        .await?;
    Ok((StatusCode::CREATED, Json(rule)))
}

pub async fn list_rules(
    State(state): State<AppState>,
    Path(tenant_id): Path<Uuid>,
) -> AppResult<Json<Vec<NotificationRule>>> {
    let rules = state.notifications.list_rules(tenant_id).await?;
    Ok(Json(rules))
}

#[derive(Deserialize)]
pub struct DispatchesQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
}
fn default_limit() -> i64 {
    100
}

pub async fn list_dispatches(
    State(state): State<AppState>,
    Path(tenant_id): Path<Uuid>,
    Query(q): Query<DispatchesQuery>,
) -> AppResult<Json<Vec<NotificationDispatch>>> {
    let dispatches = state
        .notifications
        .list_dispatches(tenant_id, q.limit.clamp(1, 500))
        .await?;
    Ok(Json(dispatches))
}
