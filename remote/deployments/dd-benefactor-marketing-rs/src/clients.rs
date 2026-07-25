use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use dd_pg_defs_sea_orm::{
    benefactor_marketing_call_insights as call_insights,
    benefactor_marketing_campaigns as campaigns,
    benefactor_marketing_client_approvals as client_approvals,
    benefactor_marketing_clients as clients,
    benefactor_marketing_collaboration_comments as collaboration_comments,
    benefactor_marketing_contacts as contacts, benefactor_marketing_contracts as contracts,
    benefactor_marketing_conversion_events as conversion_events,
    benefactor_marketing_invoices as invoices, benefactor_marketing_leads as leads,
    benefactor_marketing_notifications as notifications,
    benefactor_marketing_opportunities as opportunities,
    benefactor_marketing_outreach_sequences as outreach_sequences,
    benefactor_marketing_portal_members as portal_members,
    benefactor_marketing_project_tasks as project_tasks,
    benefactor_marketing_prospect_research_briefs as research_briefs,
    benefactor_marketing_reports as reports,
    benefactor_marketing_service_packages as service_packages,
    benefactor_marketing_shared_documents as shared_documents,
    benefactor_marketing_tickets as tickets,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder,
    QuerySelect, Set,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::redis_support::{
    cache_get_json, cache_set_json, client_dashboard_cache_key, record_client_mutation,
    record_mutation,
};
use crate::shared::{
    array_or_default, ensure_client, limit, now_fixed, object_or_default, require_auth,
    require_write_access, slugify, tickets_count, ListQuery,
};
use crate::state::{AppError, AppResult, AppState};
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateClientRequest {
    name: String,
    slug: Option<String>,
    status: Option<String>,
    industry: Option<String>,
    website_url: Option<String>,
    billing_email: Option<String>,
    owner_user_id: Option<Uuid>,
    service_package: Option<String>,
    onboarding_stage: Option<String>,
    portal_enabled: Option<bool>,
    meta_data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateContactRequest {
    status: Option<String>,
    first_name: Option<String>,
    last_name: Option<String>,
    email: Option<String>,
    phone: Option<String>,
    job_title: Option<String>,
    lifecycle_role: Option<String>,
    consent_status: Option<String>,
    meta_data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateServicePackageRequest {
    status: Option<String>,
    code: String,
    name: String,
    channel_mix: Option<Value>,
    deliverables: Option<Value>,
    monthly_budget_cents: Option<i32>,
    retainer_cents: Option<i32>,
    meta_data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateContractRequest {
    package_id: Option<Uuid>,
    status: Option<String>,
    contract_number: Option<String>,
    starts_on: Option<String>,
    ends_on: Option<String>,
    billing_terms: Option<Value>,
    total_value_cents: Option<i32>,
    meta_data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateInvoiceRequest {
    contract_id: Option<Uuid>,
    status: Option<String>,
    invoice_number: Option<String>,
    due_on: Option<String>,
    amount_cents: Option<i32>,
    line_items: Option<Value>,
    meta_data: Option<Value>,
}

pub(crate) async fn list_service_packages(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    let rows = service_packages::Entity::find()
        .order_by_asc(service_packages::Column::Code)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    Ok(Json(json!({ "servicePackages": rows })))
}

pub(crate) async fn create_service_package(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateServicePackageRequest>,
) -> AppResult<(StatusCode, Json<service_packages::Model>)> {
    require_write_access(&state, &headers, "service-packages.create").await?;
    let model = service_packages::ActiveModel {
        status: Set(req.status.unwrap_or_else(|| "active".to_string())),
        code: Set(req.code),
        name: Set(req.name),
        channel_mix: Set(array_or_default(req.channel_mix, "channelMix")?),
        deliverables: Set(array_or_default(req.deliverables, "deliverables")?),
        monthly_budget_cents: Set(req.monthly_budget_cents.unwrap_or(0)),
        retainer_cents: Set(req.retainer_cents.unwrap_or(0)),
        meta_data: Set(object_or_default(req.meta_data, "metaData")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_mutation(&state);
    Ok((StatusCode::CREATED, Json(model)))
}

pub(crate) async fn list_clients(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    let rows = clients::Entity::find()
        .order_by_desc(clients::Column::UpdatedAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    Ok(Json(json!({ "clients": rows })))
}

pub(crate) async fn create_client(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateClientRequest>,
) -> AppResult<(StatusCode, Json<clients::Model>)> {
    require_write_access(&state, &headers, "clients.create").await?;
    let slug = req.slug.unwrap_or_else(|| slugify(&req.name));
    let model = clients::ActiveModel {
        status: Set(req.status.unwrap_or_else(|| "onboarding".to_string())),
        name: Set(req.name),
        slug: Set(slug),
        industry: Set(req.industry),
        website_url: Set(req.website_url),
        billing_email: Set(req.billing_email),
        owner_user_id: Set(req.owner_user_id),
        service_package: Set(req.service_package),
        onboarding_stage: Set(req.onboarding_stage.unwrap_or_else(|| "intake".to_string())),
        portal_enabled: Set(req.portal_enabled.unwrap_or(true)),
        meta_data: Set(object_or_default(req.meta_data, "metaData")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_client_mutation(&state, model.id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

pub(crate) async fn client_dashboard(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    let cache_key = client_dashboard_cache_key(client_id);
    if let Some(cached) = cache_get_json(&state, &cache_key).await {
        return Ok(Json(cached));
    }
    let client = clients::Entity::find_by_id(client_id)
        .one(&state.db)
        .await?
        .ok_or(AppError::NotFound("client"))?;
    let lead_count = leads::Entity::find()
        .filter(leads::Column::ClientId.eq(client_id))
        .count(&state.db)
        .await?;
    let campaign_count = campaigns::Entity::find()
        .filter(campaigns::Column::ClientId.eq(client_id))
        .count(&state.db)
        .await?;
    let opportunity_count = opportunities::Entity::find()
        .filter(opportunities::Column::ClientId.eq(client_id))
        .count(&state.db)
        .await?;
    let open_ticket_count = tickets_count(&state.db, client_id).await?;
    let pending_approval_count = client_approvals::Entity::find()
        .filter(client_approvals::Column::ClientId.eq(client_id))
        .filter(client_approvals::Column::Status.eq("pending"))
        .count(&state.db)
        .await?;
    let outreach_sequence_count = outreach_sequences::Entity::find()
        .filter(outreach_sequences::Column::ClientId.eq(client_id))
        .count(&state.db)
        .await?;
    let research_brief_count = research_briefs::Entity::find()
        .filter(research_briefs::Column::ClientId.eq(client_id))
        .count(&state.db)
        .await?;
    let conversion_event_count = conversion_events::Entity::find()
        .filter(conversion_events::Column::ClientId.eq(client_id))
        .count(&state.db)
        .await?;
    let portal_member_count = portal_members::Entity::find()
        .filter(portal_members::Column::ClientId.eq(client_id))
        .count(&state.db)
        .await?;
    let document_count = shared_documents::Entity::find()
        .filter(shared_documents::Column::ClientId.eq(client_id))
        .count(&state.db)
        .await?;
    let open_comment_count = collaboration_comments::Entity::find()
        .filter(collaboration_comments::Column::ClientId.eq(client_id))
        .filter(collaboration_comments::Column::Status.eq("open"))
        .count(&state.db)
        .await?;
    let queued_notification_count = notifications::Entity::find()
        .filter(notifications::Column::ClientId.eq(client_id))
        .filter(notifications::Column::Status.eq("queued"))
        .count(&state.db)
        .await?;
    let call_insight_count = call_insights::Entity::find()
        .filter(call_insights::Column::ClientId.eq(client_id))
        .count(&state.db)
        .await?;
    let recent_campaigns = campaigns::Entity::find()
        .filter(campaigns::Column::ClientId.eq(client_id))
        .order_by_desc(campaigns::Column::UpdatedAt)
        .limit(8)
        .all(&state.db)
        .await?;
    let recent_reports = reports::Entity::find()
        .filter(reports::Column::ClientId.eq(client_id))
        .order_by_desc(reports::Column::UpdatedAt)
        .limit(5)
        .all(&state.db)
        .await?;
    let open_tasks = project_tasks::Entity::find()
        .filter(project_tasks::Column::ClientId.eq(client_id))
        .filter(project_tasks::Column::Status.is_in(["todo", "in_progress", "blocked"]))
        .order_by_desc(project_tasks::Column::UpdatedAt)
        .limit(10)
        .all(&state.db)
        .await?;
    let recent_conversions = conversion_events::Entity::find()
        .filter(conversion_events::Column::ClientId.eq(client_id))
        .order_by_desc(conversion_events::Column::OccurredAt)
        .limit(8)
        .all(&state.db)
        .await?;
    let recent_documents = shared_documents::Entity::find()
        .filter(shared_documents::Column::ClientId.eq(client_id))
        .order_by_desc(shared_documents::Column::UpdatedAt)
        .limit(8)
        .all(&state.db)
        .await?;
    let recent_comments = collaboration_comments::Entity::find()
        .filter(collaboration_comments::Column::ClientId.eq(client_id))
        .order_by_desc(collaboration_comments::Column::UpdatedAt)
        .limit(8)
        .all(&state.db)
        .await?;
    let recent_call_insights = call_insights::Entity::find()
        .filter(call_insights::Column::ClientId.eq(client_id))
        .order_by_desc(call_insights::Column::AnalyzedAt)
        .limit(5)
        .all(&state.db)
        .await?;
    let payload = json!({
        "client": client,
        "counts": {
            "leads": lead_count,
            "campaigns": campaign_count,
            "opportunities": opportunity_count,
            "openTickets": open_ticket_count,
            "pendingApprovals": pending_approval_count,
            "outreachSequences": outreach_sequence_count,
            "researchBriefs": research_brief_count,
            "conversionEvents": conversion_event_count,
            "portalMembers": portal_member_count,
            "documents": document_count,
            "openComments": open_comment_count,
            "queuedNotifications": queued_notification_count,
            "callInsights": call_insight_count
        },
        "recent": {
            "campaigns": recent_campaigns,
            "reports": recent_reports,
            "openTasks": open_tasks,
            "conversions": recent_conversions,
            "documents": recent_documents,
            "comments": recent_comments,
            "callInsights": recent_call_insights
        }
    });
    cache_set_json(&state, &cache_key, &payload).await;
    Ok(Json(payload))
}

pub(crate) async fn client_operations_summary(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    ensure_client(&state.db, client_id).await?;
    let now = now_fixed();
    let open_task_count = project_tasks::Entity::find()
        .filter(project_tasks::Column::ClientId.eq(client_id))
        .filter(project_tasks::Column::Status.is_in(["todo", "in_progress", "blocked"]))
        .count(&state.db)
        .await?;
    let blocked_task_count = project_tasks::Entity::find()
        .filter(project_tasks::Column::ClientId.eq(client_id))
        .filter(project_tasks::Column::Status.eq("blocked"))
        .count(&state.db)
        .await?;
    let sla_risk_task_count = project_tasks::Entity::find()
        .filter(project_tasks::Column::ClientId.eq(client_id))
        .filter(project_tasks::Column::Status.is_in(["todo", "in_progress", "blocked"]))
        .filter(project_tasks::Column::SlaDueAt.lt(now))
        .count(&state.db)
        .await?;
    let pending_approval_count = client_approvals::Entity::find()
        .filter(client_approvals::Column::ClientId.eq(client_id))
        .filter(client_approvals::Column::Status.eq("pending"))
        .count(&state.db)
        .await?;
    let open_ticket_count = tickets_count(&state.db, client_id).await?;
    let open_comment_count = collaboration_comments::Entity::find()
        .filter(collaboration_comments::Column::ClientId.eq(client_id))
        .filter(collaboration_comments::Column::Status.eq("open"))
        .count(&state.db)
        .await?;
    let queued_notification_count = notifications::Entity::find()
        .filter(notifications::Column::ClientId.eq(client_id))
        .filter(notifications::Column::Status.eq("queued"))
        .count(&state.db)
        .await?;
    let document_review_count = shared_documents::Entity::find()
        .filter(shared_documents::Column::ClientId.eq(client_id))
        .filter(shared_documents::Column::Status.is_in(["draft", "review"]))
        .count(&state.db)
        .await?;
    let recent_tasks = project_tasks::Entity::find()
        .filter(project_tasks::Column::ClientId.eq(client_id))
        .order_by_desc(project_tasks::Column::UpdatedAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    let recent_tickets = tickets::Entity::find()
        .filter(tickets::Column::ClientId.eq(client_id))
        .order_by_desc(tickets::Column::LastActivityAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    let recent_comments = collaboration_comments::Entity::find()
        .filter(collaboration_comments::Column::ClientId.eq(client_id))
        .order_by_desc(collaboration_comments::Column::UpdatedAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    let recent_documents = shared_documents::Entity::find()
        .filter(shared_documents::Column::ClientId.eq(client_id))
        .order_by_desc(shared_documents::Column::UpdatedAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    Ok(Json(json!({
        "clientId": client_id,
        "counts": {
            "openTasks": open_task_count,
            "blockedTasks": blocked_task_count,
            "slaRiskTasks": sla_risk_task_count,
            "pendingApprovals": pending_approval_count,
            "openTickets": open_ticket_count,
            "openComments": open_comment_count,
            "queuedNotifications": queued_notification_count,
            "documentsInReview": document_review_count
        },
        "recent": {
            "tasks": recent_tasks,
            "tickets": recent_tickets,
            "comments": recent_comments,
            "documents": recent_documents
        }
    })))
}

pub(crate) async fn create_contact(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Json(req): Json<CreateContactRequest>,
) -> AppResult<(StatusCode, Json<contacts::Model>)> {
    require_write_access(&state, &headers, "contacts.create").await?;
    ensure_client(&state.db, client_id).await?;
    let model = contacts::ActiveModel {
        client_id: Set(client_id),
        status: Set(req.status.unwrap_or_else(|| "active".to_string())),
        first_name: Set(req.first_name),
        last_name: Set(req.last_name),
        email: Set(req.email),
        phone: Set(req.phone),
        job_title: Set(req.job_title),
        lifecycle_role: Set(req.lifecycle_role.unwrap_or_else(|| "other".to_string())),
        consent_status: Set(req.consent_status.unwrap_or_else(|| "unknown".to_string())),
        meta_data: Set(object_or_default(req.meta_data, "metaData")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

pub(crate) async fn create_contract(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Json(req): Json<CreateContractRequest>,
) -> AppResult<(StatusCode, Json<contracts::Model>)> {
    require_write_access(&state, &headers, "contracts.create").await?;
    ensure_client(&state.db, client_id).await?;
    let model = contracts::ActiveModel {
        client_id: Set(client_id),
        package_id: Set(req.package_id),
        status: Set(req.status.unwrap_or_else(|| "draft".to_string())),
        contract_number: Set(req.contract_number),
        starts_on: Set(req.starts_on),
        ends_on: Set(req.ends_on),
        billing_terms: Set(object_or_default(req.billing_terms, "billingTerms")?),
        total_value_cents: Set(req.total_value_cents.unwrap_or(0)),
        meta_data: Set(object_or_default(req.meta_data, "metaData")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

pub(crate) async fn create_invoice(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Json(req): Json<CreateInvoiceRequest>,
) -> AppResult<(StatusCode, Json<invoices::Model>)> {
    require_write_access(&state, &headers, "invoices.create").await?;
    ensure_client(&state.db, client_id).await?;
    let model = invoices::ActiveModel {
        client_id: Set(client_id),
        contract_id: Set(req.contract_id),
        status: Set(req.status.unwrap_or_else(|| "draft".to_string())),
        invoice_number: Set(req.invoice_number),
        due_on: Set(req.due_on),
        amount_cents: Set(req.amount_cents.unwrap_or(0)),
        line_items: Set(array_or_default(req.line_items, "lineItems")?),
        meta_data: Set(object_or_default(req.meta_data, "metaData")?),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}
