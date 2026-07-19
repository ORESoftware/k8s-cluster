//! Read-only HTML routes over the `daedalus` schema.
//!
//! Same ownership rule as the API server: every query filters on the *verified*
//! operator email. This process only ever reads — writes go through
//! daedalus-api-server — but the auth boundary is identical, because the
//! database has no RLS to fall back on.

use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
};
use dd_pg_defs_sea_orm::{fab_instructions, fab_plans, fab_runs};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use uuid::Uuid;

use crate::{error::ServiceError, supabase_auth::Operator, views, SharedState};

const MAX_PLANS: u64 = 200;

pub(crate) async fn index(
    State(state): State<SharedState>,
    operator: Operator,
) -> Result<Html<String>, ServiceError> {
    let db = state.persistence.connection()?;
    let plans = fab_plans::Entity::find()
        .filter(fab_plans::Column::OwnerEmail.eq(operator.email.as_str()))
        .order_by_desc(fab_plans::Column::CreatedAt)
        .limit(MAX_PLANS)
        .all(db)
        .await?;
    state.metrics.record_page();
    Ok(Html(
        views::page("Plans", &operator.email, views::plan_list(&plans)).into_string(),
    ))
}

pub(crate) async fn plan_detail(
    State(state): State<SharedState>,
    operator: Operator,
    Path(id): Path<Uuid>,
) -> Result<Html<String>, ServiceError> {
    let db = state.persistence.connection()?;
    let plan = owned_plan(db, id, &operator).await?;
    let runs = runs_for_plan(db, id).await?;
    state.metrics.record_page();
    Ok(Html(
        views::page(
            &plan.title,
            &operator.email,
            views::plan_detail(&plan, &runs),
        )
        .into_string(),
    ))
}

/// htmx fragment: just the runs table, for a non-websocket refresh (fallback
/// when the ws is unavailable, and the initial hx-get some clients issue).
pub(crate) async fn plan_runs_fragment(
    State(state): State<SharedState>,
    operator: Operator,
    Path(id): Path<Uuid>,
) -> Result<Html<String>, ServiceError> {
    let db = state.persistence.connection()?;
    // Ownership check still applies to a fragment request.
    owned_plan(db, id, &operator).await?;
    let runs = runs_for_plan(db, id).await?;
    state.metrics.record_page();
    Ok(Html(views::runs_fragment(&runs).into_string()))
}

/// Fetch a plan only if it belongs to the operator; 404 otherwise (never 403,
/// which would confirm the id exists).
pub(crate) async fn owned_plan(
    db: &sea_orm::DatabaseConnection,
    id: Uuid,
    operator: &Operator,
) -> Result<fab_plans::Model, ServiceError> {
    fab_plans::Entity::find_by_id(id)
        .filter(fab_plans::Column::OwnerEmail.eq(operator.email.as_str()))
        .one(db)
        .await?
        .ok_or(ServiceError::NotFound)
}

/// Runs belonging to a plan, newest first. Joined through the plan's released
/// instruction sets so runs stay scoped to the (already ownership-checked) plan.
pub(crate) async fn runs_for_plan(
    db: &sea_orm::DatabaseConnection,
    plan_id: Uuid,
) -> Result<Vec<fab_runs::Model>, ServiceError> {
    let instruction_ids: Vec<Uuid> = fab_instructions::Entity::find()
        .filter(fab_instructions::Column::PlanId.eq(plan_id))
        .all(db)
        .await?
        .into_iter()
        .map(|instruction| instruction.id)
        .collect();
    if instruction_ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(fab_runs::Entity::find()
        .filter(fab_runs::Column::InstructionsId.is_in(instruction_ids))
        .order_by_desc(fab_runs::Column::CreatedAt)
        .all(db)
        .await?)
}

pub(crate) async fn ready(State(state): State<SharedState>) -> impl IntoResponse {
    if state.persistence.is_enabled() {
        (StatusCode::OK, "ready")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "database disabled")
    }
}

pub(crate) async fn metrics(State(state): State<SharedState>) -> impl IntoResponse {
    (StatusCode::OK, state.metrics.encode())
}

/// Serve the two pinned htmx assets from the binary. Embedding them keeps the
/// Content-Security-Policy `self`-only (no third-party script origin) and means
/// the UI has no runtime CDN dependency.
pub(crate) async fn asset(Path(name): Path<String>) -> Response {
    let body: Option<&'static str> = match name.as_str() {
        n if n == format!("htmx-{}.min.js", views::HTMX_VERSION) => {
            Some(include_str!("../assets/htmx.min.js"))
        }
        n if n == format!("htmx-ws-{}.min.js", views::HTMX_VERSION) => {
            Some(include_str!("../assets/htmx-ws.min.js"))
        }
        _ => None,
    };
    match body {
        Some(js) => (
            StatusCode::OK,
            [(
                header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            )],
            js,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn asset_names_must_match_the_pinned_version() {
        // A version bump in views.rs without shipping the matching asset file
        // would 404 the script and break the whole UI; assert the coupling.
        let good = format!("htmx-{}.min.js", views::HTMX_VERSION);
        assert!(matches!(asset(Path(good)).await.status(), StatusCode::OK));
        assert_eq!(
            asset(Path("htmx-0.0.0.min.js".to_string())).await.status(),
            StatusCode::NOT_FOUND
        );
    }
}
