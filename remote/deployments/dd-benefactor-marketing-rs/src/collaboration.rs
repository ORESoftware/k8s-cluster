use std::sync::atomic::Ordering;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use chrono::{DateTime, FixedOffset};
use dd_pg_defs_sea_orm::{
    benefactor_marketing_client_approvals as client_approvals,
    benefactor_marketing_collaboration_comments as collaboration_comments,
    benefactor_marketing_notifications as notifications,
    benefactor_marketing_portal_members as portal_members,
    benefactor_marketing_project_tasks as project_tasks,
    benefactor_marketing_shared_documents as shared_documents,
    benefactor_marketing_team_allocations as team_allocations,
    benefactor_marketing_tickets as tickets,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect, Set,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::redis_support::{publish_job_event, record_client_mutation};
use crate::shared::{
    ensure_campaign, ensure_client, ensure_optional_campaign_for_client, ensure_optional_contact,
    ensure_optional_content_asset, ensure_optional_parent_comment, limit, now_fixed,
    object_or_default, percent, require_auth, require_write_access, ListQuery,
};
use crate::state::{AppError, AppResult, AppState};
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateProjectTaskRequest {
    client_id: Uuid,
    campaign_id: Option<Uuid>,
    content_asset_id: Option<Uuid>,
    status: Option<String>,
    priority: Option<String>,
    title: String,
    description: Option<String>,
    assigned_to: Option<Uuid>,
    due_on: Option<String>,
    sla_due_at: Option<DateTime<FixedOffset>>,
    time_spent_minutes: Option<i32>,
    meta_data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateApprovalRequest {
    client_id: Uuid,
    campaign_id: Option<Uuid>,
    content_asset_id: Option<Uuid>,
    requested_by: Option<Uuid>,
    approval_kind: String,
    title: String,
    request_payload: Option<Value>,
    due_at: Option<DateTime<FixedOffset>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DecideApprovalRequest {
    status: String,
    response_note: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateTicketRequest {
    client_id: Uuid,
    status: Option<String>,
    priority: Option<String>,
    subject: String,
    description: Option<String>,
    source: Option<String>,
    assigned_to: Option<Uuid>,
    meta_data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateTeamAllocationRequest {
    campaign_id: Option<Uuid>,
    user_id: Uuid,
    role: String,
    allocation_percent: Option<i32>,
    starts_on: Option<String>,
    ends_on: Option<String>,
    billable: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreatePortalMemberRequest {
    contact_id: Option<Uuid>,
    user_id: Option<Uuid>,
    email: String,
    status: Option<String>,
    role: Option<String>,
    access_scope: Option<Value>,
    last_seen_at: Option<DateTime<FixedOffset>>,
    accepted_at: Option<DateTime<FixedOffset>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateSharedDocumentRequest {
    campaign_id: Option<Uuid>,
    content_asset_id: Option<Uuid>,
    status: Option<String>,
    document_kind: String,
    title: String,
    storage_uri: String,
    mime_type: Option<String>,
    visibility: Option<String>,
    uploaded_by: Option<Uuid>,
    meta_data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateCommentRequest {
    parent_comment_id: Option<Uuid>,
    resource_type: String,
    resource_id: Option<Uuid>,
    author_user_id: Option<Uuid>,
    author_contact_id: Option<Uuid>,
    body: String,
    status: Option<String>,
    visibility: Option<String>,
    meta_data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateNotificationRequest {
    recipient_user_id: Option<Uuid>,
    recipient_contact_id: Option<Uuid>,
    channel: Option<String>,
    status: Option<String>,
    notification_kind: String,
    title: String,
    body: Option<String>,
    payload: Option<Value>,
    scheduled_at: Option<DateTime<FixedOffset>>,
    sent_at: Option<DateTime<FixedOffset>>,
}

pub(crate) async fn list_client_team_allocations(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    ensure_client(&state.db, client_id).await?;
    let rows = team_allocations::Entity::find()
        .filter(team_allocations::Column::ClientId.eq(Some(client_id)))
        .order_by_desc(team_allocations::Column::UpdatedAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    Ok(Json(json!({ "teamAllocations": rows })))
}

pub(crate) async fn create_team_allocation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Json(req): Json<CreateTeamAllocationRequest>,
) -> AppResult<(StatusCode, Json<team_allocations::Model>)> {
    require_write_access(&state, &headers, "team.allocations.create").await?;
    ensure_client(&state.db, client_id).await?;
    if let Some(campaign_id) = req.campaign_id {
        let campaign = ensure_campaign(&state.db, campaign_id).await?;
        if campaign.client_id != client_id {
            return Err(AppError::BadRequest(
                "campaignId must belong to the route client".to_string(),
            ));
        }
    }
    let model = team_allocations::ActiveModel {
        client_id: Set(Some(client_id)),
        campaign_id: Set(req.campaign_id),
        user_id: Set(req.user_id),
        role: Set(req.role),
        allocation_percent: Set(percent(req.allocation_percent.unwrap_or(100))?),
        starts_on: Set(req.starts_on),
        ends_on: Set(req.ends_on),
        billable: Set(req.billable.unwrap_or(true)),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

pub(crate) async fn list_client_portal_members(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    ensure_client(&state.db, client_id).await?;
    let rows = portal_members::Entity::find()
        .filter(portal_members::Column::ClientId.eq(client_id))
        .order_by_desc(portal_members::Column::UpdatedAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    Ok(Json(json!({ "portalMembers": rows })))
}

pub(crate) async fn create_portal_member(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Json(req): Json<CreatePortalMemberRequest>,
) -> AppResult<(StatusCode, Json<portal_members::Model>)> {
    require_write_access(&state, &headers, "portal.members.create").await?;
    ensure_client(&state.db, client_id).await?;
    ensure_optional_contact(&state.db, client_id, req.contact_id).await?;
    let model = portal_members::ActiveModel {
        client_id: Set(client_id),
        contact_id: Set(req.contact_id),
        user_id: Set(req.user_id),
        email: Set(req.email),
        status: Set(req.status.unwrap_or_else(|| "invited".to_string())),
        role: Set(req.role.unwrap_or_else(|| "viewer".to_string())),
        access_scope: Set(object_or_default(req.access_scope, "accessScope")?),
        last_seen_at: Set(req.last_seen_at),
        accepted_at: Set(req.accepted_at),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    state
        .metrics
        .client_collaboration_events_total
        .fetch_add(1, Ordering::Relaxed);
    publish_job_event(
        &state,
        "portal_member_created",
        json!({
            "clientId": client_id,
            "portalMemberId": model.id,
            "contactId": model.contact_id,
            "userId": model.user_id,
            "role": &model.role,
            "status": &model.status
        }),
    )
    .await;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

pub(crate) async fn list_client_documents(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    ensure_client(&state.db, client_id).await?;
    let rows = shared_documents::Entity::find()
        .filter(shared_documents::Column::ClientId.eq(client_id))
        .order_by_desc(shared_documents::Column::UpdatedAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    Ok(Json(json!({ "documents": rows })))
}

pub(crate) async fn create_shared_document(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Json(req): Json<CreateSharedDocumentRequest>,
) -> AppResult<(StatusCode, Json<shared_documents::Model>)> {
    require_write_access(&state, &headers, "documents.create").await?;
    ensure_client(&state.db, client_id).await?;
    ensure_optional_campaign_for_client(&state.db, client_id, req.campaign_id).await?;
    ensure_optional_content_asset(&state.db, client_id, req.content_asset_id).await?;
    let model = shared_documents::ActiveModel {
        client_id: Set(client_id),
        campaign_id: Set(req.campaign_id),
        content_asset_id: Set(req.content_asset_id),
        status: Set(req.status.unwrap_or_else(|| "draft".to_string())),
        document_kind: Set(req.document_kind),
        title: Set(req.title),
        storage_uri: Set(req.storage_uri),
        mime_type: Set(req.mime_type),
        visibility: Set(req.visibility.unwrap_or_else(|| "client".to_string())),
        uploaded_by: Set(req.uploaded_by),
        meta_data: Set(object_or_default(req.meta_data, "metaData")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    state
        .metrics
        .client_collaboration_events_total
        .fetch_add(1, Ordering::Relaxed);
    publish_job_event(
        &state,
        "shared_document_created",
        json!({
            "clientId": client_id,
            "documentId": model.id,
            "campaignId": model.campaign_id,
            "contentAssetId": model.content_asset_id,
            "documentKind": &model.document_kind,
            "visibility": &model.visibility,
            "status": &model.status
        }),
    )
    .await;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

pub(crate) async fn list_client_comments(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    ensure_client(&state.db, client_id).await?;
    let rows = collaboration_comments::Entity::find()
        .filter(collaboration_comments::Column::ClientId.eq(client_id))
        .order_by_desc(collaboration_comments::Column::UpdatedAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    Ok(Json(json!({ "comments": rows })))
}

pub(crate) async fn create_comment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Json(req): Json<CreateCommentRequest>,
) -> AppResult<(StatusCode, Json<collaboration_comments::Model>)> {
    require_write_access(&state, &headers, "comments.create").await?;
    if req.author_user_id.is_none() && req.author_contact_id.is_none() {
        return Err(AppError::BadRequest(
            "authorUserId or authorContactId is required".to_string(),
        ));
    }
    ensure_client(&state.db, client_id).await?;
    ensure_optional_contact(&state.db, client_id, req.author_contact_id).await?;
    ensure_optional_parent_comment(&state.db, client_id, req.parent_comment_id).await?;
    let model = collaboration_comments::ActiveModel {
        client_id: Set(client_id),
        parent_comment_id: Set(req.parent_comment_id),
        resource_type: Set(req.resource_type),
        resource_id: Set(req.resource_id),
        author_user_id: Set(req.author_user_id),
        author_contact_id: Set(req.author_contact_id),
        body: Set(req.body),
        status: Set(req.status.unwrap_or_else(|| "open".to_string())),
        visibility: Set(req.visibility.unwrap_or_else(|| "client".to_string())),
        meta_data: Set(object_or_default(req.meta_data, "metaData")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    state
        .metrics
        .client_collaboration_events_total
        .fetch_add(1, Ordering::Relaxed);
    publish_job_event(
        &state,
        "collaboration_comment_created",
        json!({
            "clientId": client_id,
            "commentId": model.id,
            "parentCommentId": model.parent_comment_id,
            "resourceType": &model.resource_type,
            "resourceId": model.resource_id,
            "visibility": &model.visibility,
            "status": &model.status
        }),
    )
    .await;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

pub(crate) async fn list_client_notifications(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    ensure_client(&state.db, client_id).await?;
    let rows = notifications::Entity::find()
        .filter(notifications::Column::ClientId.eq(client_id))
        .order_by_desc(notifications::Column::ScheduledAt)
        .order_by_desc(notifications::Column::UpdatedAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    Ok(Json(json!({ "notifications": rows })))
}

pub(crate) async fn create_notification(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Json(req): Json<CreateNotificationRequest>,
) -> AppResult<(StatusCode, Json<notifications::Model>)> {
    require_write_access(&state, &headers, "notifications.create").await?;
    if req.recipient_user_id.is_none() && req.recipient_contact_id.is_none() {
        return Err(AppError::BadRequest(
            "recipientUserId or recipientContactId is required".to_string(),
        ));
    }
    ensure_client(&state.db, client_id).await?;
    ensure_optional_contact(&state.db, client_id, req.recipient_contact_id).await?;
    let model = notifications::ActiveModel {
        client_id: Set(client_id),
        recipient_user_id: Set(req.recipient_user_id),
        recipient_contact_id: Set(req.recipient_contact_id),
        channel: Set(req.channel.unwrap_or_else(|| "email".to_string())),
        status: Set(req.status.unwrap_or_else(|| "queued".to_string())),
        notification_kind: Set(req.notification_kind),
        title: Set(req.title),
        body: Set(req.body),
        payload: Set(object_or_default(req.payload, "payload")?),
        scheduled_at: Set(req.scheduled_at),
        sent_at: Set(req.sent_at),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    state
        .metrics
        .client_collaboration_events_total
        .fetch_add(1, Ordering::Relaxed);
    publish_job_event(
        &state,
        "client_notification_queued",
        json!({
            "clientId": client_id,
            "notificationId": model.id,
            "recipientUserId": model.recipient_user_id,
            "recipientContactId": model.recipient_contact_id,
            "channel": &model.channel,
            "notificationKind": &model.notification_kind,
            "status": &model.status
        }),
    )
    .await;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

pub(crate) async fn create_project_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateProjectTaskRequest>,
) -> AppResult<(StatusCode, Json<project_tasks::Model>)> {
    require_write_access(&state, &headers, "projects.tasks.create").await?;
    let client_id = req.client_id;
    ensure_client(&state.db, client_id).await?;
    let model = project_tasks::ActiveModel {
        client_id: Set(client_id),
        campaign_id: Set(req.campaign_id),
        content_asset_id: Set(req.content_asset_id),
        status: Set(req.status.unwrap_or_else(|| "todo".to_string())),
        priority: Set(req.priority.unwrap_or_else(|| "normal".to_string())),
        title: Set(req.title),
        description: Set(req.description),
        assigned_to: Set(req.assigned_to),
        due_on: Set(req.due_on),
        sla_due_at: Set(req.sla_due_at),
        time_spent_minutes: Set(req.time_spent_minutes.unwrap_or(0)),
        meta_data: Set(object_or_default(req.meta_data, "metaData")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

pub(crate) async fn create_approval(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateApprovalRequest>,
) -> AppResult<(StatusCode, Json<client_approvals::Model>)> {
    require_write_access(&state, &headers, "approvals.create").await?;
    let client_id = req.client_id;
    ensure_client(&state.db, client_id).await?;
    let model = client_approvals::ActiveModel {
        client_id: Set(client_id),
        campaign_id: Set(req.campaign_id),
        content_asset_id: Set(req.content_asset_id),
        requested_by: Set(req.requested_by),
        status: Set("pending".to_string()),
        approval_kind: Set(req.approval_kind),
        title: Set(req.title),
        request_payload: Set(object_or_default(req.request_payload, "requestPayload")?),
        due_at: Set(req.due_at),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

pub(crate) async fn decide_approval(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(approval_id): Path<Uuid>,
    Json(req): Json<DecideApprovalRequest>,
) -> AppResult<Json<client_approvals::Model>> {
    require_write_access(&state, &headers, "approvals.decide").await?;
    if !["approved", "rejected", "canceled", "expired"].contains(&req.status.as_str()) {
        return Err(AppError::BadRequest(
            "approval decision status must be approved, rejected, canceled, or expired".to_string(),
        ));
    }
    let approval = client_approvals::Entity::find_by_id(approval_id)
        .one(&state.db)
        .await?
        .ok_or(AppError::NotFound("approval"))?;
    let client_id = approval.client_id;
    let mut active: client_approvals::ActiveModel = approval.into();
    active.status = Set(req.status);
    active.response_note = Set(req.response_note);
    active.decided_at = Set(Some(now_fixed()));
    active.updated_at = Set(now_fixed());
    let model = active.update(&state.db).await?;
    record_client_mutation(&state, client_id).await;
    Ok(Json(model))
}

pub(crate) async fn create_ticket(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateTicketRequest>,
) -> AppResult<(StatusCode, Json<tickets::Model>)> {
    require_write_access(&state, &headers, "tickets.create").await?;
    let client_id = req.client_id;
    ensure_client(&state.db, client_id).await?;
    let model = tickets::ActiveModel {
        client_id: Set(client_id),
        status: Set(req.status.unwrap_or_else(|| "open".to_string())),
        priority: Set(req.priority.unwrap_or_else(|| "normal".to_string())),
        subject: Set(req.subject),
        description: Set(req.description),
        source: Set(req.source.unwrap_or_else(|| "portal".to_string())),
        assigned_to: Set(req.assigned_to),
        last_activity_at: Set(now_fixed()),
        meta_data: Set(object_or_default(req.meta_data, "metaData")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}
