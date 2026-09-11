//! Read-only history endpoints — recent rows for each table, newest first,
//! consumed by the web dashboard and for operator inspection.

use crate::error::ApiError;
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::Json;
use sea_orm::{EntityTrait, PaginatorTrait, QueryOrder, QuerySelect};
use serde::Deserialize;
use serde_json::{json, Value};
use t2v_entity::{synthesis, transcription, translation, vapi_call};

const DEFAULT_LIMIT: u64 = 25;
const MAX_LIMIT: u64 = 200;

#[derive(Debug, Deserialize)]
pub struct HistoryParams {
    pub limit: Option<u64>,
}

fn clamp_limit(params: &HistoryParams) -> u64 {
    params.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

pub async fn transcriptions(
    State(state): State<AppState>,
    Query(params): Query<HistoryParams>,
) -> Result<Json<Value>, ApiError> {
    let limit = clamp_limit(&params);
    let rows = transcription::Entity::find()
        .order_by_desc(transcription::Column::CreatedAt)
        .limit(limit)
        .all(&state.db)
        .await?;
    let total = transcription::Entity::find().count(&state.db).await?;
    Ok(Json(json!({ "ok": true, "total": total, "items": rows })))
}

pub async fn translations(
    State(state): State<AppState>,
    Query(params): Query<HistoryParams>,
) -> Result<Json<Value>, ApiError> {
    let limit = clamp_limit(&params);
    let rows = translation::Entity::find()
        .order_by_desc(translation::Column::CreatedAt)
        .limit(limit)
        .all(&state.db)
        .await?;
    let total = translation::Entity::find().count(&state.db).await?;
    Ok(Json(json!({ "ok": true, "total": total, "items": rows })))
}

pub async fn syntheses(
    State(state): State<AppState>,
    Query(params): Query<HistoryParams>,
) -> Result<Json<Value>, ApiError> {
    let limit = clamp_limit(&params);
    let rows = synthesis::Entity::find()
        .order_by_desc(synthesis::Column::CreatedAt)
        .limit(limit)
        .all(&state.db)
        .await?;
    let total = synthesis::Entity::find().count(&state.db).await?;
    Ok(Json(json!({ "ok": true, "total": total, "items": rows })))
}

pub async fn vapi_calls(
    State(state): State<AppState>,
    Query(params): Query<HistoryParams>,
) -> Result<Json<Value>, ApiError> {
    let limit = clamp_limit(&params);
    let rows = vapi_call::Entity::find()
        .order_by_desc(vapi_call::Column::UpdatedAt)
        .limit(limit)
        .all(&state.db)
        .await?;
    let total = vapi_call::Entity::find().count(&state.db).await?;
    Ok(Json(json!({ "ok": true, "total": total, "items": rows })))
}
