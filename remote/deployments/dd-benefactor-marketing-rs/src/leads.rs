use std::sync::atomic::Ordering;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use dd_pg_defs_sea_orm::{
    benefactor_marketing_enrichment_jobs as enrichment_jobs, benefactor_marketing_leads as leads,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder,
    QuerySelect, Set,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::redis_support::{publish_job_event, record_client_mutation};
use crate::shared::{
    array_or_default, ensure_client, limit, now_fixed, object_or_default, require_auth,
    require_write_access, score, ListQuery,
};
use crate::state::{AppError, AppResult, AppState};
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LeadImportRequest {
    client_id: Uuid,
    source_integration_id: Option<Uuid>,
    leads: Vec<LeadDraft>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LeadDraft {
    company_name: String,
    domain: Option<String>,
    contact_name: Option<String>,
    contact_email: Option<String>,
    contact_title: Option<String>,
    country_code: Option<String>,
    lead_score: Option<i32>,
    icp_fit_score: Option<i32>,
    verification_status: Option<String>,
    company_profile: Option<Value>,
    signals: Option<Value>,
    meta_data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EnrichmentJobRequest {
    job_kind: String,
    external_job_id: Option<String>,
    scraper_handoff_url: Option<String>,
    input: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScoreLeadRequest {
    lead_score: Option<i32>,
    icp_fit_score: Option<i32>,
    status: Option<String>,
    verification_status: Option<String>,
    enrichment_status: Option<String>,
    company_profile: Option<Value>,
    signals: Option<Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LeadImportResponse {
    imported: usize,
    leads: Vec<leads::Model>,
}

pub(crate) async fn client_lead_intelligence(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    ensure_client(&state.db, client_id).await?;
    let total_leads = leads::Entity::find()
        .filter(leads::Column::ClientId.eq(client_id))
        .count(&state.db)
        .await?;
    let enrichment_pending = leads::Entity::find()
        .filter(leads::Column::ClientId.eq(client_id))
        .filter(leads::Column::EnrichmentStatus.eq("pending"))
        .count(&state.db)
        .await?;
    let enrichment_running = leads::Entity::find()
        .filter(leads::Column::ClientId.eq(client_id))
        .filter(leads::Column::EnrichmentStatus.eq("running"))
        .count(&state.db)
        .await?;
    let verified_contacts = leads::Entity::find()
        .filter(leads::Column::ClientId.eq(client_id))
        .filter(leads::Column::VerificationStatus.eq("verified"))
        .count(&state.db)
        .await?;
    let high_fit_leads = leads::Entity::find()
        .filter(leads::Column::ClientId.eq(client_id))
        .filter(leads::Column::IcpFitScore.gte(80))
        .count(&state.db)
        .await?;
    let top_leads = leads::Entity::find()
        .filter(leads::Column::ClientId.eq(client_id))
        .order_by_desc(leads::Column::LeadScore)
        .order_by_desc(leads::Column::IcpFitScore)
        .order_by_desc(leads::Column::UpdatedAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    Ok(Json(json!({
        "clientId": client_id,
        "counts": {
            "total": total_leads,
            "enrichmentPending": enrichment_pending,
            "enrichmentRunning": enrichment_running,
            "verifiedContacts": verified_contacts,
            "highFit": high_fit_leads
        },
        "topLeads": top_leads
    })))
}

pub(crate) async fn import_leads(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<LeadImportRequest>,
) -> AppResult<(StatusCode, Json<LeadImportResponse>)> {
    require_write_access(&state, &headers, "leads.import").await?;
    let client_id = req.client_id;
    let source_integration_id = req.source_integration_id;
    ensure_client(&state.db, client_id).await?;
    if req.leads.is_empty() {
        return Err(AppError::BadRequest(
            "leads must contain at least one item".to_string(),
        ));
    }
    if req.leads.len() > 500 {
        return Err(AppError::BadRequest(
            "lead import is limited to 500 records".to_string(),
        ));
    }
    let mut inserted = Vec::with_capacity(req.leads.len());
    for draft in req.leads {
        let model = leads::ActiveModel {
            client_id: Set(client_id),
            source_integration_id: Set(source_integration_id),
            status: Set("new".to_string()),
            company_name: Set(draft.company_name),
            domain: Set(draft.domain),
            contact_name: Set(draft.contact_name),
            contact_email: Set(draft.contact_email),
            contact_title: Set(draft.contact_title),
            country_code: Set(draft.country_code),
            lead_score: Set(score(draft.lead_score.unwrap_or(0))?),
            icp_fit_score: Set(score(draft.icp_fit_score.unwrap_or(0))?),
            verification_status: Set(draft
                .verification_status
                .unwrap_or_else(|| "unknown".to_string())),
            enrichment_status: Set("pending".to_string()),
            company_profile: Set(object_or_default(draft.company_profile, "companyProfile")?),
            signals: Set(array_or_default(draft.signals, "signals")?),
            meta_data: Set(object_or_default(draft.meta_data, "metaData")?),
            ..Default::default()
        }
        .insert(&state.db)
        .await?;
        inserted.push(model);
    }
    state
        .metrics
        .lead_imports_total
        .fetch_add(1, Ordering::Relaxed);
    publish_job_event(
        &state,
        "lead_import_batch",
        json!({
            "clientId": client_id,
            "imported": inserted.len(),
            "leadIds": inserted.iter().map(|lead| lead.id).collect::<Vec<_>>()
        }),
    )
    .await;
    record_client_mutation(&state, client_id).await;
    Ok((
        StatusCode::CREATED,
        Json(LeadImportResponse {
            imported: inserted.len(),
            leads: inserted,
        }),
    ))
}

pub(crate) async fn list_client_leads(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> AppResult<Json<Value>> {
    require_auth(&state, &headers)?;
    ensure_client(&state.db, client_id).await?;
    let rows = leads::Entity::find()
        .filter(leads::Column::ClientId.eq(client_id))
        .order_by_desc(leads::Column::LeadScore)
        .order_by_desc(leads::Column::UpdatedAt)
        .limit(limit(query.limit))
        .all(&state.db)
        .await?;
    Ok(Json(json!({ "leads": rows })))
}

pub(crate) async fn queue_enrichment_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(lead_id): Path<Uuid>,
    Json(req): Json<EnrichmentJobRequest>,
) -> AppResult<(StatusCode, Json<enrichment_jobs::Model>)> {
    require_write_access(&state, &headers, "leads.enrichment.queue").await?;
    let lead = leads::Entity::find_by_id(lead_id)
        .one(&state.db)
        .await?
        .ok_or(AppError::NotFound("lead"))?;
    let client_id = lead.client_id;
    let lead_id = lead.id;
    let job_id = Uuid::new_v4();
    let handoff_url = req.scraper_handoff_url.or_else(|| {
        state
            .cfg
            .scraper_base_url
            .as_ref()
            .map(|base| format!("{}/jobs/{}", base.trim_end_matches('/'), job_id))
    });
    let model = enrichment_jobs::ActiveModel {
        id: Set(job_id),
        client_id: Set(client_id),
        lead_id: Set(Some(lead_id)),
        job_kind: Set(req.job_kind),
        status: Set("queued".to_string()),
        external_job_id: Set(req.external_job_id),
        scraper_handoff_url: Set(handoff_url),
        input: Set(object_or_default(req.input, "input")?),
        result: Set(json!({})),
        ..Default::default()
    }
    .insert(&state.db)
    .await?;
    let mut active_lead: leads::ActiveModel = lead.into();
    active_lead.enrichment_status = Set("running".to_string());
    active_lead.updated_at = Set(now_fixed());
    active_lead.update(&state.db).await?;
    state
        .metrics
        .enrichment_jobs_total
        .fetch_add(1, Ordering::Relaxed);
    publish_job_event(
        &state,
        "lead_enrichment_queued",
        json!({
            "clientId": client_id,
            "leadId": lead_id,
            "jobId": model.id,
            "jobKind": &model.job_kind,
            "scraperHandoffUrl": &model.scraper_handoff_url
        }),
    )
    .await;
    record_client_mutation(&state, client_id).await;
    Ok((StatusCode::CREATED, Json(model)))
}

pub(crate) async fn score_lead(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(lead_id): Path<Uuid>,
    Json(req): Json<ScoreLeadRequest>,
) -> AppResult<Json<leads::Model>> {
    require_write_access(&state, &headers, "leads.score").await?;
    let lead = leads::Entity::find_by_id(lead_id)
        .one(&state.db)
        .await?
        .ok_or(AppError::NotFound("lead"))?;
    let client_id = lead.client_id;
    let mut active: leads::ActiveModel = lead.into();
    if let Some(value) = req.lead_score {
        active.lead_score = Set(score(value)?);
    }
    if let Some(value) = req.icp_fit_score {
        active.icp_fit_score = Set(score(value)?);
    }
    if let Some(value) = req.status {
        active.status = Set(value);
    }
    if let Some(value) = req.verification_status {
        active.verification_status = Set(value);
    }
    if let Some(value) = req.enrichment_status {
        active.enrichment_status = Set(value);
    }
    if let Some(value) = req.company_profile {
        active.company_profile = Set(object_or_default(Some(value), "companyProfile")?);
    }
    if let Some(value) = req.signals {
        active.signals = Set(array_or_default(Some(value), "signals")?);
    }
    active.updated_at = Set(now_fixed());
    let model = active.update(&state.db).await?;
    record_client_mutation(&state, client_id).await;
    Ok(Json(model))
}
