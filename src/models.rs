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

#[derive(Debug, Serialize, sea_orm::FromQueryResult)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn page_query_limit_defaults_and_clamps() {
        let q: PageQuery = serde_json::from_value(json!({})).unwrap();
        assert_eq!(q.limit(200), 50);

        let q: PageQuery = serde_json::from_value(json!({ "limit": 0 })).unwrap();
        assert_eq!(q.limit(200), 1);

        let q: PageQuery = serde_json::from_value(json!({ "limit": -10 })).unwrap();
        assert_eq!(q.limit(200), 1);

        let q: PageQuery = serde_json::from_value(json!({ "limit": 9999 })).unwrap();
        assert_eq!(q.limit(200), 200);
    }

    #[test]
    fn page_query_offset_defaults_and_floors_at_zero() {
        let q: PageQuery = serde_json::from_value(json!({})).unwrap();
        assert_eq!(q.offset(), 0);

        let q: PageQuery = serde_json::from_value(json!({ "offset": -3 })).unwrap();
        assert_eq!(q.offset(), 0);

        let q: PageQuery = serde_json::from_value(json!({ "offset": 120 })).unwrap();
        assert_eq!(q.offset(), 120);
    }

    #[test]
    fn create_user_request_deserializes_camel_case() {
        let req: CreateUserRequest = serde_json::from_value(json!({
            "externalSubject": "sub-1",
            "displayName": "Ada",
            "userKind": "citizen",
            "isLegalEntity": false,
            "legalRegion": "US-NY",
            "metaData": { "k": "v" }
        }))
        .unwrap();
        assert_eq!(req.external_subject.as_deref(), Some("sub-1"));
        assert_eq!(req.display_name, "Ada");
        assert_eq!(req.user_kind.as_deref(), Some("citizen"));
        assert_eq!(req.is_legal_entity, Some(false));
        assert_eq!(req.meta_data, Some(json!({ "k": "v" })));
    }

    #[test]
    fn create_user_request_requires_display_name() {
        let result: Result<CreateUserRequest, _> =
            serde_json::from_value(json!({ "externalSubject": "sub-1" }));
        assert!(result.is_err());
    }

    #[test]
    fn cast_vote_request_deserializes_optional_fields() {
        let req: CastVoteRequest = serde_json::from_value(json!({
            "voterUserId": "user-9",
            "voteValue": "guilty",
            "weightMicros": 1_000_000,
            "sealedPayload": { "cipher": "abc" }
        }))
        .unwrap();
        assert_eq!(req.voter_user_id, "user-9");
        assert_eq!(req.vote_value, "guilty");
        assert_eq!(req.weight_micros, Some(1_000_000));
        assert!(req.vote_kind.is_none());
        assert!(req.contract_envelope.is_none());
    }

    #[test]
    fn ledger_entry_request_deserializes_amount_cents() {
        let req: LedgerEntryRequest = serde_json::from_value(json!({
            "entryKind": "pledge",
            "direction": "debit",
            "amountCents": -2_500_000_000i64
        }))
        .unwrap();
        assert_eq!(req.entry_kind, "pledge");
        assert_eq!(req.direction, "debit");
        assert_eq!(req.amount_cents, -2_500_000_000);
        assert!(req.currency.is_none());
    }

    #[test]
    fn simulation_run_response_serializes_camel_case() {
        let response = SimulationRunResponse {
            ok: true,
            persisted: false,
            run_id: Some("run-1".to_string()),
            case_id: None,
            seed: 42,
            horizon_days: 180,
            actor_count: 100,
            event_count: 7,
            metrics: json!({}),
            trace: json!([]),
        };
        let value = serde_json::to_value(&response).unwrap();
        assert_eq!(value["runId"], "run-1");
        assert_eq!(value["horizonDays"], 180);
        assert_eq!(value["actorCount"], 100);
        assert_eq!(value["eventCount"], 7);
        assert!(value.get("run_id").is_none());
    }

    #[test]
    fn json_object_or_default_passes_objects_through() {
        let obj = json!({ "a": 1 });
        assert_eq!(json_object_or_default(Some(obj.clone())), obj);
    }

    #[test]
    fn json_object_or_default_replaces_non_objects() {
        assert_eq!(json_object_or_default(None), json!({}));
        assert_eq!(json_object_or_default(Some(json!([1, 2]))), json!({}));
        assert_eq!(json_object_or_default(Some(json!("str"))), json!({}));
        assert_eq!(json_object_or_default(Some(json!(null))), json!({}));
    }
}
