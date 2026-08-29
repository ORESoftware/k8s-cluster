use std::sync::atomic::Ordering;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use chrono::{DateTime, FixedOffset};
use dd_pg_defs_sea_orm::{
    benefactor_marketing_call_insights as call_insights,
    benefactor_marketing_meetings as meetings,
    benefactor_marketing_opportunities as opportunities,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect, Set,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::redis_support::{publish_job_event, record_client_mutation};
use crate::shared::{
    array_or_default, ensure_client, ensure_meeting, ensure_optional_lead,
    ensure_optional_meeting_for_client, ensure_optional_opportunity, limit, now_fixed,
    object_or_default, probability, require_auth, require_write_access, ListQuery,
};
use crate::state::{AppError, AppResult, AppState};
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateOpportunityRequest {
    client_id: Uuid,
    lead_id: Option<Uuid>,
    status: Option<String>,
    stage: Option<String>,
    name: String,
    amount_cents: Option<i32>,
    probability_micros: Option<i32>,
    expected_close_on: Option<String>,
    owner_user_id: Option<Uuid>,
    meta_data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateMeetingRequest {
    client_id: Uuid,
    lead_id: Option<Uuid>,
    opportunity_id: Option<Uuid>,
    status: Option<String>,
    meeting_kind: String,
    title: String,
    scheduled_at: DateTime<FixedOffset>,
    duration_minutes: Option<i32>,
    notes: Option<String>,
    recording_uri: Option<String>,
    transcript_summary: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateCallInsightRequest {
    meeting_id: Option<Uuid>,
    lead_id: Option<Uuid>,
    opportunity_id: Option<Uuid>,
    status: Option<String>,
    provider: Option<String>,
    transcript_uri: Option<String>,
    summary: Option<String>,
    sentiment: Option<String>,
    action_items: Option<Value>,
    objections: Option<Value>,
    next_steps: Option<Value>,
    confidence_micros: Option<i32>,
    analyzed_at: Option<DateTime<FixedOffset>>,
}

pub(crate) async fn list_client_call_insights(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    ensure_client(&state.db, client_id).await?;
    let rows = call_insights::Entity::find()
        .filter(call_insights::Column::ClientId.eq(client_id))
        .order_by_desc(call_insights::Column::AnalyzedAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    Ok(Json(json!({ "callInsights": rows })))
}

pub(crate) async fn create_client_call_insight(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Json(req): Json<CreateCallInsightRequest>,
) -> AppResult<(StatusCode, Json<call_insights::Model>)> {
    require_write_access(&state, &headers, "call-insights.create").await?;
    ensure_client(&state.db, client_id).await?;
    let meeting_id = req.meeting_id;
    let model = create_call_insight_for_client(&state, client_id, meeting_id, req).await?;
    Ok((StatusCode::CREATED, Json(model)))
}

pub(crate) async fn create_meeting_call_insight(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(meeting_id): Path<Uuid>,
    Json(mut req): Json<CreateCallInsightRequest>,
) -> AppResult<(StatusCode, Json<call_insights::Model>)> {
    require_write_access(&state, &headers, "meetings.call-insights.create").await?;
    if req.meeting_id.is_some() && req.meeting_id != Some(meeting_id) {
        return Err(AppError::BadRequest(
            "meetingId must match the route meeting".to_string(),
        ));
    }
    let meeting = ensure_meeting(&state.db, meeting_id).await?;
    req.lead_id = req.lead_id.or(meeting.lead_id);
    req.opportunity_id = req.opportunity_id.or(meeting.opportunity_id);
    let model =
        create_call_insight_for_client(&state, meeting.client_id, Some(meeting_id), req).await?;
    Ok((StatusCode::CREATED, Json(model)))
}

pub(crate) async fn create_opportunity(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateOpportunityRequest>,
) -> AppResult<(StatusCode, Json<opportunities::Model>)> {
    require_write_access(&state, &headers, "opportunities.create").await?;
    let client_id = req.client_id;
    ensure_client(&state.db, client_id).await?;
    let model = opportunities::ActiveModel {
        client_id: Set(client_id),
        lead_id: Set(req.lead_id),
        status: Set(req.status.unwrap_or_else(|| "open".to_string())),
        stage: Set(req.stage.unwrap_or_else(|| "prospecting".to_string())),
        name: Set(req.name),
        amount_cents: Set(req.amount_cents.unwrap_or(0)),
        probability_micros: Set(probability(req.probability_micros.unwrap_or(0))?),
        expected_close_on: Set(req.expected_close_on),
        owner_user_id: Set(req.owner_user_id),
        meta_data: Set(object_or_default(req.meta_data, "metaData")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

pub(crate) async fn create_meeting(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateMeetingRequest>,
) -> AppResult<(StatusCode, Json<meetings::Model>)> {
    require_write_access(&state, &headers, "meetings.create").await?;
    let client_id = req.client_id;
    ensure_client(&state.db, client_id).await?;
    let model = meetings::ActiveModel {
        client_id: Set(client_id),
        lead_id: Set(req.lead_id),
        opportunity_id: Set(req.opportunity_id),
        status: Set(req.status.unwrap_or_else(|| "scheduled".to_string())),
        meeting_kind: Set(req.meeting_kind),
        title: Set(req.title),
        scheduled_at: Set(req.scheduled_at),
        duration_minutes: Set(req.duration_minutes.unwrap_or(30)),
        notes: Set(req.notes),
        recording_uri: Set(req.recording_uri),
        transcript_summary: Set(object_or_default(
            req.transcript_summary,
            "transcriptSummary",
        )?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

async fn create_call_insight_for_client(
    state: &AppState,
    client_id: Uuid,
    meeting_id: Option<Uuid>,
    req: CreateCallInsightRequest,
) -> AppResult<call_insights::Model> {
    ensure_optional_meeting_for_client(&state.db, client_id, meeting_id).await?;
    ensure_optional_lead(&state.db, client_id, req.lead_id).await?;
    ensure_optional_opportunity(&state.db, client_id, req.opportunity_id).await?;
    let model = call_insights::ActiveModel {
        client_id: Set(client_id),
        meeting_id: Set(meeting_id),
        lead_id: Set(req.lead_id),
        opportunity_id: Set(req.opportunity_id),
        status: Set(req.status.unwrap_or_else(|| "ready".to_string())),
        provider: Set(req.provider),
        transcript_uri: Set(req.transcript_uri),
        summary: Set(req.summary),
        sentiment: Set(req.sentiment),
        action_items: Set(array_or_default(req.action_items, "actionItems")?),
        objections: Set(array_or_default(req.objections, "objections")?),
        next_steps: Set(array_or_default(req.next_steps, "nextSteps")?),
        confidence_micros: Set(probability(req.confidence_micros.unwrap_or(0))?),
        analyzed_at: Set(req.analyzed_at.unwrap_or_else(now_fixed)),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    state
        .metrics
        .call_insights_total
        .fetch_add(1, Ordering::Relaxed);
    publish_job_event(
        state,
        "call_insight_recorded",
        json!({
            "clientId": client_id,
            "callInsightId": model.id,
            "meetingId": model.meeting_id,
            "leadId": model.lead_id,
            "opportunityId": model.opportunity_id,
            "provider": &model.provider,
            "sentiment": &model.sentiment,
            "status": &model.status
        }),
    )
    .await;
    record_client_mutation(state, client_id).await;
    Ok(model)
}
