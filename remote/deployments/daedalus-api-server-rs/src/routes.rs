//! JSON handlers over the `daedalus` schema.
//!
//! Ownership rule: every query is filtered by the *verified* operator email
//! from the token. Nothing here accepts an owner from the request body or query
//! string — this database has no RLS, so a missing filter is a data leak, not a
//! degraded experience.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use dd_pg_defs_sea_orm::fab_plans;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, Set,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{error::ServiceError, supabase_auth::Operator, SharedState};

/// Hard ceiling on a single list response, independent of any client input.
const MAX_PAGE_SIZE: u64 = 200;
const DEFAULT_PAGE_SIZE: u64 = 50;

#[derive(Debug, Serialize)]
pub(crate) struct PlanView {
    id: Uuid,
    title: String,
    goal: String,
    process_family: String,
    status: String,
    created_at: String,
    updated_at: String,
}

impl From<fab_plans::Model> for PlanView {
    fn from(model: fab_plans::Model) -> Self {
        Self {
            id: model.id,
            title: model.title,
            goal: model.goal,
            process_family: model.process_family,
            status: model.status,
            created_at: model.created_at.to_rfc3339(),
            updated_at: model.updated_at.to_rfc3339(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreatePlan {
    title: String,
    goal: String,
    #[serde(default = "default_process_family")]
    process_family: String,
}

fn default_process_family() -> String {
    "additive".to_string()
}

/// Mirrors the CHECK constraint in pg-defs. Validating here turns a 500 from a
/// constraint violation into an actionable 400, but the database remains the
/// authority — this list must stay in sync with schema.sql.
const PROCESS_FAMILIES: [&str; 3] = ["additive", "subtractive", "hybrid"];

impl CreatePlan {
    fn validate(&self) -> Result<(), ServiceError> {
        let title = self.title.trim();
        if title.is_empty() || title.len() > 200 {
            return Err(ServiceError::BadRequest(
                "title must be 1–200 bytes".to_string(),
            ));
        }
        let goal = self.goal.trim();
        if goal.is_empty() || goal.len() > 20_000 {
            return Err(ServiceError::BadRequest(
                "goal must be 1–20000 bytes".to_string(),
            ));
        }
        if !PROCESS_FAMILIES.contains(&self.process_family.as_str()) {
            return Err(ServiceError::BadRequest(format!(
                "process_family must be one of {}",
                PROCESS_FAMILIES.join(", ")
            )));
        }
        Ok(())
    }
}

pub(crate) async fn list_plans(
    State(state): State<SharedState>,
    operator: Operator,
) -> Result<impl IntoResponse, ServiceError> {
    let db = state.persistence.connection()?;
    let plans = fab_plans::Entity::find()
        // Ownership filter — see the module doc. Never remove.
        .filter(fab_plans::Column::OwnerEmail.eq(operator.email.as_str()))
        .order_by_desc(fab_plans::Column::CreatedAt)
        .paginate(db, DEFAULT_PAGE_SIZE.min(MAX_PAGE_SIZE))
        .fetch_page(0)
        .await?;
    let body: Vec<PlanView> = plans.into_iter().map(PlanView::from).collect();
    Ok((StatusCode::OK, Json(body)))
}

pub(crate) async fn get_plan(
    State(state): State<SharedState>,
    operator: Operator,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ServiceError> {
    let db = state.persistence.connection()?;
    let plan = fab_plans::Entity::find_by_id(id)
        .filter(fab_plans::Column::OwnerEmail.eq(operator.email.as_str()))
        .one(db)
        .await?
        // 404 rather than 403 for a plan owned by someone else: distinguishing
        // them would confirm the id exists.
        .ok_or(ServiceError::NotFound)?;
    Ok((StatusCode::OK, Json(PlanView::from(plan))))
}

pub(crate) async fn create_plan(
    State(state): State<SharedState>,
    operator: Operator,
    Json(body): Json<CreatePlan>,
) -> Result<impl IntoResponse, ServiceError> {
    body.validate()?;
    let db = state.persistence.connection()?;
    let plan = fab_plans::ActiveModel {
        id: Set(Uuid::new_v4()),
        // Owner comes from the verified token, never from the request body.
        owner_email: Set(operator.email.clone()),
        title: Set(body.title.trim().to_string()),
        goal: Set(body.goal.trim().to_string()),
        process_family: Set(body.process_family),
        status: Set("draft".to_string()),
        ..Default::default()
    }
    .insert(db)
    .await?;
    Ok((StatusCode::CREATED, Json(PlanView::from(plan))))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(title: &str, goal: &str, family: &str) -> CreatePlan {
        CreatePlan {
            title: title.to_string(),
            goal: goal.to_string(),
            process_family: family.to_string(),
        }
    }

    #[test]
    fn valid_input_passes() {
        assert!(plan("bracket", "print a bracket", "additive")
            .validate()
            .is_ok());
    }

    #[test]
    fn empty_and_whitespace_titles_are_rejected() {
        assert!(plan("", "g", "additive").validate().is_err());
        assert!(plan("   ", "g", "additive").validate().is_err());
    }

    #[test]
    fn oversized_fields_are_rejected_before_reaching_postgres() {
        // These bounds mirror the CHECK constraints; exceeding them should be a
        // 400, not a constraint-violation 500.
        assert!(plan(&"x".repeat(201), "g", "additive").validate().is_err());
        assert!(plan("t", &"x".repeat(20_001), "additive")
            .validate()
            .is_err());
        assert!(plan(&"x".repeat(200), &"x".repeat(20_000), "additive")
            .validate()
            .is_ok());
    }

    #[test]
    fn process_family_matches_the_schema_check_constraint() {
        for family in PROCESS_FAMILIES {
            assert!(plan("t", "g", family).validate().is_ok());
        }
        assert!(plan("t", "g", "casting").validate().is_err());
        assert!(plan("t", "g", "ADDITIVE").validate().is_err());
    }

    #[test]
    fn default_process_family_is_additive() {
        let parsed: CreatePlan =
            serde_json::from_str(r#"{"title":"t","goal":"g"}"#).expect("body parses");
        assert_eq!(parsed.process_family, "additive");
        assert!(parsed.validate().is_ok());
    }
}
