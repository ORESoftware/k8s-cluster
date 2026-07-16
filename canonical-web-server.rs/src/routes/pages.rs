use crate::{
    auth::{require_csrf, require_origin, SessionAuthenticated},
    db::{
        begin_user_transaction,
        entity::{audit_engagement, engagement_note},
    },
    error::AppError,
    views, AppState,
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Form, Router,
};
use chrono::{NaiveDate, Utc};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter,
    QueryOrder,
};
use serde::Deserialize;
use uuid::Uuid;

const COMPANY_MAX_CHARS: usize = 200;
const NOTE_MAX_CHARS: usize = 4000;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(dashboard))
        .route("/fragments/session", get(session_fragment))
        .route("/engagements", get(engagements).post(create_engagement))
        .route("/engagements/{id}", get(engagement_detail))
        .route("/engagements/{id}/status", post(update_engagement_status))
        .route("/engagements/{id}/notes", post(add_engagement_note))
        .fallback(not_found)
}

async fn dashboard(auth: Result<SessionAuthenticated, AppError>) -> Response {
    match auth {
        Ok(SessionAuthenticated(actor)) => views::dashboard(&actor).into_response(),
        Err(AppError::Unauthorized) => Redirect::to("/login").into_response(),
        Err(error) => error.into_response(),
    }
}

async fn session_fragment(auth: Result<SessionAuthenticated, AppError>) -> Response {
    match auth {
        Ok(SessionAuthenticated(actor)) => views::session_fragment(&actor).into_response(),
        Err(AppError::Unauthorized) => {
            let mut response = StatusCode::UNAUTHORIZED.into_response();
            response.headers_mut().insert(
                "hx-redirect",
                axum::http::HeaderValue::from_static("/login"),
            );
            response
        }
        Err(error) => error.into_response(),
    }
}

async fn not_found() -> Response {
    (StatusCode::NOT_FOUND, views::html_not_found()).into_response()
}

/// HTMX-aware validation failure: retarget the error fragment at the form's
/// error slot (mirrors the login fragment pattern); plain form posts get a 422
/// page-level error instead.
fn form_error(headers: &HeaderMap, slot: &'static str, message: &str) -> Response {
    if headers.contains_key("hx-request") {
        let mut response = (StatusCode::OK, views::engagement_form_error(message)).into_response();
        response
            .headers_mut()
            .insert("hx-retarget", HeaderValue::from_static(slot));
        response
    } else {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            views::engagement_form_error(message),
        )
            .into_response()
    }
}

async fn owned_engagement<C>(
    connection: &C,
    owner_id: Uuid,
    raw_id: &str,
) -> Result<audit_engagement::Model, AppError>
where
    C: ConnectionTrait,
{
    // A malformed id and someone else's engagement are indistinguishable: 404.
    let id = Uuid::parse_str(raw_id).map_err(|_| AppError::NotFound)?;
    audit_engagement::Entity::find_by_id(id)
        .filter(audit_engagement::Column::OwnerId.eq(owner_id))
        .one(connection)
        .await?
        .ok_or(AppError::NotFound)
}

async fn list_engagements<C>(
    connection: &C,
    owner_id: Uuid,
) -> Result<Vec<audit_engagement::Model>, AppError>
where
    C: ConnectionTrait,
{
    Ok(audit_engagement::Entity::find()
        .filter(audit_engagement::Column::OwnerId.eq(owner_id))
        .order_by_desc(audit_engagement::Column::OpenedAt)
        .all(connection)
        .await?)
}

async fn engagements(
    State(state): State<AppState>,
    auth: Result<SessionAuthenticated, AppError>,
) -> Response {
    let actor = match auth {
        Ok(SessionAuthenticated(actor)) => actor,
        Err(AppError::Unauthorized) => return Redirect::to("/login").into_response(),
        Err(error) => return error.into_response(),
    };
    let result: Result<Vec<audit_engagement::Model>, AppError> = async {
        let transaction = begin_user_transaction(&state.db, actor.user_id).await?;
        let engagements = list_engagements(&transaction, actor.user_id).await?;
        transaction.commit().await?;
        Ok(engagements)
    }
    .await;
    match result {
        Ok(engagements) => views::engagements_page(&actor, &engagements).into_response(),
        Err(error) => error.into_response(),
    }
}

#[derive(Deserialize)]
struct CreateEngagementForm {
    csrf: String,
    company: String,
    framework: String,
    #[serde(default)]
    target_report_date: String,
}

async fn create_engagement(
    State(state): State<AppState>,
    headers: HeaderMap,
    SessionAuthenticated(actor): SessionAuthenticated,
    Form(form): Form<CreateEngagementForm>,
) -> Result<Response, AppError> {
    require_origin(&headers, &state)?;
    require_csrf(&actor, &headers, Some(&form.csrf))?;

    let company = form.company.trim();
    if company.is_empty() || company.chars().count() > COMPANY_MAX_CHARS {
        return Ok(form_error(
            &headers,
            "#engagement-form-error",
            "Company is required and must be at most 200 characters.",
        ));
    }
    if !audit_engagement::FRAMEWORKS.contains(&form.framework.as_str()) {
        return Ok(form_error(
            &headers,
            "#engagement-form-error",
            "Choose a supported compliance framework.",
        ));
    }
    let target_report_date = match form.target_report_date.trim() {
        "" => None,
        raw => match NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
            Ok(date) => Some(date),
            Err(_) => {
                return Ok(form_error(
                    &headers,
                    "#engagement-form-error",
                    "Target report date must be a valid YYYY-MM-DD date.",
                ))
            }
        },
    };

    let now = Utc::now();
    let transaction = begin_user_transaction(&state.db, actor.user_id).await?;
    audit_engagement::ActiveModel {
        id: Set(Uuid::new_v4()),
        owner_id: Set(actor.user_id),
        company: Set(company.to_string()),
        framework: Set(form.framework),
        status: Set("scoping".to_string()),
        opened_at: Set(now),
        target_report_date: Set(target_report_date),
        updated_at: Set(now),
    }
    .insert(&transaction)
    .await?;

    let htmx = headers.contains_key("hx-request");
    let engagements = if htmx {
        Some(list_engagements(&transaction, actor.user_id).await?)
    } else {
        None
    };
    transaction.commit().await?;

    if let Some(engagements) = engagements {
        Ok(views::engagement_list(&engagements).into_response())
    } else {
        Ok(Redirect::to("/app/engagements").into_response())
    }
}

async fn engagement_detail(
    State(state): State<AppState>,
    auth: Result<SessionAuthenticated, AppError>,
    Path(raw_id): Path<String>,
) -> Response {
    let actor = match auth {
        Ok(SessionAuthenticated(actor)) => actor,
        Err(AppError::Unauthorized) => return Redirect::to("/login").into_response(),
        Err(error) => return error.into_response(),
    };
    let result: Result<_, AppError> = async {
        let transaction = begin_user_transaction(&state.db, actor.user_id).await?;
        let engagement = owned_engagement(&transaction, actor.user_id, &raw_id).await?;
        let notes = engagement_note::Entity::find()
            .filter(engagement_note::Column::EngagementId.eq(engagement.id))
            .filter(engagement_note::Column::OwnerId.eq(actor.user_id))
            .order_by_desc(engagement_note::Column::CreatedAt)
            .all(&transaction)
            .await?;
        transaction.commit().await?;
        Ok((engagement, notes))
    }
    .await;
    let (engagement, notes) = match result {
        Ok(result) => result,
        Err(AppError::NotFound) => return not_found().await,
        Err(error) => return error.into_response(),
    };
    views::engagement_detail_page(&actor, &engagement, &notes).into_response()
}

#[derive(Deserialize)]
struct UpdateStatusForm {
    csrf: String,
    status: String,
}

async fn update_engagement_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    SessionAuthenticated(actor): SessionAuthenticated,
    Path(raw_id): Path<String>,
    Form(form): Form<UpdateStatusForm>,
) -> Result<Response, AppError> {
    require_origin(&headers, &state)?;
    require_csrf(&actor, &headers, Some(&form.csrf))?;

    let transaction = begin_user_transaction(&state.db, actor.user_id).await?;
    let engagement = owned_engagement(&transaction, actor.user_id, &raw_id).await?;
    if !audit_engagement::STATUSES.contains(&form.status.as_str()) {
        transaction.rollback().await?;
        return Ok(form_error(
            &headers,
            "#engagement-status",
            "Choose a supported engagement status.",
        ));
    }

    let mut active: audit_engagement::ActiveModel = engagement.into();
    active.status = Set(form.status);
    active.updated_at = Set(Utc::now());
    let engagement = active.update(&transaction).await?;
    transaction.commit().await?;

    if headers.contains_key("hx-request") {
        let csrf = actor.csrf_token.as_deref().unwrap_or_default();
        Ok(views::engagement_status(&engagement, csrf).into_response())
    } else {
        Ok(Redirect::to(&format!("/app/engagements/{}", engagement.id)).into_response())
    }
}

#[derive(Deserialize)]
struct AddNoteForm {
    csrf: String,
    body: String,
}

async fn add_engagement_note(
    State(state): State<AppState>,
    headers: HeaderMap,
    SessionAuthenticated(actor): SessionAuthenticated,
    Path(raw_id): Path<String>,
    Form(form): Form<AddNoteForm>,
) -> Result<Response, AppError> {
    require_origin(&headers, &state)?;
    require_csrf(&actor, &headers, Some(&form.csrf))?;

    let transaction = begin_user_transaction(&state.db, actor.user_id).await?;
    let engagement = owned_engagement(&transaction, actor.user_id, &raw_id).await?;
    let body = form.body.trim();
    if body.is_empty() || body.chars().count() > NOTE_MAX_CHARS {
        transaction.rollback().await?;
        return Ok(form_error(
            &headers,
            "#note-form-error",
            "A note is required and must be at most 4000 characters.",
        ));
    }

    engagement_note::ActiveModel {
        id: Set(Uuid::new_v4()),
        engagement_id: Set(engagement.id),
        owner_id: Set(actor.user_id),
        body: Set(body.to_string()),
        created_at: Set(Utc::now()),
    }
    .insert(&transaction)
    .await?;

    let htmx = headers.contains_key("hx-request");
    let notes = if htmx {
        Some(
            engagement_note::Entity::find()
                .filter(engagement_note::Column::EngagementId.eq(engagement.id))
                .filter(engagement_note::Column::OwnerId.eq(actor.user_id))
                .order_by_desc(engagement_note::Column::CreatedAt)
                .all(&transaction)
                .await?,
        )
    } else {
        None
    };
    transaction.commit().await?;

    if let Some(notes) = notes {
        Ok(views::engagement_notes(&notes).into_response())
    } else {
        Ok(Redirect::to(&format!("/app/engagements/{}", engagement.id)).into_response())
    }
}
