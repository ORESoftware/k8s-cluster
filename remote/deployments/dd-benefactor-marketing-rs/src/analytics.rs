use std::sync::atomic::Ordering;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use chrono::{DateTime, FixedOffset};
use dd_pg_defs_sea_orm::{
    benefactor_marketing_attribution_events as attribution_events,
    benefactor_marketing_conversion_events as conversion_events,
    benefactor_marketing_opportunities as opportunities,
    benefactor_marketing_prospect_research_briefs as research_briefs,
    benefactor_marketing_reports as reports,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder,
    QuerySelect, Set,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::redis_support::{publish_job_event, record_client_mutation};
use crate::shared::{
    array_or_default, ensure_campaign, ensure_client, ensure_optional_content_asset,
    ensure_optional_lead, limit, non_negative, now_fixed, object_or_default, probability,
    require_auth, require_write_access, ListQuery,
};
use crate::state::{AppError, AppResult, AppState};
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReportSnapshotRequest {
    client_id: Uuid,
    campaign_id: Option<Uuid>,
    report_kind: Option<String>,
    status: Option<String>,
    period_start: Option<String>,
    period_end: Option<String>,
    metrics: Option<Value>,
    narrative: Option<String>,
    delivery_targets: Option<Value>,
    generated_at: Option<DateTime<FixedOffset>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AttributionEventRequest {
    client_id: Uuid,
    campaign_id: Option<Uuid>,
    lead_id: Option<Uuid>,
    event_type: String,
    source_platform: Option<String>,
    source_event_id: Option<String>,
    occurred_at: Option<DateTime<FixedOffset>>,
    value_cents: Option<i32>,
    payload: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateResearchBriefRequest {
    client_id: Uuid,
    lead_id: Option<Uuid>,
    status: Option<String>,
    research_kind: Option<String>,
    source: Option<String>,
    summary: Option<String>,
    findings: Option<Value>,
    recommended_actions: Option<Value>,
    confidence_micros: Option<i32>,
    model_name: Option<String>,
    generated_at: Option<DateTime<FixedOffset>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecordConversionEventRequest {
    client_id: Uuid,
    campaign_id: Option<Uuid>,
    lead_id: Option<Uuid>,
    content_asset_id: Option<Uuid>,
    event_type: String,
    source_platform: Option<String>,
    source_event_id: Option<String>,
    session_id: Option<String>,
    visitor_key: Option<String>,
    occurred_at: Option<DateTime<FixedOffset>>,
    value_cents: Option<i32>,
    utm: Option<Value>,
    payload: Option<Value>,
}

pub(crate) async fn client_revenue_attribution(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    ensure_client(&state.db, client_id).await?;
    let attribution_event_count = attribution_events::Entity::find()
        .filter(attribution_events::Column::ClientId.eq(client_id))
        .count(&state.db)
        .await?;
    let recent_attribution_events = attribution_events::Entity::find()
        .filter(attribution_events::Column::ClientId.eq(client_id))
        .order_by_desc(attribution_events::Column::OccurredAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    let recent_value_cents: i64 = recent_attribution_events
        .iter()
        .map(|event| i64::from(event.value_cents))
        .sum();
    let conversion_event_count = conversion_events::Entity::find()
        .filter(conversion_events::Column::ClientId.eq(client_id))
        .count(&state.db)
        .await?;
    let recent_conversion_events = conversion_events::Entity::find()
        .filter(conversion_events::Column::ClientId.eq(client_id))
        .order_by_desc(conversion_events::Column::OccurredAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    let recent_conversion_value_cents: i64 = recent_conversion_events
        .iter()
        .map(|event| i64::from(event.value_cents))
        .sum();
    let open_opportunities = opportunities::Entity::find()
        .filter(opportunities::Column::ClientId.eq(client_id))
        .filter(opportunities::Column::Status.eq("open"))
        .order_by_desc(opportunities::Column::UpdatedAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    let forecast_cents: i64 = open_opportunities
        .iter()
        .map(|opportunity| {
            i64::from(opportunity.amount_cents) * i64::from(opportunity.probability_micros)
                / 1_000_000
        })
        .sum();
    Ok(Json(json!({
        "clientId": client_id,
        "attribution": {
            "eventCount": attribution_event_count,
            "recentValueCents": recent_value_cents,
            "recentEvents": recent_attribution_events
        },
        "conversions": {
            "eventCount": conversion_event_count,
            "recentValueCents": recent_conversion_value_cents,
            "recentEvents": recent_conversion_events
        },
        "pipeline": {
            "openOpportunityCount": open_opportunities.len(),
            "forecastCents": forecast_cents,
            "openOpportunities": open_opportunities
        }
    })))
}

pub(crate) async fn list_client_research_briefs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    ensure_client(&state.db, client_id).await?;
    let rows = research_briefs::Entity::find()
        .filter(research_briefs::Column::ClientId.eq(client_id))
        .order_by_desc(research_briefs::Column::UpdatedAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    Ok(Json(json!({ "researchBriefs": rows })))
}

pub(crate) async fn list_client_conversion_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    ensure_client(&state.db, client_id).await?;
    let rows = conversion_events::Entity::find()
        .filter(conversion_events::Column::ClientId.eq(client_id))
        .order_by_desc(conversion_events::Column::OccurredAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    Ok(Json(json!({ "conversionEvents": rows })))
}

pub(crate) async fn create_report_snapshot(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<ReportSnapshotRequest>,
) -> AppResult<(StatusCode, Json<reports::Model>)> {
    require_write_access(&state, &headers, "reports.snapshots.create").await?;
    let client_id = req.client_id;
    ensure_client(&state.db, client_id).await?;
    let model = reports::ActiveModel {
        client_id: Set(client_id),
        campaign_id: Set(req.campaign_id),
        report_kind: Set(req.report_kind.unwrap_or_else(|| "dashboard".to_string())),
        status: Set(req.status.unwrap_or_else(|| "ready".to_string())),
        period_start: Set(req.period_start),
        period_end: Set(req.period_end),
        metrics: Set(object_or_default(req.metrics, "metrics")?),
        narrative: Set(req.narrative),
        delivery_targets: Set(array_or_default(req.delivery_targets, "deliveryTargets")?),
        generated_at: Set(req.generated_at.or_else(|| Some(now_fixed()))),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    publish_job_event(
        &state,
        "report_snapshot_ready",
        json!({
            "clientId": client_id,
            "reportId": model.id,
            "campaignId": model.campaign_id,
            "reportKind": &model.report_kind,
            "status": &model.status
        }),
    )
    .await;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

pub(crate) async fn record_attribution_event(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<AttributionEventRequest>,
) -> AppResult<(StatusCode, Json<attribution_events::Model>)> {
    require_write_access(&state, &headers, "attribution.events.record").await?;
    let client_id = req.client_id;
    ensure_client(&state.db, client_id).await?;
    let model = attribution_events::ActiveModel {
        client_id: Set(client_id),
        campaign_id: Set(req.campaign_id),
        lead_id: Set(req.lead_id),
        event_type: Set(req.event_type),
        source_platform: Set(req.source_platform),
        source_event_id: Set(req.source_event_id),
        occurred_at: Set(req.occurred_at.unwrap_or_else(now_fixed)),
        value_cents: Set(req.value_cents.unwrap_or(0)),
        payload: Set(object_or_default(req.payload, "payload")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    publish_job_event(
        &state,
        "attribution_event_recorded",
        json!({
            "clientId": client_id,
            "eventId": model.id,
            "campaignId": model.campaign_id,
            "leadId": model.lead_id,
            "eventType": &model.event_type,
            "sourcePlatform": &model.source_platform,
            "valueCents": model.value_cents
        }),
    )
    .await;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

pub(crate) async fn create_research_brief(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateResearchBriefRequest>,
) -> AppResult<(StatusCode, Json<research_briefs::Model>)> {
    require_write_access(&state, &headers, "research.briefs.create").await?;
    let client_id = req.client_id;
    ensure_client(&state.db, client_id).await?;
    ensure_optional_lead(&state.db, client_id, req.lead_id).await?;
    let model = research_briefs::ActiveModel {
        client_id: Set(client_id),
        lead_id: Set(req.lead_id),
        status: Set(req.status.unwrap_or_else(|| "draft".to_string())),
        research_kind: Set(req
            .research_kind
            .unwrap_or_else(|| "account_research".to_string())),
        source: Set(req.source.unwrap_or_else(|| "ai_assisted".to_string())),
        summary: Set(req.summary),
        findings: Set(array_or_default(req.findings, "findings")?),
        recommended_actions: Set(array_or_default(
            req.recommended_actions,
            "recommendedActions",
        )?),
        confidence_micros: Set(probability(req.confidence_micros.unwrap_or(0))?),
        model_name: Set(req.model_name),
        generated_at: Set(req.generated_at.or_else(|| Some(now_fixed()))),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    state
        .metrics
        .research_briefs_total
        .fetch_add(1, Ordering::Relaxed);
    publish_job_event(
        &state,
        "prospect_research_brief_created",
        json!({
            "clientId": client_id,
            "briefId": model.id,
            "leadId": model.lead_id,
            "researchKind": &model.research_kind,
            "source": &model.source,
            "status": &model.status
        }),
    )
    .await;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

pub(crate) async fn record_conversion_event(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RecordConversionEventRequest>,
) -> AppResult<(StatusCode, Json<conversion_events::Model>)> {
    require_write_access(&state, &headers, "conversion.events.record").await?;
    let client_id = req.client_id;
    ensure_client(&state.db, client_id).await?;
    if let Some(campaign_id) = req.campaign_id {
        let campaign = ensure_campaign(&state.db, campaign_id).await?;
        if campaign.client_id != client_id {
            return Err(AppError::BadRequest(
                "campaignId must belong to the conversion client".to_string(),
            ));
        }
    }
    ensure_optional_lead(&state.db, client_id, req.lead_id).await?;
    ensure_optional_content_asset(&state.db, client_id, req.content_asset_id).await?;
    let model = conversion_events::ActiveModel {
        client_id: Set(client_id),
        campaign_id: Set(req.campaign_id),
        lead_id: Set(req.lead_id),
        content_asset_id: Set(req.content_asset_id),
        event_type: Set(req.event_type),
        source_platform: Set(req.source_platform),
        source_event_id: Set(req.source_event_id),
        session_id: Set(req.session_id),
        visitor_key: Set(req.visitor_key),
        occurred_at: Set(req.occurred_at.unwrap_or_else(now_fixed)),
        value_cents: Set(non_negative(req.value_cents.unwrap_or(0), "valueCents")?),
        utm: Set(object_or_default(req.utm, "utm")?),
        payload: Set(object_or_default(req.payload, "payload")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    state
        .metrics
        .conversion_events_total
        .fetch_add(1, Ordering::Relaxed);
    publish_job_event(
        &state,
        "conversion_event_recorded",
        json!({
            "clientId": client_id,
            "eventId": model.id,
            "campaignId": model.campaign_id,
            "leadId": model.lead_id,
            "eventType": &model.event_type,
            "sourcePlatform": &model.source_platform,
            "valueCents": model.value_cents
        }),
    )
    .await;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}
