use dd_pg_defs::{
    UsaccCaseStagesRow, UsaccCasesRow, UsaccElectionsRow, UsaccLedgerEntriesRow,
    UsaccSimulationRunsRow, UsaccUsersRow, UsaccVotesRow,
};
use dd_pg_defs_sea_orm::{
    UsaccCaseStagesModel, UsaccCasesModel, UsaccElectionsModel, UsaccLedgerEntriesModel,
    UsaccSimulationRunsModel, UsaccUsersModel, UsaccVotesModel,
};
use sea_orm::prelude::{DateTimeWithTimeZone, Uuid};
use sea_orm::{ConnectOptions, Database, DatabaseConnection};

use crate::{
    config::Config,
    error::{ApiError, ApiResult},
    state::AppState,
};

pub async fn connect(config: &Config) -> Result<Option<DatabaseConnection>, sea_orm::DbErr> {
    let Some(database_url) = config.database_url.as_ref() else {
        return Ok(None);
    };

    let mut options = ConnectOptions::new(database_url);
    options
        .max_connections(5)
        .acquire_timeout(config.request_timeout);
    let pool = Database::connect(options).await?;

    Ok(Some(pool))
}

pub fn pool(state: &AppState) -> ApiResult<&DatabaseConnection> {
    state.pool.as_ref().ok_or_else(|| {
        state.metrics.inc_db_error();
        ApiError::unavailable(
            "USACC database is not configured; set USACC_DATABASE_URL for database-backed routes",
        )
    })
}

/// Parse an id the database previously coerced via a `$n::uuid` bind cast,
/// keeping the same 500 surface an invalid uuid produced under sqlx.
pub fn parse_uuid(value: &str) -> ApiResult<Uuid> {
    Uuid::parse_str(value).map_err(|error| {
        ApiError::internal(format!("database error: invalid uuid {value:?}: {error}"))
    })
}

/// Mirror the generated `*_SELECT_SQL` timestamp shape,
/// `to_char(x at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS"Z"')`.
fn ts(value: &DateTimeWithTimeZone) -> String {
    value.naive_utc().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn ts_opt(value: &Option<DateTimeWithTimeZone>) -> Option<String> {
    value.as_ref().map(ts)
}

fn id_opt(value: &Option<Uuid>) -> Option<String> {
    value.as_ref().map(Uuid::to_string)
}

// SeaORM entity models carry native uuid/timestamptz values; the API and
// console keep responding with the generated `*Row` text shapes (uuid::text
// ids, `YYYY-MM-DDTHH:MI:SSZ` timestamps), so each model is converted here.

pub fn user_row(model: UsaccUsersModel) -> UsaccUsersRow {
    UsaccUsersRow {
        id: model.id.to_string(),
        external_subject: model.external_subject,
        email_hash: model.email_hash,
        display_name: model.display_name,
        user_kind: model.user_kind,
        status: model.status,
        kyc_level: model.kyc_level,
        roles: model.roles,
        is_legal_entity: model.is_legal_entity,
        legal_region: model.legal_region,
        meta_data: model.meta_data,
        created_at: ts(&model.created_at),
        updated_at: ts(&model.updated_at),
    }
}

pub fn case_row(model: UsaccCasesModel) -> UsaccCasesRow {
    UsaccCasesRow {
        id: model.id.to_string(),
        case_number: model.case_number,
        title: model.title,
        status: model.status,
        filing_tier: model.filing_tier,
        plaintiff_user_id: id_opt(&model.plaintiff_user_id),
        defendant_summary: model.defendant_summary,
        conduct_summary: model.conduct_summary,
        conduct_fingerprint: model.conduct_fingerprint,
        conduct_window_start: model.conduct_window_start,
        conduct_window_end: model.conduct_window_end,
        priority_score_micros: model.priority_score_micros,
        meta_data: model.meta_data,
        opened_at: ts_opt(&model.opened_at),
        closed_at: ts_opt(&model.closed_at),
        created_at: ts(&model.created_at),
        updated_at: ts(&model.updated_at),
    }
}

pub fn case_stage_row(model: UsaccCaseStagesModel) -> UsaccCaseStagesRow {
    UsaccCaseStagesRow {
        id: model.id.to_string(),
        case_id: model.case_id.to_string(),
        stage_key: model.stage_key,
        stage_order: model.stage_order,
        title: model.title,
        status: model.status,
        assigned_user_id: id_opt(&model.assigned_user_id),
        opened_at: ts_opt(&model.opened_at),
        due_at: ts_opt(&model.due_at),
        closed_at: ts_opt(&model.closed_at),
        decision_summary: model.decision_summary,
        meta_data: model.meta_data,
        created_at: ts(&model.created_at),
        updated_at: ts(&model.updated_at),
    }
}

pub fn election_row(model: UsaccElectionsModel) -> UsaccElectionsRow {
    UsaccElectionsRow {
        id: model.id.to_string(),
        case_id: id_opt(&model.case_id),
        stage_id: id_opt(&model.stage_id),
        election_kind: model.election_kind,
        title: model.title,
        status: model.status,
        quorum_count: model.quorum_count,
        threshold_micros: model.threshold_micros,
        opens_at: ts_opt(&model.opens_at),
        closes_at: ts_opt(&model.closes_at),
        sealed_until: ts_opt(&model.sealed_until),
        tally: model.tally,
        meta_data: model.meta_data,
        created_at: ts(&model.created_at),
        updated_at: ts(&model.updated_at),
    }
}

pub fn vote_row(model: UsaccVotesModel) -> UsaccVotesRow {
    UsaccVotesRow {
        id: model.id.to_string(),
        election_id: model.election_id.to_string(),
        case_id: id_opt(&model.case_id),
        voter_user_id: model.voter_user_id.to_string(),
        vote_kind: model.vote_kind,
        vote_value: model.vote_value,
        weight_micros: model.weight_micros,
        commitment_hash: model.commitment_hash,
        sealed_payload: model.sealed_payload,
        revealed_at: ts_opt(&model.revealed_at),
        contract_digest: model.contract_digest,
        meta_data: model.meta_data,
        created_at: ts(&model.created_at),
        updated_at: ts(&model.updated_at),
    }
}

pub fn ledger_entry_row(model: UsaccLedgerEntriesModel) -> UsaccLedgerEntriesRow {
    UsaccLedgerEntriesRow {
        id: model.id.to_string(),
        case_id: id_opt(&model.case_id),
        escrow_account_id: id_opt(&model.escrow_account_id),
        user_id: id_opt(&model.user_id),
        entry_kind: model.entry_kind,
        direction: model.direction,
        amount_cents: model.amount_cents,
        currency: model.currency,
        provider_ref: model.provider_ref,
        contract_digest: model.contract_digest,
        meta_data: model.meta_data,
        created_at: ts(&model.created_at),
    }
}

pub fn simulation_run_row(model: UsaccSimulationRunsModel) -> UsaccSimulationRunsRow {
    UsaccSimulationRunsRow {
        id: model.id.to_string(),
        case_id: id_opt(&model.case_id),
        status: model.status,
        mode: model.mode,
        seed: model.seed,
        horizon_days: model.horizon_days,
        actor_count: model.actor_count,
        event_count: model.event_count,
        metrics: model.metrics,
        trace: model.trace,
        input: model.input,
        started_at: ts_opt(&model.started_at),
        finished_at: ts_opt(&model.finished_at),
        created_at: ts(&model.created_at),
        updated_at: ts(&model.updated_at),
    }
}
