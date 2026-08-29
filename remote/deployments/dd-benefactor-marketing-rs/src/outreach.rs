use std::sync::atomic::Ordering;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use chrono::{DateTime, FixedOffset};
use dd_pg_defs_sea_orm::{
    benefactor_marketing_automation_events as automation_events,
    benefactor_marketing_automation_workflows as automation_workflows,
    benefactor_marketing_outreach_enrollments as outreach_enrollments,
    benefactor_marketing_outreach_sequences as outreach_sequences,
    benefactor_marketing_outreach_steps as outreach_steps,
    benefactor_marketing_outreach_touchpoints as outreach_touchpoints,
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
    array_or_default, ensure_campaign, ensure_client, ensure_optional_contact,
    ensure_optional_enrollment, ensure_optional_lead, ensure_outreach_sequence, limit,
    non_negative, now_fixed, object_or_default, require_auth, require_write_access, step_order,
    ListQuery,
};
use crate::state::{AppError, AppResult, AppState};
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateOutreachSequenceRequest {
    client_id: Uuid,
    campaign_id: Option<Uuid>,
    status: Option<String>,
    channel: Option<String>,
    name: String,
    audience_filter: Option<Value>,
    cadence: Option<Value>,
    meta_data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateOutreachStepRequest {
    status: Option<String>,
    step_order: i32,
    channel: String,
    delay_minutes: Option<i32>,
    subject: Option<String>,
    body_template: Option<String>,
    personalization_hints: Option<Value>,
    experiment_key: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateOutreachEnrollmentRequest {
    client_id: Uuid,
    sequence_id: Uuid,
    lead_id: Option<Uuid>,
    contact_id: Option<Uuid>,
    status: Option<String>,
    current_step_order: Option<i32>,
    enrollment_context: Option<Value>,
    last_touch_at: Option<DateTime<FixedOffset>>,
    next_touch_at: Option<DateTime<FixedOffset>>,
    outcome: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecordOutreachTouchpointRequest {
    client_id: Uuid,
    sequence_id: Option<Uuid>,
    enrollment_id: Option<Uuid>,
    campaign_id: Option<Uuid>,
    lead_id: Option<Uuid>,
    contact_id: Option<Uuid>,
    channel: String,
    direction: Option<String>,
    status: Option<String>,
    subject: Option<String>,
    body_excerpt: Option<String>,
    external_message_id: Option<String>,
    occurred_at: Option<DateTime<FixedOffset>>,
    payload: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateAutomationWorkflowRequest {
    client_id: Uuid,
    status: Option<String>,
    name: String,
    trigger_kind: String,
    trigger_config: Option<Value>,
    action_graph: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AutomationEventRequest {
    client_id: Uuid,
    workflow_id: Option<Uuid>,
    lead_id: Option<Uuid>,
    event_kind: String,
    status: Option<String>,
    payload: Option<Value>,
}

pub(crate) async fn client_outreach_summary(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    ensure_client(&state.db, client_id).await?;
    let sequence_count = outreach_sequences::Entity::find()
        .filter(outreach_sequences::Column::ClientId.eq(client_id))
        .count(&state.db)
        .await?;
    let active_sequence_count = outreach_sequences::Entity::find()
        .filter(outreach_sequences::Column::ClientId.eq(client_id))
        .filter(outreach_sequences::Column::Status.eq("active"))
        .count(&state.db)
        .await?;
    let active_enrollment_count = outreach_enrollments::Entity::find()
        .filter(outreach_enrollments::Column::ClientId.eq(client_id))
        .filter(outreach_enrollments::Column::Status.eq("active"))
        .count(&state.db)
        .await?;
    let recent_touchpoints = outreach_touchpoints::Entity::find()
        .filter(outreach_touchpoints::Column::ClientId.eq(client_id))
        .order_by_desc(outreach_touchpoints::Column::OccurredAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    let upcoming_enrollments = outreach_enrollments::Entity::find()
        .filter(outreach_enrollments::Column::ClientId.eq(client_id))
        .filter(outreach_enrollments::Column::Status.eq("active"))
        .order_by_asc(outreach_enrollments::Column::NextTouchAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    Ok(Json(json!({
        "clientId": client_id,
        "counts": {
            "sequences": sequence_count,
            "activeSequences": active_sequence_count,
            "activeEnrollments": active_enrollment_count
        },
        "recentTouchpoints": recent_touchpoints,
        "upcomingEnrollments": upcoming_enrollments
    })))
}

pub(crate) async fn list_client_outreach_sequences(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    ensure_client(&state.db, client_id).await?;
    let rows = outreach_sequences::Entity::find()
        .filter(outreach_sequences::Column::ClientId.eq(client_id))
        .order_by_desc(outreach_sequences::Column::UpdatedAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    Ok(Json(json!({ "outreachSequences": rows })))
}

pub(crate) async fn create_outreach_sequence(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateOutreachSequenceRequest>,
) -> AppResult<(StatusCode, Json<outreach_sequences::Model>)> {
    require_write_access(&state, &headers, "outreach.sequences.create").await?;
    let client_id = req.client_id;
    ensure_client(&state.db, client_id).await?;
    if let Some(campaign_id) = req.campaign_id {
        let campaign = ensure_campaign(&state.db, campaign_id).await?;
        if campaign.client_id != client_id {
            return Err(AppError::BadRequest(
                "campaignId must belong to the sequence client".to_string(),
            ));
        }
    }
    let model = outreach_sequences::ActiveModel {
        client_id: Set(client_id),
        campaign_id: Set(req.campaign_id),
        status: Set(req.status.unwrap_or_else(|| "draft".to_string())),
        channel: Set(req.channel.unwrap_or_else(|| "email".to_string())),
        name: Set(req.name),
        audience_filter: Set(object_or_default(req.audience_filter, "audienceFilter")?),
        cadence: Set(object_or_default(req.cadence, "cadence")?),
        meta_data: Set(object_or_default(req.meta_data, "metaData")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

pub(crate) async fn create_outreach_step(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(sequence_id): Path<Uuid>,
    Json(req): Json<CreateOutreachStepRequest>,
) -> AppResult<(StatusCode, Json<outreach_steps::Model>)> {
    require_write_access(&state, &headers, "outreach.steps.create").await?;
    let sequence = ensure_outreach_sequence(&state.db, sequence_id).await?;
    let model = outreach_steps::ActiveModel {
        sequence_id: Set(sequence_id),
        status: Set(req.status.unwrap_or_else(|| "active".to_string())),
        step_order: Set(step_order(req.step_order)?),
        channel: Set(req.channel),
        delay_minutes: Set(non_negative(
            req.delay_minutes.unwrap_or(0),
            "delayMinutes",
        )?),
        subject: Set(req.subject),
        body_template: Set(req.body_template),
        personalization_hints: Set(array_or_default(
            req.personalization_hints,
            "personalizationHints",
        )?),
        experiment_key: Set(req.experiment_key),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_client_mutation(&state, sequence.client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

pub(crate) async fn create_outreach_enrollment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateOutreachEnrollmentRequest>,
) -> AppResult<(StatusCode, Json<outreach_enrollments::Model>)> {
    require_write_access(&state, &headers, "outreach.enrollments.create").await?;
    if req.lead_id.is_none() && req.contact_id.is_none() {
        return Err(AppError::BadRequest(
            "leadId or contactId is required".to_string(),
        ));
    }
    let client_id = req.client_id;
    ensure_client(&state.db, client_id).await?;
    let sequence = ensure_outreach_sequence(&state.db, req.sequence_id).await?;
    if sequence.client_id != client_id {
        return Err(AppError::BadRequest(
            "sequenceId must belong to the enrollment client".to_string(),
        ));
    }
    ensure_optional_lead(&state.db, client_id, req.lead_id).await?;
    ensure_optional_contact(&state.db, client_id, req.contact_id).await?;
    let model = outreach_enrollments::ActiveModel {
        client_id: Set(client_id),
        sequence_id: Set(req.sequence_id),
        lead_id: Set(req.lead_id),
        contact_id: Set(req.contact_id),
        status: Set(req.status.unwrap_or_else(|| "active".to_string())),
        current_step_order: Set(step_order(req.current_step_order.unwrap_or(1))?),
        enrollment_context: Set(object_or_default(
            req.enrollment_context,
            "enrollmentContext",
        )?),
        last_touch_at: Set(req.last_touch_at),
        next_touch_at: Set(req.next_touch_at),
        outcome: Set(req.outcome),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    publish_job_event(
        &state,
        "outreach_enrollment_created",
        json!({
            "clientId": client_id,
            "sequenceId": model.sequence_id,
            "enrollmentId": model.id,
            "leadId": model.lead_id,
            "contactId": model.contact_id,
            "status": &model.status
        }),
    )
    .await;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

pub(crate) async fn record_outreach_touchpoint(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RecordOutreachTouchpointRequest>,
) -> AppResult<(StatusCode, Json<outreach_touchpoints::Model>)> {
    require_write_access(&state, &headers, "outreach.touchpoints.record").await?;
    let client_id = req.client_id;
    ensure_client(&state.db, client_id).await?;
    if let Some(sequence_id) = req.sequence_id {
        let sequence = ensure_outreach_sequence(&state.db, sequence_id).await?;
        if sequence.client_id != client_id {
            return Err(AppError::BadRequest(
                "sequenceId must belong to the touchpoint client".to_string(),
            ));
        }
    }
    ensure_optional_enrollment(&state.db, client_id, req.enrollment_id).await?;
    if let Some(campaign_id) = req.campaign_id {
        let campaign = ensure_campaign(&state.db, campaign_id).await?;
        if campaign.client_id != client_id {
            return Err(AppError::BadRequest(
                "campaignId must belong to the touchpoint client".to_string(),
            ));
        }
    }
    ensure_optional_lead(&state.db, client_id, req.lead_id).await?;
    ensure_optional_contact(&state.db, client_id, req.contact_id).await?;
    let model = outreach_touchpoints::ActiveModel {
        client_id: Set(client_id),
        sequence_id: Set(req.sequence_id),
        enrollment_id: Set(req.enrollment_id),
        campaign_id: Set(req.campaign_id),
        lead_id: Set(req.lead_id),
        contact_id: Set(req.contact_id),
        channel: Set(req.channel),
        direction: Set(req.direction.unwrap_or_else(|| "outbound".to_string())),
        status: Set(req.status.unwrap_or_else(|| "planned".to_string())),
        subject: Set(req.subject),
        body_excerpt: Set(req.body_excerpt),
        external_message_id: Set(req.external_message_id),
        occurred_at: Set(req.occurred_at.unwrap_or_else(now_fixed)),
        payload: Set(object_or_default(req.payload, "payload")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    state
        .metrics
        .outreach_touchpoints_total
        .fetch_add(1, Ordering::Relaxed);
    publish_job_event(
        &state,
        "outreach_touchpoint_recorded",
        json!({
            "clientId": client_id,
            "touchpointId": model.id,
            "sequenceId": model.sequence_id,
            "enrollmentId": model.enrollment_id,
            "leadId": model.lead_id,
            "contactId": model.contact_id,
            "channel": &model.channel,
            "status": &model.status
        }),
    )
    .await;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

pub(crate) async fn create_automation_workflow(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateAutomationWorkflowRequest>,
) -> AppResult<(StatusCode, Json<automation_workflows::Model>)> {
    require_write_access(&state, &headers, "automation.workflows.create").await?;
    let client_id = req.client_id;
    ensure_client(&state.db, client_id).await?;
    let model = automation_workflows::ActiveModel {
        client_id: Set(client_id),
        status: Set(req.status.unwrap_or_else(|| "draft".to_string())),
        name: Set(req.name),
        trigger_kind: Set(req.trigger_kind),
        trigger_config: Set(object_or_default(req.trigger_config, "triggerConfig")?),
        action_graph: Set(object_or_default(req.action_graph, "actionGraph")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

pub(crate) async fn record_automation_event(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<AutomationEventRequest>,
) -> AppResult<(StatusCode, Json<automation_events::Model>)> {
    require_write_access(&state, &headers, "automation.events.record").await?;
    let client_id = req.client_id;
    ensure_client(&state.db, client_id).await?;
    let model = automation_events::ActiveModel {
        client_id: Set(client_id),
        workflow_id: Set(req.workflow_id),
        lead_id: Set(req.lead_id),
        event_kind: Set(req.event_kind),
        status: Set(req.status.unwrap_or_else(|| "received".to_string())),
        payload: Set(object_or_default(req.payload, "payload")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    publish_job_event(
        &state,
        "automation_event_recorded",
        json!({
            "clientId": client_id,
            "eventId": model.id,
            "workflowId": model.workflow_id,
            "leadId": model.lead_id,
            "eventKind": &model.event_kind,
            "status": &model.status
        }),
    )
    .await;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}
