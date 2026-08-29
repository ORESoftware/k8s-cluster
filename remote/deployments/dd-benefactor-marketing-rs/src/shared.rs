use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    sync::atomic::Ordering,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::http::{header, HeaderMap};
use chrono::{DateTime, FixedOffset, Utc};
use dd_pg_defs_sea_orm::{
    benefactor_marketing_campaigns as campaigns, benefactor_marketing_clients as clients,
    benefactor_marketing_collaboration_comments as collaboration_comments,
    benefactor_marketing_contacts as contacts,
    benefactor_marketing_content_assets as content_assets,
    benefactor_marketing_integrations as integrations, benefactor_marketing_leads as leads,
    benefactor_marketing_meetings as meetings, benefactor_marketing_opportunities as opportunities,
    benefactor_marketing_outreach_enrollments as outreach_enrollments,
    benefactor_marketing_outreach_sequences as outreach_sequences,
    benefactor_marketing_project_tasks as project_tasks, benefactor_marketing_tickets as tickets,
};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::redis_support::redis_incr_with_ttl;
use crate::state::{AppError, AppResult, AppState, DEFAULT_LIMIT, MAX_LIMIT};
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ListQuery {
    pub(crate) limit: Option<u64>,
}

pub(crate) async fn ensure_client(db: &DatabaseConnection, client_id: Uuid) -> AppResult<clients::Model> {
    clients::Entity::find_by_id(client_id)
        .one(db)
        .await?
        .ok_or(AppError::NotFound("client"))
}

pub(crate) async fn ensure_campaign(
    db: &DatabaseConnection,
    campaign_id: Uuid,
) -> AppResult<campaigns::Model> {
    campaigns::Entity::find_by_id(campaign_id)
        .one(db)
        .await?
        .ok_or(AppError::NotFound("campaign"))
}

pub(crate) async fn ensure_integration(
    db: &DatabaseConnection,
    integration_id: Uuid,
) -> AppResult<integrations::Model> {
    integrations::Entity::find_by_id(integration_id)
        .one(db)
        .await?
        .ok_or(AppError::NotFound("integration"))
}

pub(crate) async fn ensure_outreach_sequence(
    db: &DatabaseConnection,
    sequence_id: Uuid,
) -> AppResult<outreach_sequences::Model> {
    outreach_sequences::Entity::find_by_id(sequence_id)
        .one(db)
        .await?
        .ok_or(AppError::NotFound("outreach sequence"))
}

pub(crate) async fn ensure_meeting(db: &DatabaseConnection, meeting_id: Uuid) -> AppResult<meetings::Model> {
    meetings::Entity::find_by_id(meeting_id)
        .one(db)
        .await?
        .ok_or(AppError::NotFound("meeting"))
}

pub(crate) async fn ensure_optional_campaign_for_client(
    db: &DatabaseConnection,
    client_id: Uuid,
    campaign_id: Option<Uuid>,
) -> AppResult<()> {
    let Some(campaign_id) = campaign_id else {
        return Ok(());
    };
    let campaign = ensure_campaign(db, campaign_id).await?;
    if campaign.client_id != client_id {
        return Err(AppError::BadRequest(
            "campaignId must belong to the request client".to_string(),
        ));
    }
    Ok(())
}

pub(crate) async fn ensure_optional_lead(
    db: &DatabaseConnection,
    client_id: Uuid,
    lead_id: Option<Uuid>,
) -> AppResult<()> {
    let Some(lead_id) = lead_id else {
        return Ok(());
    };
    let lead = leads::Entity::find_by_id(lead_id)
        .one(db)
        .await?
        .ok_or(AppError::NotFound("lead"))?;
    if lead.client_id != client_id {
        return Err(AppError::BadRequest(
            "leadId must belong to the request client".to_string(),
        ));
    }
    Ok(())
}

pub(crate) async fn ensure_optional_opportunity(
    db: &DatabaseConnection,
    client_id: Uuid,
    opportunity_id: Option<Uuid>,
) -> AppResult<()> {
    let Some(opportunity_id) = opportunity_id else {
        return Ok(());
    };
    let opportunity = opportunities::Entity::find_by_id(opportunity_id)
        .one(db)
        .await?
        .ok_or(AppError::NotFound("opportunity"))?;
    if opportunity.client_id != client_id {
        return Err(AppError::BadRequest(
            "opportunityId must belong to the request client".to_string(),
        ));
    }
    Ok(())
}

pub(crate) async fn ensure_optional_project_task(
    db: &DatabaseConnection,
    client_id: Uuid,
    project_task_id: Option<Uuid>,
) -> AppResult<()> {
    let Some(project_task_id) = project_task_id else {
        return Ok(());
    };
    let task = project_tasks::Entity::find_by_id(project_task_id)
        .one(db)
        .await?
        .ok_or(AppError::NotFound("project task"))?;
    if task.client_id != client_id {
        return Err(AppError::BadRequest(
            "projectTaskId must belong to the request client".to_string(),
        ));
    }
    Ok(())
}

pub(crate) async fn ensure_optional_meeting_for_client(
    db: &DatabaseConnection,
    client_id: Uuid,
    meeting_id: Option<Uuid>,
) -> AppResult<()> {
    let Some(meeting_id) = meeting_id else {
        return Ok(());
    };
    let meeting = ensure_meeting(db, meeting_id).await?;
    if meeting.client_id != client_id {
        return Err(AppError::BadRequest(
            "meetingId must belong to the request client".to_string(),
        ));
    }
    Ok(())
}

pub(crate) async fn ensure_optional_parent_comment(
    db: &DatabaseConnection,
    client_id: Uuid,
    parent_comment_id: Option<Uuid>,
) -> AppResult<()> {
    let Some(parent_comment_id) = parent_comment_id else {
        return Ok(());
    };
    let comment = collaboration_comments::Entity::find_by_id(parent_comment_id)
        .one(db)
        .await?
        .ok_or(AppError::NotFound("parent comment"))?;
    if comment.client_id != client_id {
        return Err(AppError::BadRequest(
            "parentCommentId must belong to the request client".to_string(),
        ));
    }
    Ok(())
}

pub(crate) async fn ensure_optional_contact(
    db: &DatabaseConnection,
    client_id: Uuid,
    contact_id: Option<Uuid>,
) -> AppResult<()> {
    let Some(contact_id) = contact_id else {
        return Ok(());
    };
    let contact = contacts::Entity::find_by_id(contact_id)
        .one(db)
        .await?
        .ok_or(AppError::NotFound("contact"))?;
    if contact.client_id != client_id {
        return Err(AppError::BadRequest(
            "contactId must belong to the request client".to_string(),
        ));
    }
    Ok(())
}

pub(crate) async fn ensure_optional_content_asset(
    db: &DatabaseConnection,
    client_id: Uuid,
    content_asset_id: Option<Uuid>,
) -> AppResult<()> {
    let Some(content_asset_id) = content_asset_id else {
        return Ok(());
    };
    let asset = content_assets::Entity::find_by_id(content_asset_id)
        .one(db)
        .await?
        .ok_or(AppError::NotFound("content asset"))?;
    if asset.client_id != client_id {
        return Err(AppError::BadRequest(
            "contentAssetId must belong to the request client".to_string(),
        ));
    }
    Ok(())
}

pub(crate) async fn ensure_optional_enrollment(
    db: &DatabaseConnection,
    client_id: Uuid,
    enrollment_id: Option<Uuid>,
) -> AppResult<()> {
    let Some(enrollment_id) = enrollment_id else {
        return Ok(());
    };
    let enrollment = outreach_enrollments::Entity::find_by_id(enrollment_id)
        .one(db)
        .await?
        .ok_or(AppError::NotFound("outreach enrollment"))?;
    if enrollment.client_id != client_id {
        return Err(AppError::BadRequest(
            "enrollmentId must belong to the request client".to_string(),
        ));
    }
    Ok(())
}

pub(crate) async fn tickets_count(db: &DatabaseConnection, client_id: Uuid) -> AppResult<u64> {
    Ok(tickets::Entity::find()
        .filter(tickets::Column::ClientId.eq(client_id))
        .filter(tickets::Column::Status.is_in(["open", "pending_client", "pending_agency"]))
        .count(db)
        .await?)
}

pub(crate) fn require_auth(state: &AppState, headers: &HeaderMap) -> AppResult<()> {
    if state.cfg.allow_unauthenticated {
        return Ok(());
    }
    let Some(expected) = state.cfg.api_auth_bearer.as_deref() else {
        state
            .metrics
            .auth_failures_total
            .fetch_add(1, Ordering::Relaxed);
        return Err(AppError::Unauthorized);
    };
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let legacy_auth = headers
        .get("Auth")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let bearer = authorization
        .strip_prefix("Bearer ")
        .map(str::trim)
        .unwrap_or(authorization);
    if bearer == expected || legacy_auth == expected {
        return Ok(());
    }
    state
        .metrics
        .auth_failures_total
        .fetch_add(1, Ordering::Relaxed);
    Err(AppError::Unauthorized)
}

pub(crate) async fn require_write_access(
    state: &AppState,
    headers: &HeaderMap,
    action: &'static str,
) -> AppResult<()> {
    require_auth(state, headers)?;
    enforce_rate_limit(state, headers, action).await
}

async fn enforce_rate_limit(
    state: &AppState,
    headers: &HeaderMap,
    action: &'static str,
) -> AppResult<()> {
    if state.cfg.rate_limit_per_minute == 0 || state.redis.is_none() {
        return Ok(());
    }

    let key = format!(
        "benefactor:marketing:rate:{action}:{}",
        auth_actor_hash(headers)
    );
    let Some(count) = redis_incr_with_ttl(state, &key, 60).await else {
        return Ok(());
    };
    if count > state.cfg.rate_limit_per_minute as i64 {
        state
            .metrics
            .rate_limit_rejections_total
            .fetch_add(1, Ordering::Relaxed);
        return Err(AppError::RateLimited);
    }
    Ok(())
}

fn auth_actor_hash(headers: &HeaderMap) -> String {
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let legacy_auth = headers
        .get("Auth")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let actor = if authorization.is_empty() {
        legacy_auth
    } else {
        authorization
    };
    let mut hasher = DefaultHasher::new();
    actor.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

pub(crate) fn object_or_default(value: Option<Value>, field: &str) -> AppResult<Value> {
    match value {
        Some(value) if value.is_object() => Ok(value),
        Some(_) => Err(AppError::BadRequest(format!(
            "{field} must be a JSON object"
        ))),
        None => Ok(json!({})),
    }
}

pub(crate) fn array_or_default(value: Option<Value>, field: &str) -> AppResult<Value> {
    match value {
        Some(value) if value.is_array() => Ok(value),
        Some(_) => Err(AppError::BadRequest(format!(
            "{field} must be a JSON array"
        ))),
        None => Ok(json!([])),
    }
}

pub(crate) fn limit(value: Option<u64>) -> u64 {
    value.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

pub(crate) fn score(value: i32) -> AppResult<i32> {
    if (0..=100).contains(&value) {
        Ok(value)
    } else {
        Err(AppError::BadRequest(
            "scores must be between 0 and 100".to_string(),
        ))
    }
}

pub(crate) fn probability(value: i32) -> AppResult<i32> {
    if (0..=1_000_000).contains(&value) {
        Ok(value)
    } else {
        Err(AppError::BadRequest(
            "probabilityMicros must be between 0 and 1000000".to_string(),
        ))
    }
}

pub(crate) fn percent(value: i32) -> AppResult<i32> {
    if (0..=100).contains(&value) {
        Ok(value)
    } else {
        Err(AppError::BadRequest(
            "allocationPercent must be between 0 and 100".to_string(),
        ))
    }
}

pub(crate) fn step_order(value: i32) -> AppResult<i32> {
    if (1..=100).contains(&value) {
        Ok(value)
    } else {
        Err(AppError::BadRequest(
            "step order must be between 1 and 100".to_string(),
        ))
    }
}

pub(crate) fn non_negative(value: i32, field: &str) -> AppResult<i32> {
    if value >= 0 {
        Ok(value)
    } else {
        Err(AppError::BadRequest(format!(
            "{field} must be non-negative"
        )))
    }
}

pub(crate) fn minutes_in_day(value: i32) -> AppResult<i32> {
    if (1..=1440).contains(&value) {
        Ok(value)
    } else {
        Err(AppError::BadRequest(
            "minutes must be between 1 and 1440".to_string(),
        ))
    }
}

pub(crate) fn iso_date(value: String, field: &str) -> AppResult<String> {
    let bytes = value.as_bytes();
    let valid = bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(idx, byte)| idx == 4 || idx == 7 || byte.is_ascii_digit());
    if valid {
        Ok(value)
    } else {
        Err(AppError::BadRequest(format!(
            "{field} must use YYYY-MM-DD format"
        )))
    }
}

pub(crate) fn optional_iso_date(value: Option<String>, field: &str) -> AppResult<Option<String>> {
    value.map(|value| iso_date(value, field)).transpose()
}

pub(crate) fn computed_commission_amount(basis_cents: i32, rate_micros: i32) -> AppResult<i32> {
    let amount = i64::from(basis_cents) * i64::from(rate_micros) / 1_000_000;
    if amount <= i64::from(i32::MAX) {
        Ok(amount as i32)
    } else {
        Err(AppError::BadRequest(
            "computed commission amount exceeds supported range".to_string(),
        ))
    }
}

pub(crate) fn now_fixed() -> DateTime<FixedOffset> {
    Utc::now().fixed_offset()
}

pub(crate) fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

pub(crate) fn slugify(value: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
        if out.len() >= 220 {
            break;
        }
    }
    let mut out = out.trim_matches('-').to_string();
    if out.is_empty() {
        out = "client".to_string();
    }
    while out.len() < 3 {
        out.push('x');
    }
    out
}
