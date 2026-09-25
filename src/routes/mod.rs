use std::{sync::atomic::Ordering, time::Instant};

use axum::{
    extract::{Path, Query, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use dd_pg_defs::{
    validate_usacc_case_stages_insert, validate_usacc_cases_insert,
    validate_usacc_elections_insert, validate_usacc_ledger_entries_insert,
    validate_usacc_users_insert, validate_usacc_votes_insert, UsaccCaseStagesInsert,
    UsaccCaseStagesRow, UsaccCasesInsert, UsaccCasesRow, UsaccElectionsInsert, UsaccElectionsRow,
    UsaccLedgerEntriesInsert, UsaccLedgerEntriesRow, UsaccSimulationRunsRow, UsaccUsersInsert,
    UsaccUsersRow, UsaccVotesInsert, UsaccVotesRow, USACC_SIMULATION_RUNS_TABLE,
};
use dd_pg_defs_sea_orm::{
    usacc_case_stages, usacc_cases, usacc_contract_operations, usacc_elections,
    usacc_ledger_entries, usacc_simulation_runs, usacc_users, usacc_votes,
};
use sea_orm::sea_query::{Alias, Asterisk, Expr, Func, OnConflict, SimpleExpr};
use sea_orm::{
    ActiveValue::{NotSet, Set},
    ColumnTrait, ConnectionTrait, DatabaseBackend, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Statement,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tower_http::{cors::CorsLayer, trace::TraceLayer};

use crate::{
    auth::require_auth,
    contract, db,
    docs::{api_docs_html, api_docs_json},
    error::{ApiError, ApiResult},
    models::{
        json_object_or_default, CastVoteRequest, ContractProxyRequest, CreateCaseRequest,
        CreateElectionRequest, CreateStageRequest, CreateUserRequest, LedgerEntryRequest,
        LedgerSummary, PageQuery, PatchCaseRequest, PatchUserRequest,
        SimulateTransactionProxyRequest, SimulationRunRequest, SimulationRunResponse, TallyChoice,
        TallyResponse,
    },
    simulation::run_simulation,
    state::AppState,
};

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/", get(root))
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .route("/api/usacc", get(api_index))
        .route("/api/usacc/users", get(list_users).post(create_user))
        .route("/api/usacc/users/:id", get(get_user).patch(patch_user))
        .route("/api/usacc/cases", get(list_cases).post(create_case))
        .route("/api/usacc/cases/:id", get(get_case).patch(patch_case))
        .route(
            "/api/usacc/cases/:case_id/stages",
            get(list_case_stages).post(create_case_stage),
        )
        .route(
            "/api/usacc/elections",
            get(list_elections).post(create_election),
        )
        .route(
            "/api/usacc/elections/:election_id/votes",
            get(list_votes).post(cast_vote),
        )
        .route(
            "/api/usacc/elections/:election_id/tally",
            post(tally_election),
        )
        .route(
            "/api/usacc/accounting/ledger-entries",
            post(create_ledger_entry),
        )
        .route("/api/usacc/cases/:case_id/ledger", get(case_ledger))
        .route("/api/usacc/contracts/validate", post(validate_contract))
        .route("/api/usacc/contracts/simulate", post(simulate_contract))
        .route("/api/usacc/simulations", post(run_simulation_route))
        .route("/api/usacc/simulations/:id", get(get_simulation_run))
        // Permissive CORS is for cross-origin JSON API consumers and is
        // scoped to the API routes here, before the merge, so it does NOT
        // wrap the same-origin-only `/app` console.
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());

    // The HTMX operator console is a parallel surface over the same pool.
    // Its security middleware (and its own trace layer) are scoped to
    // `/app` because they are attached inside `ui::router` before the merge.
    let app = if state.config.app_ui_enabled {
        api.merge(crate::ui::router(&state))
    } else {
        api
    };

    app.layer(middleware::from_fn_with_state(
        state.clone(),
        observe_http_response,
    ))
    .with_state(state)
}

async fn observe_http_response(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let started = Instant::now();
    let response = next.run(request).await;
    state
        .metrics
        .observe_http_response(response.status().as_u16(), started.elapsed());
    response
}

async fn root(State(state): State<AppState>) -> Json<Value> {
    state.metrics.inc_http();
    Json(service_index(&state))
}

async fn api_index(State(state): State<AppState>) -> Json<Value> {
    state.metrics.inc_http();
    Json(service_index(&state))
}

fn service_index(state: &AppState) -> Value {
    json!({
        "ok": true,
        "service": "usacc-rest-api-backend-rs",
        "databaseConfigured": state.database_configured(),
        "routes": {
            "users": "/api/usacc/users",
            "cases": "/api/usacc/cases",
            "elections": "/api/usacc/elections",
            "ledger": "/api/usacc/cases/{caseId}/ledger",
            "contracts": "/api/usacc/contracts/validate",
            "simulations": "/api/usacc/simulations",
            "docs": ["/docs/api", "/api/docs", "/api/docs.json"]
        }
    })
}

async fn healthz(State(state): State<AppState>) -> Json<Value> {
    state.metrics.inc_http();
    Json(json!({
        "ok": true,
        "service": "usacc-rest-api-backend-rs",
        "databaseConfigured": state.database_configured(),
        "contractServiceUrl": state.config.contract_service_url.as_str(),
    }))
}

async fn metrics(State(state): State<AppState>) -> Response {
    state.metrics.inc_http();
    let body = state.metrics.render(state.database_configured());
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
        .into_response()
}

async fn list_users(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(page): Query<PageQuery>,
) -> ApiResult<Json<Vec<UsaccUsersRow>>> {
    state.metrics.inc_http();
    require_auth(&headers, &state)?;
    let pool = db::pool(&state)?;
    state.metrics.inc_db_query();
    let rows = usacc_users::Entity::find()
        .order_by_desc(usacc_users::Column::CreatedAt)
        .limit(page.limit(state.config.max_page_limit) as u64)
        .offset(page.offset() as u64)
        .all(pool)
        .await?
        .into_iter()
        .map(db::user_row)
        .collect();
    Ok(Json(rows))
}

async fn create_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateUserRequest>,
) -> ApiResult<(StatusCode, Json<UsaccUsersRow>)> {
    state.metrics.inc_http();
    require_auth(&headers, &state)?;
    let pool = db::pool(&state)?;
    let roles = json_object_or_default(body.roles);
    let meta_data = json_object_or_default(body.meta_data);
    let is_legal_entity = body.is_legal_entity.unwrap_or(false);
    let user_kind = body.user_kind.unwrap_or_else(|| {
        if is_legal_entity {
            "legal_entity".to_string()
        } else {
            "natural_person".to_string()
        }
    });
    let status = body.status.unwrap_or_else(|| "active".to_string());
    let kyc_level = body.kyc_level.unwrap_or_else(|| "none".to_string());

    validate_usacc_users_insert(&UsaccUsersInsert {
        display_name: Some(body.display_name.clone()),
        external_subject: body.external_subject.clone(),
        email_hash: body.email_hash.clone(),
        user_kind: Some(user_kind.clone()),
        status: Some(status.clone()),
        kyc_level: Some(kyc_level.clone()),
        roles: Some(roles.clone()),
        is_legal_entity: Some(is_legal_entity),
        legal_region: body.legal_region.clone(),
        meta_data: Some(meta_data.clone()),
        ..Default::default()
    })
    .map_err(ApiError::bad_request)?;

    state.metrics.inc_db_query();
    let id = usacc_users::Entity::insert(usacc_users::ActiveModel {
        external_subject: Set(body.external_subject),
        email_hash: Set(body.email_hash),
        display_name: Set(body.display_name),
        user_kind: Set(user_kind),
        status: Set(status),
        kyc_level: Set(kyc_level),
        roles: Set(roles),
        is_legal_entity: Set(is_legal_entity),
        legal_region: Set(body.legal_region),
        meta_data: Set(meta_data),
        ..Default::default()
    })
    .exec(pool)
    .await?
    .last_insert_id;
    Ok((
        StatusCode::CREATED,
        Json(fetch_user(&state, &id.to_string()).await?),
    ))
}

async fn get_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<UsaccUsersRow>> {
    state.metrics.inc_http();
    require_auth(&headers, &state)?;
    Ok(Json(fetch_user(&state, &id).await?))
}

async fn patch_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<PatchUserRequest>,
) -> ApiResult<Json<UsaccUsersRow>> {
    state.metrics.inc_http();
    require_auth(&headers, &state)?;
    let pool = db::pool(&state)?;
    if let Some(roles) = &body.roles {
        if !roles.is_object() {
            return Err(ApiError::bad_request("roles must be a JSON object"));
        }
    }
    if let Some(meta_data) = &body.meta_data {
        if !meta_data.is_object() {
            return Err(ApiError::bad_request("metaData must be a JSON object"));
        }
    }
    // Absent fields stay `NotSet`, matching the old `coalesce($n, column)`
    // update that left them untouched.
    let user_id = db::parse_uuid(&id)?;
    state.metrics.inc_db_query();
    let updated = usacc_users::Entity::update_many()
        .set(usacc_users::ActiveModel {
            display_name: body.display_name.map_or(NotSet, Set),
            status: body.status.map_or(NotSet, Set),
            kyc_level: body.kyc_level.map_or(NotSet, Set),
            roles: body.roles.map_or(NotSet, Set),
            legal_region: body.legal_region.map_or(NotSet, |value| Set(Some(value))),
            meta_data: body.meta_data.map_or(NotSet, Set),
            ..Default::default()
        })
        .col_expr(
            usacc_users::Column::UpdatedAt,
            Expr::current_timestamp().into(),
        )
        .filter(usacc_users::Column::Id.eq(user_id))
        .exec(pool)
        .await?;
    if updated.rows_affected == 0 {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "user not found"));
    }
    Ok(Json(fetch_user(&state, &id).await?))
}

async fn fetch_user(state: &AppState, id: &str) -> ApiResult<UsaccUsersRow> {
    let pool = db::pool(state)?;
    let user_id = db::parse_uuid(id)?;
    state.metrics.inc_db_query();
    let user = usacc_users::Entity::find_by_id(user_id)
        .one(pool)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "row not found"))?;
    Ok(db::user_row(user))
}

async fn list_cases(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(page): Query<PageQuery>,
) -> ApiResult<Json<Vec<UsaccCasesRow>>> {
    state.metrics.inc_http();
    require_auth(&headers, &state)?;
    let pool = db::pool(&state)?;
    state.metrics.inc_db_query();
    let rows = usacc_cases::Entity::find()
        .order_by_desc(usacc_cases::Column::CreatedAt)
        .limit(page.limit(state.config.max_page_limit) as u64)
        .offset(page.offset() as u64)
        .all(pool)
        .await?
        .into_iter()
        .map(db::case_row)
        .collect();
    Ok(Json(rows))
}

async fn create_case(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateCaseRequest>,
) -> ApiResult<(StatusCode, Json<UsaccCasesRow>)> {
    state.metrics.inc_http();
    require_auth(&headers, &state)?;
    let pool = db::pool(&state)?;
    let meta_data = json_object_or_default(body.meta_data);
    let status = body.status.unwrap_or_else(|| "draft".to_string());
    let filing_tier = body.filing_tier.unwrap_or_else(|| "screen".to_string());
    let priority_score_micros = body.priority_score_micros.unwrap_or(0);

    validate_usacc_cases_insert(&UsaccCasesInsert {
        case_number: Some(body.case_number.clone()),
        title: Some(body.title.clone()),
        status: Some(status.clone()),
        filing_tier: Some(filing_tier.clone()),
        plaintiff_user_id: body.plaintiff_user_id.clone(),
        defendant_summary: Some(body.defendant_summary.clone()),
        conduct_summary: Some(body.conduct_summary.clone()),
        conduct_fingerprint: body.conduct_fingerprint.clone(),
        conduct_window_start: body.conduct_window_start.clone(),
        conduct_window_end: body.conduct_window_end.clone(),
        priority_score_micros: Some(priority_score_micros),
        meta_data: Some(meta_data.clone()),
        ..Default::default()
    })
    .map_err(ApiError::bad_request)?;

    let plaintiff_user_id = body
        .plaintiff_user_id
        .as_deref()
        .map(db::parse_uuid)
        .transpose()?;
    state.metrics.inc_db_query();
    let id = usacc_cases::Entity::insert(usacc_cases::ActiveModel {
        case_number: Set(body.case_number),
        title: Set(body.title),
        status: Set(status),
        filing_tier: Set(filing_tier),
        plaintiff_user_id: Set(plaintiff_user_id),
        defendant_summary: Set(body.defendant_summary),
        conduct_summary: Set(body.conduct_summary),
        conduct_fingerprint: Set(body.conduct_fingerprint),
        conduct_window_start: Set(body.conduct_window_start),
        conduct_window_end: Set(body.conduct_window_end),
        priority_score_micros: Set(priority_score_micros),
        meta_data: Set(meta_data),
        ..Default::default()
    })
    .exec(pool)
    .await?
    .last_insert_id;
    Ok((
        StatusCode::CREATED,
        Json(fetch_case(&state, &id.to_string()).await?),
    ))
}

async fn get_case(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<UsaccCasesRow>> {
    state.metrics.inc_http();
    require_auth(&headers, &state)?;
    Ok(Json(fetch_case(&state, &id).await?))
}

async fn patch_case(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<PatchCaseRequest>,
) -> ApiResult<Json<UsaccCasesRow>> {
    state.metrics.inc_http();
    require_auth(&headers, &state)?;
    let pool = db::pool(&state)?;
    if let Some(meta_data) = &body.meta_data {
        if !meta_data.is_object() {
            return Err(ApiError::bad_request("metaData must be a JSON object"));
        }
    }
    // Absent fields stay `NotSet`, matching the old `coalesce($n, column)`
    // update that left them untouched.
    let case_id = db::parse_uuid(&id)?;
    state.metrics.inc_db_query();
    let updated = usacc_cases::Entity::update_many()
        .set(usacc_cases::ActiveModel {
            title: body.title.map_or(NotSet, Set),
            status: body.status.map_or(NotSet, Set),
            filing_tier: body.filing_tier.map_or(NotSet, Set),
            priority_score_micros: body.priority_score_micros.map_or(NotSet, Set),
            meta_data: body.meta_data.map_or(NotSet, Set),
            ..Default::default()
        })
        .col_expr(
            usacc_cases::Column::UpdatedAt,
            Expr::current_timestamp().into(),
        )
        .filter(usacc_cases::Column::Id.eq(case_id))
        .exec(pool)
        .await?;
    if updated.rows_affected == 0 {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "case not found"));
    }
    Ok(Json(fetch_case(&state, &id).await?))
}

async fn fetch_case(state: &AppState, id: &str) -> ApiResult<UsaccCasesRow> {
    let pool = db::pool(state)?;
    let case_id = db::parse_uuid(id)?;
    state.metrics.inc_db_query();
    let case = usacc_cases::Entity::find_by_id(case_id)
        .one(pool)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "row not found"))?;
    Ok(db::case_row(case))
}

async fn list_case_stages(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(case_id): Path<String>,
) -> ApiResult<Json<Vec<UsaccCaseStagesRow>>> {
    state.metrics.inc_http();
    require_auth(&headers, &state)?;
    let pool = db::pool(&state)?;
    let case_id = db::parse_uuid(&case_id)?;
    state.metrics.inc_db_query();
    let rows = usacc_case_stages::Entity::find()
        .filter(usacc_case_stages::Column::CaseId.eq(case_id))
        .order_by_asc(usacc_case_stages::Column::StageOrder)
        .all(pool)
        .await?
        .into_iter()
        .map(db::case_stage_row)
        .collect();
    Ok(Json(rows))
}

async fn create_case_stage(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(case_id): Path<String>,
    Json(body): Json<CreateStageRequest>,
) -> ApiResult<(StatusCode, Json<UsaccCaseStagesRow>)> {
    state.metrics.inc_http();
    require_auth(&headers, &state)?;
    let pool = db::pool(&state)?;
    let meta_data = json_object_or_default(body.meta_data);
    let status = body.status.unwrap_or_else(|| "pending".to_string());
    validate_usacc_case_stages_insert(&UsaccCaseStagesInsert {
        case_id: Some(case_id.clone()),
        stage_key: Some(body.stage_key.clone()),
        stage_order: Some(body.stage_order),
        title: Some(body.title.clone()),
        status: Some(status.clone()),
        assigned_user_id: body.assigned_user_id.clone(),
        decision_summary: body.decision_summary.clone(),
        meta_data: Some(meta_data.clone()),
        ..Default::default()
    })
    .map_err(ApiError::bad_request)?;
    let case_id = db::parse_uuid(&case_id)?;
    let assigned_user_id = body
        .assigned_user_id
        .as_deref()
        .map(db::parse_uuid)
        .transpose()?;
    state.metrics.inc_db_query();
    let id = usacc_case_stages::Entity::insert(usacc_case_stages::ActiveModel {
        case_id: Set(case_id),
        stage_key: Set(body.stage_key),
        stage_order: Set(body.stage_order),
        title: Set(body.title),
        status: Set(status),
        assigned_user_id: Set(assigned_user_id),
        decision_summary: Set(body.decision_summary),
        meta_data: Set(meta_data),
        ..Default::default()
    })
    .exec(pool)
    .await?
    .last_insert_id;
    Ok((
        StatusCode::CREATED,
        Json(fetch_stage(&state, &id.to_string()).await?),
    ))
}

async fn fetch_stage(state: &AppState, id: &str) -> ApiResult<UsaccCaseStagesRow> {
    let pool = db::pool(state)?;
    let stage_id = db::parse_uuid(id)?;
    state.metrics.inc_db_query();
    let stage = usacc_case_stages::Entity::find_by_id(stage_id)
        .one(pool)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "row not found"))?;
    Ok(db::case_stage_row(stage))
}

async fn list_elections(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(page): Query<PageQuery>,
) -> ApiResult<Json<Vec<UsaccElectionsRow>>> {
    state.metrics.inc_http();
    require_auth(&headers, &state)?;
    let pool = db::pool(&state)?;
    state.metrics.inc_db_query();
    let rows = usacc_elections::Entity::find()
        .order_by_desc(usacc_elections::Column::CreatedAt)
        .limit(page.limit(state.config.max_page_limit) as u64)
        .offset(page.offset() as u64)
        .all(pool)
        .await?
        .into_iter()
        .map(db::election_row)
        .collect();
    Ok(Json(rows))
}

async fn create_election(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateElectionRequest>,
) -> ApiResult<(StatusCode, Json<UsaccElectionsRow>)> {
    state.metrics.inc_http();
    require_auth(&headers, &state)?;
    let pool = db::pool(&state)?;
    let meta_data = json_object_or_default(body.meta_data);
    let status = body.status.unwrap_or_else(|| "draft".to_string());
    let quorum_count = body.quorum_count.unwrap_or(1);
    let threshold_micros = body.threshold_micros.unwrap_or(500_000);
    validate_usacc_elections_insert(&UsaccElectionsInsert {
        case_id: body.case_id.clone(),
        stage_id: body.stage_id.clone(),
        election_kind: Some(body.election_kind.clone()),
        title: Some(body.title.clone()),
        status: Some(status.clone()),
        quorum_count: Some(quorum_count),
        threshold_micros: Some(threshold_micros),
        tally: Some(json!({})),
        meta_data: Some(meta_data.clone()),
        ..Default::default()
    })
    .map_err(ApiError::bad_request)?;
    let case_id = body.case_id.as_deref().map(db::parse_uuid).transpose()?;
    let stage_id = body.stage_id.as_deref().map(db::parse_uuid).transpose()?;
    state.metrics.inc_db_query();
    let id = usacc_elections::Entity::insert(usacc_elections::ActiveModel {
        case_id: Set(case_id),
        stage_id: Set(stage_id),
        election_kind: Set(body.election_kind),
        title: Set(body.title),
        status: Set(status),
        quorum_count: Set(quorum_count),
        threshold_micros: Set(threshold_micros),
        meta_data: Set(meta_data),
        ..Default::default()
    })
    .exec(pool)
    .await?
    .last_insert_id;
    Ok((
        StatusCode::CREATED,
        Json(fetch_election(&state, &id.to_string()).await?),
    ))
}

async fn fetch_election(state: &AppState, id: &str) -> ApiResult<UsaccElectionsRow> {
    let pool = db::pool(state)?;
    let election_id = db::parse_uuid(id)?;
    state.metrics.inc_db_query();
    let election = usacc_elections::Entity::find_by_id(election_id)
        .one(pool)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "row not found"))?;
    Ok(db::election_row(election))
}

async fn list_votes(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(election_id): Path<String>,
) -> ApiResult<Json<Vec<UsaccVotesRow>>> {
    state.metrics.inc_http();
    require_auth(&headers, &state)?;
    let pool = db::pool(&state)?;
    let election_id = db::parse_uuid(&election_id)?;
    state.metrics.inc_db_query();
    let rows = usacc_votes::Entity::find()
        .filter(usacc_votes::Column::ElectionId.eq(election_id))
        .order_by_desc(usacc_votes::Column::CreatedAt)
        .all(pool)
        .await?
        .into_iter()
        .map(db::vote_row)
        .collect();
    Ok(Json(rows))
}

async fn cast_vote(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(election_id): Path<String>,
    Json(body): Json<CastVoteRequest>,
) -> ApiResult<(StatusCode, Json<UsaccVotesRow>)> {
    state.metrics.inc_http();
    require_auth(&headers, &state)?;
    let pool = db::pool(&state)?;
    let meta_data = json_object_or_default(body.meta_data);
    let vote_kind = body.vote_kind.unwrap_or_else(|| "choice".to_string());
    let weight_micros = body.weight_micros.unwrap_or(1_000_000);
    let mut contract_digest = None;
    if let Some(envelope) = &body.contract_envelope {
        let contract_response = contract::validate_envelope(&state, envelope).await?;
        contract_digest = contract::digest_from_contract_response(&contract_response);
    }
    validate_usacc_votes_insert(&UsaccVotesInsert {
        election_id: Some(election_id.clone()),
        case_id: body.case_id.clone(),
        voter_user_id: Some(body.voter_user_id.clone()),
        vote_kind: Some(vote_kind.clone()),
        vote_value: Some(body.vote_value.clone()),
        weight_micros: Some(weight_micros),
        commitment_hash: body.commitment_hash.clone(),
        sealed_payload: body.sealed_payload.clone(),
        contract_digest: contract_digest.clone(),
        meta_data: Some(meta_data.clone()),
        ..Default::default()
    })
    .map_err(ApiError::bad_request)?;

    let election_id = db::parse_uuid(&election_id)?;
    let case_id = body.case_id.as_deref().map(db::parse_uuid).transpose()?;
    let voter_user_id = db::parse_uuid(&body.voter_user_id)?;
    state.metrics.inc_db_query();
    let id = usacc_votes::Entity::insert(usacc_votes::ActiveModel {
        election_id: Set(election_id),
        case_id: Set(case_id),
        voter_user_id: Set(voter_user_id),
        vote_kind: Set(vote_kind),
        vote_value: Set(body.vote_value),
        weight_micros: Set(weight_micros),
        commitment_hash: Set(body.commitment_hash),
        sealed_payload: Set(body.sealed_payload),
        contract_digest: Set(contract_digest),
        meta_data: Set(meta_data),
        ..Default::default()
    })
    .on_conflict(
        OnConflict::columns([
            usacc_votes::Column::ElectionId,
            usacc_votes::Column::VoterUserId,
        ])
        .update_columns([
            usacc_votes::Column::VoteKind,
            usacc_votes::Column::VoteValue,
            usacc_votes::Column::WeightMicros,
            usacc_votes::Column::CommitmentHash,
            usacc_votes::Column::SealedPayload,
            usacc_votes::Column::ContractDigest,
            usacc_votes::Column::MetaData,
        ])
        .value(usacc_votes::Column::UpdatedAt, Expr::current_timestamp())
        .to_owned(),
    )
    .exec(pool)
    .await?
    .last_insert_id;
    state
        .metrics
        .votes_cast_total
        .fetch_add(1, Ordering::Relaxed);
    Ok((
        StatusCode::CREATED,
        Json(fetch_vote(&state, &id.to_string()).await?),
    ))
}

async fn fetch_vote(state: &AppState, id: &str) -> ApiResult<UsaccVotesRow> {
    let pool = db::pool(state)?;
    let vote_id = db::parse_uuid(id)?;
    state.metrics.inc_db_query();
    let vote = usacc_votes::Entity::find_by_id(vote_id)
        .one(pool)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "row not found"))?;
    Ok(db::vote_row(vote))
}

async fn tally_election(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(election_id): Path<String>,
) -> ApiResult<Json<TallyResponse>> {
    state.metrics.inc_http();
    require_auth(&headers, &state)?;
    let pool = db::pool(&state)?;

    let election = fetch_election(&state, &election_id).await?;
    let election_uuid = db::parse_uuid(&election_id)?;
    let weight_sum: SimpleExpr = Func::coalesce([
        Expr::col(usacc_votes::Column::WeightMicros).sum(),
        Expr::val(0i64).into(),
    ])
    .into();
    state.metrics.inc_db_query();
    let choices: Vec<TallyChoice> = usacc_votes::Entity::find()
        .select_only()
        .column(usacc_votes::Column::VoteValue)
        .column_as(Expr::col(Asterisk).count(), "vote_count")
        .column_as(weight_sum, "weight_micros")
        .filter(usacc_votes::Column::ElectionId.eq(election_uuid))
        .group_by(usacc_votes::Column::VoteValue)
        .order_by_desc(Expr::col(Alias::new("weight_micros")))
        .order_by_desc(Expr::col(Alias::new("vote_count")))
        .order_by_asc(usacc_votes::Column::VoteValue)
        .into_model::<TallyChoice>()
        .all(pool)
        .await?;
    let total_votes = choices.iter().map(|choice| choice.vote_count).sum::<i64>();
    let total_weight_micros = choices
        .iter()
        .map(|choice| choice.weight_micros)
        .sum::<i64>();
    let winner = choices.first();
    let passed = winner
        .map(|choice| {
            choice.weight_micros.saturating_mul(1_000_000)
                >= total_weight_micros.saturating_mul(election.threshold_micros as i64)
        })
        .unwrap_or(false);
    let winning_value = winner.map(|choice| choice.vote_value.clone());
    let response = TallyResponse {
        ok: true,
        election_id: election_id.clone(),
        total_votes,
        total_weight_micros,
        threshold_micros: election.threshold_micros,
        winning_value,
        passed,
        choices,
    };
    let tally = serde_json::to_value(&response).unwrap_or_else(|_| json!({}));
    state.metrics.inc_db_query();
    usacc_elections::Entity::update_many()
        .set(usacc_elections::ActiveModel {
            status: Set("certified".to_string()),
            tally: Set(tally),
            ..Default::default()
        })
        .col_expr(
            usacc_elections::Column::UpdatedAt,
            Expr::current_timestamp().into(),
        )
        .filter(usacc_elections::Column::Id.eq(election_uuid))
        .exec(pool)
        .await?;
    state.metrics.tallies_total.fetch_add(1, Ordering::Relaxed);
    Ok(Json(response))
}

async fn create_ledger_entry(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<LedgerEntryRequest>,
) -> ApiResult<(StatusCode, Json<UsaccLedgerEntriesRow>)> {
    state.metrics.inc_http();
    require_auth(&headers, &state)?;
    let pool = db::pool(&state)?;
    let meta_data = json_object_or_default(body.meta_data);
    let currency = body.currency.unwrap_or_else(|| "USD".to_string());
    validate_usacc_ledger_entries_insert(&UsaccLedgerEntriesInsert {
        case_id: body.case_id.clone(),
        escrow_account_id: body.escrow_account_id.clone(),
        user_id: body.user_id.clone(),
        entry_kind: Some(body.entry_kind.clone()),
        direction: Some(body.direction.clone()),
        amount_cents: Some(body.amount_cents),
        currency: Some(currency.clone()),
        provider_ref: body.provider_ref.clone(),
        contract_digest: body.contract_digest.clone(),
        meta_data: Some(meta_data.clone()),
        ..Default::default()
    })
    .map_err(ApiError::bad_request)?;

    let case_id = body.case_id.as_deref().map(db::parse_uuid).transpose()?;
    let escrow_account_id = body
        .escrow_account_id
        .as_deref()
        .map(db::parse_uuid)
        .transpose()?;
    let user_id = body.user_id.as_deref().map(db::parse_uuid).transpose()?;
    state.metrics.inc_db_query();
    let id = usacc_ledger_entries::Entity::insert(usacc_ledger_entries::ActiveModel {
        case_id: Set(case_id),
        escrow_account_id: Set(escrow_account_id),
        user_id: Set(user_id),
        entry_kind: Set(body.entry_kind),
        direction: Set(body.direction),
        amount_cents: Set(body.amount_cents),
        currency: Set(currency),
        provider_ref: Set(body.provider_ref),
        contract_digest: Set(body.contract_digest),
        meta_data: Set(meta_data),
        ..Default::default()
    })
    .exec(pool)
    .await?
    .last_insert_id;
    Ok((
        StatusCode::CREATED,
        Json(fetch_ledger_entry(&state, &id.to_string()).await?),
    ))
}

async fn case_ledger(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(case_id): Path<String>,
) -> ApiResult<Json<Value>> {
    state.metrics.inc_http();
    require_auth(&headers, &state)?;
    let pool = db::pool(&state)?;
    let case_uuid = db::parse_uuid(&case_id)?;
    state.metrics.inc_db_query();
    let entries = usacc_ledger_entries::Entity::find()
        .filter(usacc_ledger_entries::Column::CaseId.eq(case_uuid))
        .order_by_desc(usacc_ledger_entries::Column::CreatedAt)
        .all(pool)
        .await?
        .into_iter()
        .map(db::ledger_entry_row)
        .collect::<Vec<_>>();
    let summary = summarize_ledger(&case_id, &entries);
    Ok(Json(json!({
        "ok": true,
        "summary": summary,
        "entries": entries,
    })))
}

async fn fetch_ledger_entry(state: &AppState, id: &str) -> ApiResult<UsaccLedgerEntriesRow> {
    let pool = db::pool(state)?;
    let entry_id = db::parse_uuid(id)?;
    state.metrics.inc_db_query();
    let entry = usacc_ledger_entries::Entity::find_by_id(entry_id)
        .one(pool)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "row not found"))?;
    Ok(db::ledger_entry_row(entry))
}

fn summarize_ledger(case_id: &str, entries: &[UsaccLedgerEntriesRow]) -> LedgerSummary {
    let currency = entries
        .first()
        .map(|entry| entry.currency.clone())
        .unwrap_or_else(|| "USD".to_string());
    let mut summary = LedgerSummary {
        case_id: case_id.to_string(),
        currency,
        debits_cents: 0,
        credits_cents: 0,
        net_cents: 0,
        pledge_cents: 0,
        capture_cents: 0,
        refund_cents: 0,
        disbursement_cents: 0,
    };
    for entry in entries {
        match entry.direction.as_str() {
            "debit" => summary.debits_cents += entry.amount_cents,
            "credit" => summary.credits_cents += entry.amount_cents,
            _ => {}
        }
        match entry.entry_kind.as_str() {
            "pledge" => summary.pledge_cents += entry.amount_cents,
            "capture" => summary.capture_cents += entry.amount_cents,
            "refund" => summary.refund_cents += entry.amount_cents,
            "disbursement" => summary.disbursement_cents += entry.amount_cents,
            _ => {}
        }
    }
    summary.net_cents = summary.credits_cents - summary.debits_cents;
    summary
}

async fn validate_contract(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ContractProxyRequest>,
) -> ApiResult<Json<Value>> {
    state.metrics.inc_http();
    require_auth(&headers, &state)?;
    let request_id = body
        .request_id
        .unwrap_or_else(|| format!("usacc-contract-{}", nowish_hash(&body.envelope)));
    let response = contract::validate_envelope(&state, &body.envelope).await?;
    persist_contract_operation(
        &state,
        ContractOperation {
            case_id: body.case_id,
            election_id: body.election_id,
            vote_id: body.vote_id,
            request_id,
            operation_kind: body
                .operation_kind
                .unwrap_or_else(|| "validate_envelope".to_string()),
            envelope: &body.envelope,
            response: &response,
        },
    )
    .await?;
    Ok(Json(json!({ "ok": true, "contract": response })))
}

async fn simulate_contract(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SimulateTransactionProxyRequest>,
) -> ApiResult<Json<Value>> {
    state.metrics.inc_http();
    require_auth(&headers, &state)?;
    let request_id = body
        .request_id
        .unwrap_or_else(|| format!("usacc-simulate-{}", nowish_hash(&body.payload)));
    let response = contract::simulate_transaction(&state, &body.payload).await?;
    persist_contract_operation(
        &state,
        ContractOperation {
            case_id: body.case_id,
            election_id: None,
            vote_id: None,
            request_id,
            operation_kind: "simulate_transaction".to_string(),
            envelope: &body.payload,
            response: &response,
        },
    )
    .await?;
    Ok(Json(json!({ "ok": true, "contract": response })))
}

struct ContractOperation<'a> {
    case_id: Option<String>,
    election_id: Option<String>,
    vote_id: Option<String>,
    request_id: String,
    operation_kind: String,
    envelope: &'a Value,
    response: &'a Value,
}

async fn persist_contract_operation(
    state: &AppState,
    operation: ContractOperation<'_>,
) -> ApiResult<()> {
    let ContractOperation {
        case_id,
        election_id,
        vote_id,
        request_id,
        operation_kind,
        envelope,
        response,
    } = operation;
    let Some(pool) = state.pool.as_ref() else {
        return Ok(());
    };
    let digest = contract::digest_from_contract_response(response);
    let status = if response.get("ok").and_then(Value::as_bool).unwrap_or(true) {
        match operation_kind.as_str() {
            "simulate_transaction" => "simulated",
            _ => "validated",
        }
    } else {
        "failed"
    };
    let program_id = envelope
        .get("programId")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let case_id = case_id.as_deref().map(db::parse_uuid).transpose()?;
    let election_id = election_id.as_deref().map(db::parse_uuid).transpose()?;
    let vote_id = vote_id.as_deref().map(db::parse_uuid).transpose()?;
    state.metrics.inc_db_query();
    usacc_contract_operations::Entity::insert(usacc_contract_operations::ActiveModel {
        case_id: Set(case_id),
        election_id: Set(election_id),
        vote_id: Set(vote_id),
        request_id: Set(request_id),
        operation_kind: Set(operation_kind),
        status: Set(status.to_string()),
        program_id: Set(program_id),
        digest: Set(digest),
        envelope: Set(envelope.clone()),
        response: Set(response.clone()),
        ..Default::default()
    })
    .on_conflict(
        OnConflict::column(usacc_contract_operations::Column::RequestId)
            .update_columns([
                usacc_contract_operations::Column::Status,
                usacc_contract_operations::Column::Digest,
                usacc_contract_operations::Column::Response,
            ])
            .value(
                usacc_contract_operations::Column::UpdatedAt,
                Expr::current_timestamp(),
            )
            .to_owned(),
    )
    .exec_without_returning(pool)
    .await?;
    Ok(())
}

async fn run_simulation_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SimulationRunRequest>,
) -> ApiResult<Json<SimulationRunResponse>> {
    state.metrics.inc_http();
    require_auth(&headers, &state)?;
    let should_persist = body.persist.unwrap_or(true);
    let mut response = run_simulation(body.clone());
    state
        .metrics
        .simulations_total
        .fetch_add(1, Ordering::Relaxed);

    if should_persist {
        if let Some(pool) = state.pool.as_ref() {
            let seed_i64 = response.seed.min(i64::MAX as u64) as i64;
            let input = body.input.unwrap_or_else(|| json!({}));
            // `started_at`/`finished_at` must stay on the database clock
            // (`now()`), which the entity insert API cannot express, so this
            // one stays a raw parameterized statement.
            let sql = format!(
                "insert into {USACC_SIMULATION_RUNS_TABLE} \
                 (case_id, status, mode, seed, horizon_days, actor_count, event_count, metrics, trace, input, started_at, finished_at) \
                 values ($1::uuid, 'succeeded', 'sim', $2, $3, $4, $5, $6::jsonb, $7::jsonb, $8::jsonb, now(), now()) \
                 returning id::text"
            );
            state.metrics.inc_db_query();
            let row = pool
                .query_one(Statement::from_sql_and_values(
                    DatabaseBackend::Postgres,
                    sql,
                    [
                        response.case_id.clone().into(),
                        seed_i64.into(),
                        response.horizon_days.into(),
                        response.actor_count.into(),
                        (response.event_count.min(i32::MAX as u64) as i32).into(),
                        response.metrics.clone().into(),
                        response.trace.clone().into(),
                        input.into(),
                    ],
                ))
                .await?
                .ok_or_else(|| {
                    ApiError::internal("database error: simulation insert returned no row")
                })?;
            let id: String = row.try_get("", "id").map_err(ApiError::from)?;
            response.persisted = true;
            response.run_id = Some(id);
        }
    }
    Ok(Json(response))
}

async fn get_simulation_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<UsaccSimulationRunsRow>> {
    state.metrics.inc_http();
    require_auth(&headers, &state)?;
    let pool = db::pool(&state)?;
    let run_id = db::parse_uuid(&id)?;
    state.metrics.inc_db_query();
    let run = usacc_simulation_runs::Entity::find_by_id(run_id)
        .one(pool)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "row not found"))?;
    Ok(Json(db::simulation_run_row(run)))
}

fn nowish_hash(value: &Value) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.to_string().as_bytes());
    hasher.update(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_string()
            .as_bytes(),
    );
    hex::encode(&hasher.finalize()[..8])
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    use tower::ServiceExt;

    use super::*;
    use crate::config::Config;

    fn test_state() -> AppState {
        AppState::new(
            Config {
                host: "127.0.0.1".into(),
                port: 8121,
                database_url: None,
                auth_secret: None,
                auth_required: false,
                contract_service_url: "http://localhost".into(),
                request_timeout: Duration::from_secs(5),
                max_page_limit: 250,
                app_ui_enabled: false,
                app_base_path: String::new(),
                app_ui_bearer: None,
                app_ui_allowed_origins: vec![],
            },
            None,
            reqwest::Client::new(),
        )
    }

    #[tokio::test]
    async fn metrics_route_exposes_prometheus_contract_and_observed_responses() {
        let app = router(test_state());
        let healthy = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(healthy.status(), StatusCode::OK);

        let missing = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/does-not-exist")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("content-type").unwrap(),
            "text/plain; version=0.0.4; charset=utf-8"
        );
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = std::str::from_utf8(&body).unwrap();
        assert!(body.contains("usacc_rest_api_http_responses_total{status_class=\"2xx\"} 1"));
        assert!(body.contains("usacc_rest_api_http_responses_total{status_class=\"4xx\"} 1"));
        assert!(body.contains("usacc_rest_api_http_request_duration_seconds_bucket{le=\"+Inf\"} 2"));
    }

    #[test]
    fn ledger_summary_tracks_direction_and_kind() {
        let entries = vec![
            UsaccLedgerEntriesRow {
                id: "1".to_string(),
                case_id: Some("case".to_string()),
                escrow_account_id: None,
                user_id: None,
                entry_kind: "pledge".to_string(),
                direction: "credit".to_string(),
                amount_cents: 3000,
                currency: "USD".to_string(),
                provider_ref: None,
                contract_digest: None,
                meta_data: json!({}),
                created_at: "2026-06-08T00:00:00Z".to_string(),
            },
            UsaccLedgerEntriesRow {
                id: "2".to_string(),
                case_id: Some("case".to_string()),
                escrow_account_id: None,
                user_id: None,
                entry_kind: "refund".to_string(),
                direction: "debit".to_string(),
                amount_cents: 500,
                currency: "USD".to_string(),
                provider_ref: None,
                contract_digest: None,
                meta_data: json!({}),
                created_at: "2026-06-08T00:00:01Z".to_string(),
            },
        ];

        let summary = summarize_ledger("case", &entries);

        assert_eq!(summary.credits_cents, 3000);
        assert_eq!(summary.debits_cents, 500);
        assert_eq!(summary.net_cents, 2500);
        assert_eq!(summary.pledge_cents, 3000);
        assert_eq!(summary.refund_cents, 500);
    }
}
