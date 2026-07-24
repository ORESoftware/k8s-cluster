use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use chrono::{DateTime, FixedOffset};
use dd_pg_defs_sea_orm::{
    benefactor_marketing_campaign_channels as campaign_channels,
    benefactor_marketing_campaign_experiments as campaign_experiments,
    benefactor_marketing_campaigns as campaigns,
    benefactor_marketing_content_assets as content_assets,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect, Set,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::redis_support::record_client_mutation;
use crate::shared::{
    array_or_default, ensure_campaign, ensure_client, limit, object_or_default, require_auth,
    require_write_access, ListQuery,
};
use crate::state::{AppResult, AppState};
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateCampaignRequest {
    client_id: Uuid,
    status: Option<String>,
    campaign_kind: Option<String>,
    name: String,
    objective: Option<String>,
    budget_cents: Option<i32>,
    starts_on: Option<String>,
    ends_on: Option<String>,
    target_segments: Option<Value>,
    kpis: Option<Value>,
    meta_data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateCampaignChannelRequest {
    channel: String,
    status: Option<String>,
    external_campaign_id: Option<String>,
    strategy: Option<Value>,
    schedule: Option<Value>,
    metrics_snapshot: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateCampaignExperimentRequest {
    status: Option<String>,
    experiment_kind: String,
    hypothesis: Option<String>,
    variants: Option<Value>,
    winning_variant: Option<String>,
    result_summary: Option<Value>,
    started_at: Option<DateTime<FixedOffset>>,
    ended_at: Option<DateTime<FixedOffset>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateContentAssetRequest {
    client_id: Uuid,
    campaign_id: Option<Uuid>,
    status: Option<String>,
    asset_kind: String,
    title: String,
    channel: Option<String>,
    body: Option<String>,
    asset_uri: Option<String>,
    seo_keywords: Option<Value>,
    approval_status: Option<String>,
    publish_at: Option<DateTime<FixedOffset>>,
    meta_data: Option<Value>,
}

pub(crate) async fn create_campaign(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateCampaignRequest>,
) -> AppResult<(StatusCode, Json<campaigns::Model>)> {
    require_write_access(&state, &headers, "campaigns.create").await?;
    let client_id = req.client_id;
    ensure_client(&state.db, client_id).await?;
    let model = campaigns::ActiveModel {
        client_id: Set(client_id),
        status: Set(req.status.unwrap_or_else(|| "draft".to_string())),
        campaign_kind: Set(req
            .campaign_kind
            .unwrap_or_else(|| "multi_channel".to_string())),
        name: Set(req.name),
        objective: Set(req.objective),
        budget_cents: Set(req.budget_cents.unwrap_or(0)),
        starts_on: Set(req.starts_on),
        ends_on: Set(req.ends_on),
        target_segments: Set(array_or_default(req.target_segments, "targetSegments")?),
        kpis: Set(object_or_default(req.kpis, "kpis")?),
        meta_data: Set(object_or_default(req.meta_data, "metaData")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

pub(crate) async fn list_client_campaigns(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    ensure_client(&state.db, client_id).await?;
    let rows = campaigns::Entity::find()
        .filter(campaigns::Column::ClientId.eq(client_id))
        .order_by_desc(campaigns::Column::UpdatedAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    Ok(Json(json!({ "campaigns": rows })))
}

pub(crate) async fn create_campaign_channel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(campaign_id): Path<Uuid>,
    Json(req): Json<CreateCampaignChannelRequest>,
) -> AppResult<(StatusCode, Json<campaign_channels::Model>)> {
    require_write_access(&state, &headers, "campaigns.channels.create").await?;
    let campaign = ensure_campaign(&state.db, campaign_id).await?;
    let model = campaign_channels::ActiveModel {
        campaign_id: Set(campaign_id),
        channel: Set(req.channel),
        status: Set(req.status.unwrap_or_else(|| "draft".to_string())),
        external_campaign_id: Set(req.external_campaign_id),
        strategy: Set(object_or_default(req.strategy, "strategy")?),
        schedule: Set(object_or_default(req.schedule, "schedule")?),
        metrics_snapshot: Set(object_or_default(req.metrics_snapshot, "metricsSnapshot")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_client_mutation(&state, campaign.client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

pub(crate) async fn create_campaign_experiment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(campaign_id): Path<Uuid>,
    Json(req): Json<CreateCampaignExperimentRequest>,
) -> AppResult<(StatusCode, Json<campaign_experiments::Model>)> {
    require_write_access(&state, &headers, "campaigns.experiments.create").await?;
    let campaign = ensure_campaign(&state.db, campaign_id).await?;
    let model = campaign_experiments::ActiveModel {
        campaign_id: Set(campaign_id),
        status: Set(req.status.unwrap_or_else(|| "draft".to_string())),
        experiment_kind: Set(req.experiment_kind),
        hypothesis: Set(req.hypothesis),
        variants: Set(array_or_default(req.variants, "variants")?),
        winning_variant: Set(req.winning_variant),
        result_summary: Set(object_or_default(req.result_summary, "resultSummary")?),
        started_at: Set(req.started_at),
        ended_at: Set(req.ended_at),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_client_mutation(&state, campaign.client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

pub(crate) async fn create_content_asset(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateContentAssetRequest>,
) -> AppResult<(StatusCode, Json<content_assets::Model>)> {
    require_write_access(&state, &headers, "content.assets.create").await?;
    let client_id = req.client_id;
    ensure_client(&state.db, client_id).await?;
    let model = content_assets::ActiveModel {
        client_id: Set(client_id),
        campaign_id: Set(req.campaign_id),
        status: Set(req.status.unwrap_or_else(|| "draft".to_string())),
        asset_kind: Set(req.asset_kind),
        title: Set(req.title),
        channel: Set(req.channel),
        body: Set(req.body),
        asset_uri: Set(req.asset_uri),
        seo_keywords: Set(array_or_default(req.seo_keywords, "seoKeywords")?),
        approval_status: Set(req.approval_status.unwrap_or_else(|| "pending".to_string())),
        publish_at: Set(req.publish_at),
        meta_data: Set(object_or_default(req.meta_data, "metaData")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}
