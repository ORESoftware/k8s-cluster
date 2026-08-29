use std::sync::atomic::Ordering;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use chrono::{DateTime, FixedOffset};
use dd_pg_defs_sea_orm::{
    benefactor_marketing_integration_sync_runs as integration_sync_runs,
    benefactor_marketing_integrations as integrations,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect, Set,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::redis_support::{publish_job_event, record_client_mutation, record_mutation};
use crate::shared::{
    ensure_client, ensure_integration, limit, non_negative, now_fixed, object_or_default,
    require_auth, require_write_access, ListQuery,
};
use crate::state::{AppResult, AppState};
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateIntegrationRequest {
    platform: String,
    status: Option<String>,
    auth_kind: Option<String>,
    external_account_id: Option<String>,
    sync_cursor: Option<String>,
    config: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateIntegrationSyncRunRequest {
    client_id: Option<Uuid>,
    sync_kind: Option<String>,
    direction: Option<String>,
    status: Option<String>,
    records_seen: Option<i32>,
    records_changed: Option<i32>,
    cursor_before: Option<String>,
    cursor_after: Option<String>,
    payload: Option<Value>,
    error_summary: Option<String>,
    started_at: Option<DateTime<FixedOffset>>,
    completed_at: Option<DateTime<FixedOffset>>,
}

pub(crate) async fn list_client_sync_runs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    ensure_client(&state.db, client_id).await?;
    let rows = integration_sync_runs::Entity::find()
        .filter(integration_sync_runs::Column::ClientId.eq(Some(client_id)))
        .order_by_desc(integration_sync_runs::Column::CreatedAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    Ok(Json(json!({ "syncRuns": rows })))
}

pub(crate) async fn create_integration(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Json(req): Json<CreateIntegrationRequest>,
) -> AppResult<(StatusCode, Json<integrations::Model>)> {
    require_write_access(&state, &headers, "integrations.create").await?;
    ensure_client(&state.db, client_id).await?;
    let model = integrations::ActiveModel {
        client_id: Set(Some(client_id)),
        platform: Set(req.platform),
        status: Set(req.status.unwrap_or_else(|| "connected".to_string())),
        auth_kind: Set(req.auth_kind.unwrap_or_else(|| "manual".to_string())),
        external_account_id: Set(req.external_account_id),
        sync_cursor: Set(req.sync_cursor),
        config: Set(object_or_default(req.config, "config")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

pub(crate) async fn create_integration_sync_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(integration_id): Path<Uuid>,
    Json(req): Json<CreateIntegrationSyncRunRequest>,
) -> AppResult<(StatusCode, Json<integration_sync_runs::Model>)> {
    require_write_access(&state, &headers, "integrations.sync-runs.create").await?;
    let integration = ensure_integration(&state.db, integration_id).await?;
    let client_id = req.client_id.or(integration.client_id);
    if let Some(client_id) = client_id {
        ensure_client(&state.db, client_id).await?;
    }
    let model = integration_sync_runs::ActiveModel {
        integration_id: Set(integration_id),
        client_id: Set(client_id),
        sync_kind: Set(req.sync_kind.unwrap_or_else(|| "incremental".to_string())),
        direction: Set(req.direction.unwrap_or_else(|| "import".to_string())),
        status: Set(req.status.unwrap_or_else(|| "queued".to_string())),
        records_seen: Set(non_negative(req.records_seen.unwrap_or(0), "recordsSeen")?),
        records_changed: Set(non_negative(
            req.records_changed.unwrap_or(0),
            "recordsChanged",
        )?),
        cursor_before: Set(req.cursor_before),
        cursor_after: Set(req.cursor_after),
        payload: Set(object_or_default(req.payload, "payload")?),
        error_summary: Set(req.error_summary),
        started_at: Set(req.started_at),
        completed_at: Set(req.completed_at),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;

    if model.status == "succeeded" {
        let mut active_integration: integrations::ActiveModel = integration.into();
        active_integration.last_sync_at = Set(Some(model.completed_at.unwrap_or_else(now_fixed)));
        if let Some(cursor_after) = model.cursor_after.clone() {
            active_integration.sync_cursor = Set(Some(cursor_after));
        }
        active_integration.updated_at = Set(now_fixed());
        active_integration.update(&state.db).await?;
    }

    state
        .metrics
        .integration_sync_runs_total
        .fetch_add(1, Ordering::Relaxed);
    publish_job_event(
        &state,
        "integration_sync_run_recorded",
        json!({
            "integrationId": integration_id,
            "clientId": client_id,
            "syncRunId": model.id,
            "syncKind": &model.sync_kind,
            "direction": &model.direction,
            "status": &model.status
        }),
    )
    .await;
    if let Some(client_id) = client_id {
        record_client_mutation(&state, client_id).await;
    } else {
        record_mutation(&state);
    }
    Ok((StatusCode::CREATED, Json(model)))
}
