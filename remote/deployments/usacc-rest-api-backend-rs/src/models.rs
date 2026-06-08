use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

impl PageQuery {
    pub fn limit(&self, max: i64) -> i64 {
        self.limit.unwrap_or(50).clamp(1, max)
    }

    pub fn offset(&self) -> i64 {
        self.offset.unwrap_or(0).max(0)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateUserRequest {
    pub external_subject: Option<String>,
    pub email_hash: Option<String>,
    pub display_name: String,
    pub user_kind: Option<String>,
    pub status: Option<String>,
    pub kyc_level: Option<String>,
    pub roles: Option<Value>,
    pub is_legal_entity: Option<bool>,
    pub legal_region: Option<String>,
    pub meta_data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchUserRequest {
    pub display_name: Option<String>,
    pub status: Option<String>,
    pub kyc_level: Option<String>,
    pub roles: Option<Value>,
    pub legal_region: Option<String>,
    pub meta_data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateCaseRequest {
    pub case_number: String,
    pub title: String,
    pub status: Option<String>,
    pub filing_tier: Option<String>,
    pub plaintiff_user_id: Option<String>,
    pub defendant_summary: String,
    pub conduct_summary: String,
    pub conduct_fingerprint: Option<String>,
    pub conduct_window_start: Option<String>,
    pub conduct_window_end: Option<String>,
    pub priority_score_micros: Option<i32>,
    pub meta_data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchCaseRequest {
    pub title: Option<String>,
    pub status: Option<String>,
    pub filing_tier: Option<String>,
    pub priority_score_micros: Option<i32>,
    pub meta_data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateStageRequest {
    pub stage_key: String,
    pub stage_order: i32,
    pub title: String,
    pub status: Option<String>,
    pub assigned_user_id: Option<String>,
    pub decision_summary: Option<String>,
    pub meta_data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateElectionRequest {
    pub case_id: Option<String>,
    pub stage_id: Option<String>,
    pub election_kind: String,
    pub title: String,
    pub status: Option<String>,
    pub quorum_count: Option<i32>,
    pub threshold_micros: Option<i32>,
    pub meta_data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CastVoteRequest {
    pub case_id: Option<String>,
    pub voter_user_id: String,
    pub vote_kind: Option<String>,
    pub vote_value: String,
    pub weight_micros: Option<i32>,
    pub commitment_hash: Option<String>,
    pub sealed_payload: Option<Value>,
    pub contract_envelope: Option<Value>,
    pub meta_data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerEntryRequest {
    pub case_id: Option<String>,
    pub escrow_account_id: Option<String>,
    pub user_id: Option<String>,
    pub entry_kind: String,
    pub direction: String,
    pub amount_cents: i64,
    pub currency: Option<String>,
    pub provider_ref: Option<String>,
    pub contract_digest: Option<String>,
    pub meta_data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractProxyRequest {
    pub case_id: Option<String>,
    pub election_id: Option<String>,
    pub vote_id: Option<String>,
    pub request_id: Option<String>,
    pub operation_kind: Option<String>,
    pub envelope: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SimulateTransactionProxyRequest {
    pub case_id: Option<String>,
    pub request_id: Option<String>,
    pub payload: Value,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SimulationRunRequest {
    pub case_id: Option<String>,
    pub seed: Option<u64>,
    pub horizon_days: Option<i32>,
    pub actor_count: Option<i32>,
    pub target_signatures: Option<u32>,
    pub sponsor_response_rate: Option<f64>,
    pub admission_approval_rate: Option<f64>,
    pub judge_conviction_rate: Option<f64>,
    pub panel_size: Option<u32>,
    pub conviction_threshold_count: Option<u32>,
    pub persist: Option<bool>,
    pub input: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SimulationRunResponse {
    pub ok: bool,
    pub persisted: bool,
    pub run_id: Option<String>,
    pub case_id: Option<String>,
    pub seed: u64,
    pub horizon_days: i32,
    pub actor_count: i32,
    pub event_count: u64,
    pub metrics: Value,
    pub trace: Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerSummary {
    pub case_id: String,
    pub currency: String,
    pub debits_cents: i64,
    pub credits_cents: i64,
    pub net_cents: i64,
    pub pledge_cents: i64,
    pub capture_cents: i64,
    pub refund_cents: i64,
    pub disbursement_cents: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TallyChoice {
    pub vote_value: String,
    pub vote_count: i64,
    pub weight_micros: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TallyResponse {
    pub ok: bool,
    pub election_id: String,
    pub total_votes: i64,
    pub total_weight_micros: i64,
    pub threshold_micros: i32,
    pub winning_value: Option<String>,
    pub passed: bool,
    pub choices: Vec<TallyChoice>,
}

pub fn json_object_or_default(value: Option<Value>) -> Value {
    match value {
        Some(value @ Value::Object(_)) => value,
        Some(_) | None => serde_json::json!({}),
    }
}
