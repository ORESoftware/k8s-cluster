use std::sync::atomic::Ordering;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
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
    UsaccUsersRow, UsaccVotesInsert, UsaccVotesRow, USACC_CASES_SELECT_SQL, USACC_CASES_TABLE,
    USACC_CASE_STAGES_SELECT_SQL, USACC_CASE_STAGES_TABLE, USACC_CONTRACT_OPERATIONS_TABLE,
    USACC_ELECTIONS_SELECT_SQL, USACC_ELECTIONS_TABLE, USACC_LEDGER_ENTRIES_SELECT_SQL,
    USACC_LEDGER_ENTRIES_TABLE, USACC_SIMULATION_RUNS_SELECT_SQL, USACC_SIMULATION_RUNS_TABLE,
    USACC_USERS_SELECT_SQL, USACC_USERS_TABLE, USACC_VOTES_SELECT_SQL, USACC_VOTES_TABLE,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::Row;
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

    app.with_state(state)
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
            "text/plain; version=0.0.4",
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
    let sql = format!("{USACC_USERS_SELECT_SQL} order by created_at desc limit $1 offset $2");
    state.metrics.inc_db_query();
    let rows = sqlx::query_as::<_, UsaccUsersRow>(&sql)
        .bind(page.limit(state.config.max_page_limit))
        .bind(page.offset())
        .fetch_all(pool)
        .await?;
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

    let sql = format!(
        "insert into {USACC_USERS_TABLE} \
         (external_subject, email_hash, display_name, user_kind, status, kyc_level, roles, is_legal_entity, legal_region, meta_data) \
         values ($1, $2, $3, $4, $5, $6, $7::jsonb, $8, $9, $10::jsonb) returning id::text"
    );
    state.metrics.inc_db_query();
    let id = sqlx::query_scalar::<_, String>(&sql)
        .bind(body.external_subject)
        .bind(body.email_hash)
        .bind(body.display_name)
        .bind(user_kind)
        .bind(status)
        .bind(kyc_level)
        .bind(roles)
        .bind(is_legal_entity)
        .bind(body.legal_region)
        .bind(meta_data)
        .fetch_one(pool)
        .await?;
    Ok((StatusCode::CREATED, Json(fetch_user(&state, &id).await?)))
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
    let sql = format!(
        "update {USACC_USERS_TABLE} set \
         display_name = coalesce($2, display_name), \
         status = coalesce($3, status), \
         kyc_level = coalesce($4, kyc_level), \
         roles = coalesce($5::jsonb, roles), \
         legal_region = coalesce($6, legal_region), \
         meta_data = coalesce($7::jsonb, meta_data), \
         updated_at = now() \
         where id = $1::uuid returning id::text"
    );
    state.metrics.inc_db_query();
    let updated = sqlx::query_scalar::<_, String>(&sql)
        .bind(&id)
        .bind(body.display_name)
        .bind(body.status)
        .bind(body.kyc_level)
        .bind(body.roles)
        .bind(body.legal_region)
        .bind(body.meta_data)
        .fetch_optional(pool)
        .await?;
    let Some(id) = updated else {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "user not found"));
    };
    Ok(Json(fetch_user(&state, &id).await?))
}

async fn fetch_user(state: &AppState, id: &str) -> ApiResult<UsaccUsersRow> {
    let pool = db::pool(state)?;
    let sql = format!("{USACC_USERS_SELECT_SQL} where id = $1::uuid");
    state.metrics.inc_db_query();
    Ok(sqlx::query_as::<_, UsaccUsersRow>(&sql)
        .bind(id)
        .fetch_one(pool)
        .await?)
}

async fn list_cases(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(page): Query<PageQuery>,
) -> ApiResult<Json<Vec<UsaccCasesRow>>> {
    state.metrics.inc_http();
    require_auth(&headers, &state)?;
    let pool = db::pool(&state)?;
    let sql = format!("{USACC_CASES_SELECT_SQL} order by created_at desc limit $1 offset $2");
    state.metrics.inc_db_query();
    let rows = sqlx::query_as::<_, UsaccCasesRow>(&sql)
        .bind(page.limit(state.config.max_page_limit))
        .bind(page.offset())
        .fetch_all(pool)
        .await?;
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

    let sql = format!(
        "insert into {USACC_CASES_TABLE} \
         (case_number, title, status, filing_tier, plaintiff_user_id, defendant_summary, conduct_summary, conduct_fingerprint, conduct_window_start, conduct_window_end, priority_score_micros, meta_data) \
         values ($1, $2, $3, $4, $5::uuid, $6, $7, $8, $9, $10, $11, $12::jsonb) returning id::text"
    );
    state.metrics.inc_db_query();
    let id = sqlx::query_scalar::<_, String>(&sql)
        .bind(body.case_number)
        .bind(body.title)
        .bind(status)
        .bind(filing_tier)
        .bind(body.plaintiff_user_id)
        .bind(body.defendant_summary)
        .bind(body.conduct_summary)
        .bind(body.conduct_fingerprint)
        .bind(body.conduct_window_start)
        .bind(body.conduct_window_end)
        .bind(priority_score_micros)
        .bind(meta_data)
        .fetch_one(pool)
        .await?;
    Ok((StatusCode::CREATED, Json(fetch_case(&state, &id).await?)))
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
    let sql = format!(
        "update {USACC_CASES_TABLE} set \
         title = coalesce($2, title), \
         status = coalesce($3, status), \
         filing_tier = coalesce($4, filing_tier), \
         priority_score_micros = coalesce($5, priority_score_micros), \
         meta_data = coalesce($6::jsonb, meta_data), \
         updated_at = now() \
         where id = $1::uuid returning id::text"
    );
    state.metrics.inc_db_query();
    let updated = sqlx::query_scalar::<_, String>(&sql)
        .bind(&id)
        .bind(body.title)
        .bind(body.status)
        .bind(body.filing_tier)
        .bind(body.priority_score_micros)
        .bind(body.meta_data)
        .fetch_optional(pool)
        .await?;
    let Some(id) = updated else {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "case not found"));
    };
    Ok(Json(fetch_case(&state, &id).await?))
}

async fn fetch_case(state: &AppState, id: &str) -> ApiResult<UsaccCasesRow> {
    let pool = db::pool(state)?;
    let sql = format!("{USACC_CASES_SELECT_SQL} where id = $1::uuid");
    state.metrics.inc_db_query();
    Ok(sqlx::query_as::<_, UsaccCasesRow>(&sql)
        .bind(id)
        .fetch_one(pool)
        .await?)
}

async fn list_case_stages(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(case_id): Path<String>,
) -> ApiResult<Json<Vec<UsaccCaseStagesRow>>> {
    state.metrics.inc_http();
    require_auth(&headers, &state)?;
    let pool = db::pool(&state)?;
    let sql =
        format!("{USACC_CASE_STAGES_SELECT_SQL} where case_id = $1::uuid order by stage_order asc");
    state.metrics.inc_db_query();
    let rows = sqlx::query_as::<_, UsaccCaseStagesRow>(&sql)
        .bind(case_id)
        .fetch_all(pool)
        .await?;
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
    let sql = format!(
        "insert into {USACC_CASE_STAGES_TABLE} \
         (case_id, stage_key, stage_order, title, status, assigned_user_id, decision_summary, meta_data) \
         values ($1::uuid, $2, $3, $4, $5, $6::uuid, $7, $8::jsonb) returning id::text"
    );
    state.metrics.inc_db_query();
    let id = sqlx::query_scalar::<_, String>(&sql)
        .bind(case_id)
        .bind(body.stage_key)
        .bind(body.stage_order)
        .bind(body.title)
        .bind(status)
        .bind(body.assigned_user_id)
        .bind(body.decision_summary)
        .bind(meta_data)
        .fetch_one(pool)
        .await?;
    Ok((StatusCode::CREATED, Json(fetch_stage(&state, &id).await?)))
}

async fn fetch_stage(state: &AppState, id: &str) -> ApiResult<UsaccCaseStagesRow> {
    let pool = db::pool(state)?;
    let sql = format!("{USACC_CASE_STAGES_SELECT_SQL} where id = $1::uuid");
    state.metrics.inc_db_query();
    Ok(sqlx::query_as::<_, UsaccCaseStagesRow>(&sql)
        .bind(id)
        .fetch_one(pool)
        .await?)
}

async fn list_elections(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(page): Query<PageQuery>,
) -> ApiResult<Json<Vec<UsaccElectionsRow>>> {
    state.metrics.inc_http();
    require_auth(&headers, &state)?;
    let pool = db::pool(&state)?;
    let sql = format!("{USACC_ELECTIONS_SELECT_SQL} order by created_at desc limit $1 offset $2");
    state.metrics.inc_db_query();
    let rows = sqlx::query_as::<_, UsaccElectionsRow>(&sql)
        .bind(page.limit(state.config.max_page_limit))
        .bind(page.offset())
        .fetch_all(pool)
        .await?;
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
    let sql = format!(
        "insert into {USACC_ELECTIONS_TABLE} \
         (case_id, stage_id, election_kind, title, status, quorum_count, threshold_micros, meta_data) \
         values ($1::uuid, $2::uuid, $3, $4, $5, $6, $7, $8::jsonb) returning id::text"
    );
    state.metrics.inc_db_query();
    let id = sqlx::query_scalar::<_, String>(&sql)
        .bind(body.case_id)
        .bind(body.stage_id)
        .bind(body.election_kind)
        .bind(body.title)
        .bind(status)
        .bind(quorum_count)
        .bind(threshold_micros)
        .bind(meta_data)
        .fetch_one(pool)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(fetch_election(&state, &id).await?),
    ))
}

async fn fetch_election(state: &AppState, id: &str) -> ApiResult<UsaccElectionsRow> {
    let pool = db::pool(state)?;
    let sql = format!("{USACC_ELECTIONS_SELECT_SQL} where id = $1::uuid");
    state.metrics.inc_db_query();
    Ok(sqlx::query_as::<_, UsaccElectionsRow>(&sql)
        .bind(id)
        .fetch_one(pool)
        .await?)
}

async fn list_votes(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(election_id): Path<String>,
) -> ApiResult<Json<Vec<UsaccVotesRow>>> {
    state.metrics.inc_http();
    require_auth(&headers, &state)?;
    let pool = db::pool(&state)?;
    let sql =
        format!("{USACC_VOTES_SELECT_SQL} where election_id = $1::uuid order by created_at desc");
    state.metrics.inc_db_query();
    let rows = sqlx::query_as::<_, UsaccVotesRow>(&sql)
        .bind(election_id)
        .fetch_all(pool)
        .await?;
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

    let sql = format!(
        "insert into {USACC_VOTES_TABLE} \
         (election_id, case_id, voter_user_id, vote_kind, vote_value, weight_micros, commitment_hash, sealed_payload, contract_digest, meta_data) \
         values ($1::uuid, $2::uuid, $3::uuid, $4, $5, $6, $7, $8::jsonb, $9, $10::jsonb) \
         on conflict (election_id, voter_user_id) do update set \
           vote_kind = excluded.vote_kind, vote_value = excluded.vote_value, weight_micros = excluded.weight_micros, \
           commitment_hash = excluded.commitment_hash, sealed_payload = excluded.sealed_payload, \
           contract_digest = excluded.contract_digest, meta_data = excluded.meta_data, updated_at = now() \
         returning id::text"
    );
    state.metrics.inc_db_query();
    let id = sqlx::query_scalar::<_, String>(&sql)
        .bind(election_id)
        .bind(body.case_id)
        .bind(body.voter_user_id)
        .bind(vote_kind)
        .bind(body.vote_value)
        .bind(weight_micros)
        .bind(body.commitment_hash)
        .bind(body.sealed_payload)
        .bind(contract_digest)
        .bind(meta_data)
        .fetch_one(pool)
        .await?;
    state
        .metrics
        .votes_cast_total
        .fetch_add(1, Ordering::Relaxed);
    Ok((StatusCode::CREATED, Json(fetch_vote(&state, &id).await?)))
}

async fn fetch_vote(state: &AppState, id: &str) -> ApiResult<UsaccVotesRow> {
    let pool = db::pool(state)?;
    let sql = format!("{USACC_VOTES_SELECT_SQL} where id = $1::uuid");
    state.metrics.inc_db_query();
    Ok(sqlx::query_as::<_, UsaccVotesRow>(&sql)
        .bind(id)
        .fetch_one(pool)
        .await?)
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
    let tally_sql = format!(
        "select vote_value, count(*)::bigint as vote_count, coalesce(sum(weight_micros), 0)::bigint as weight_micros \
         from {USACC_VOTES_TABLE} where election_id = $1::uuid group by vote_value order by weight_micros desc, vote_count desc, vote_value asc"
    );
    state.metrics.inc_db_query();
    let rows = sqlx::query(&tally_sql)
        .bind(&election_id)
        .fetch_all(pool)
        .await?;

    let choices: Vec<TallyChoice> = rows
        .into_iter()
        .map(|row| TallyChoice {
            vote_value: row.get("vote_value"),
            vote_count: row.get("vote_count"),
            weight_micros: row.get("weight_micros"),
        })
        .collect();
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
    let sql = format!(
        "update {USACC_ELECTIONS_TABLE} set status = 'certified', tally = $2::jsonb, updated_at = now() where id = $1::uuid"
    );
    state.metrics.inc_db_query();
    sqlx::query(&sql)
        .bind(&election_id)
        .bind(tally)
        .execute(pool)
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

    let sql = format!(
        "insert into {USACC_LEDGER_ENTRIES_TABLE} \
         (case_id, escrow_account_id, user_id, entry_kind, direction, amount_cents, currency, provider_ref, contract_digest, meta_data) \
         values ($1::uuid, $2::uuid, $3::uuid, $4, $5, $6, $7, $8, $9, $10::jsonb) returning id::text"
    );
    state.metrics.inc_db_query();
    let id = sqlx::query_scalar::<_, String>(&sql)
        .bind(body.case_id)
        .bind(body.escrow_account_id)
        .bind(body.user_id)
        .bind(body.entry_kind)
        .bind(body.direction)
        .bind(body.amount_cents)
        .bind(currency)
        .bind(body.provider_ref)
        .bind(body.contract_digest)
        .bind(meta_data)
        .fetch_one(pool)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(fetch_ledger_entry(&state, &id).await?),
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
    let sql = format!(
        "{USACC_LEDGER_ENTRIES_SELECT_SQL} where case_id = $1::uuid order by created_at desc"
    );
    state.metrics.inc_db_query();
    let entries = sqlx::query_as::<_, UsaccLedgerEntriesRow>(&sql)
        .bind(&case_id)
        .fetch_all(pool)
        .await?;
    let summary = summarize_ledger(&case_id, &entries);
    Ok(Json(json!({
        "ok": true,
        "summary": summary,
        "entries": entries,
    })))
}

async fn fetch_ledger_entry(state: &AppState, id: &str) -> ApiResult<UsaccLedgerEntriesRow> {
    let pool = db::pool(state)?;
    let sql = format!("{USACC_LEDGER_ENTRIES_SELECT_SQL} where id = $1::uuid");
    state.metrics.inc_db_query();
    Ok(sqlx::query_as::<_, UsaccLedgerEntriesRow>(&sql)
        .bind(id)
        .fetch_one(pool)
        .await?)
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
        body.case_id,
        body.election_id,
        body.vote_id,
        request_id,
        body.operation_kind
            .unwrap_or_else(|| "validate_envelope".to_string()),
        &body.envelope,
        &response,
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
        body.case_id,
        None,
        None,
        request_id,
        "simulate_transaction".to_string(),
        &body.payload,
        &response,
    )
    .await?;
    Ok(Json(json!({ "ok": true, "contract": response })))
}

async fn persist_contract_operation(
    state: &AppState,
    case_id: Option<String>,
    election_id: Option<String>,
    vote_id: Option<String>,
    request_id: String,
    operation_kind: String,
    envelope: &Value,
    response: &Value,
) -> ApiResult<()> {
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
    let sql = format!(
        "insert into {USACC_CONTRACT_OPERATIONS_TABLE} \
         (case_id, election_id, vote_id, request_id, operation_kind, status, program_id, digest, envelope, response) \
         values ($1::uuid, $2::uuid, $3::uuid, $4, $5, $6, $7, $8, $9::jsonb, $10::jsonb) \
         on conflict (request_id) do update set status = excluded.status, digest = excluded.digest, response = excluded.response, updated_at = now()"
    );
    state.metrics.inc_db_query();
    sqlx::query(&sql)
        .bind(case_id)
        .bind(election_id)
        .bind(vote_id)
        .bind(request_id)
        .bind(operation_kind)
        .bind(status)
        .bind(program_id)
        .bind(digest)
        .bind(envelope.clone())
        .bind(response.clone())
        .execute(pool)
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
            let sql = format!(
                "insert into {USACC_SIMULATION_RUNS_TABLE} \
                 (case_id, status, mode, seed, horizon_days, actor_count, event_count, metrics, trace, input, started_at, finished_at) \
                 values ($1::uuid, 'succeeded', 'sim', $2, $3, $4, $5, $6::jsonb, $7::jsonb, $8::jsonb, now(), now()) \
                 returning id::text"
            );
            state.metrics.inc_db_query();
            let id = sqlx::query_scalar::<_, String>(&sql)
                .bind(response.case_id.clone())
                .bind(seed_i64)
                .bind(response.horizon_days)
                .bind(response.actor_count)
                .bind(response.event_count.min(i32::MAX as u64) as i32)
                .bind(response.metrics.clone())
                .bind(response.trace.clone())
                .bind(input)
                .fetch_one(pool)
                .await?;
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
    let sql = format!("{USACC_SIMULATION_RUNS_SELECT_SQL} where id = $1::uuid");
    state.metrics.inc_db_query();
    let row = sqlx::query_as::<_, UsaccSimulationRunsRow>(&sql)
        .bind(id)
        .fetch_one(pool)
        .await?;
    Ok(Json(row))
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
    use super::*;

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
