use std::sync::atomic::Ordering;

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Response,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::confirm::{bounded_confirm, normalize_confirm_commitment};
use crate::nats::{
    publish_contract_event, publish_runtime_critical_event, publish_settlement_outcome,
};
use crate::rpc::solana_rpc;
use crate::shared::{explicit_request_id, json_response, now_ms, request_id};
use crate::state::{
    AppState, DEFAULT_CONFIRM_POLL_INTERVAL_MS, DEFAULT_CONFIRM_TIMEOUT_MS, MAX_RATIONALE_BYTES,
    RESOLUTION_SCHEMA_VERSION, SERVICE_NAME, SETTLEMENT_SCHEMA_VERSION,
};
use crate::validation::{
    authorize_settlement, normalize_commitment, normalize_request_cluster, send_params,
    simulate_params, validate_pubkey, validate_signed_transaction, TransactionRpcRequest,
};

// ---------------------------------------------------------------------------
// Settlement and resolution vocabulary (shared with dd-escrow-rs)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SettlementAction {
    Fund,
    Release,
    Refund,
    PartialRelease,
    SplitRelease,
    DisputeAward,
    Expire,
    Cancel,
}

impl SettlementAction {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            SettlementAction::Fund => "fund",
            SettlementAction::Release => "release",
            SettlementAction::Refund => "refund",
            SettlementAction::PartialRelease => "partial-release",
            SettlementAction::SplitRelease => "split-release",
            SettlementAction::DisputeAward => "dispute-award",
            SettlementAction::Expire => "expire",
            SettlementAction::Cancel => "cancel",
        }
    }
}

pub(crate) const SETTLEMENT_ACTIONS: [&str; 8] = [
    "fund",
    "release",
    "refund",
    "partial-release",
    "split-release",
    "dispute-award",
    "expire",
    "cancel",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ResolutionDecision {
    ReleaseToPayee,
    RefundToPayer,
    Split,
    AwardToClaimant,
    Uphold,
    Overturn,
}

impl ResolutionDecision {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            ResolutionDecision::ReleaseToPayee => "release-to-payee",
            ResolutionDecision::RefundToPayer => "refund-to-payer",
            ResolutionDecision::Split => "split",
            ResolutionDecision::AwardToClaimant => "award-to-claimant",
            ResolutionDecision::Uphold => "uphold",
            ResolutionDecision::Overturn => "overturn",
        }
    }

    /// Settlement actions that may legitimately enact a given dispute decision.
    pub(crate) fn allowed_actions(self) -> &'static [SettlementAction] {
        match self {
            ResolutionDecision::ReleaseToPayee => {
                &[SettlementAction::Release, SettlementAction::PartialRelease]
            }
            ResolutionDecision::RefundToPayer => &[SettlementAction::Refund],
            ResolutionDecision::Split => &[SettlementAction::SplitRelease],
            ResolutionDecision::AwardToClaimant => &[SettlementAction::DisputeAward],
            ResolutionDecision::Uphold => &[
                SettlementAction::Release,
                SettlementAction::PartialRelease,
                SettlementAction::SplitRelease,
                SettlementAction::DisputeAward,
            ],
            ResolutionDecision::Overturn => &[
                SettlementAction::Refund,
                SettlementAction::SplitRelease,
                SettlementAction::DisputeAward,
            ],
        }
    }
}

pub(crate) const RESOLUTION_DECISIONS: [&str; 6] = [
    "release-to-payee",
    "refund-to-payer",
    "split",
    "award-to-claimant",
    "uphold",
    "overturn",
];

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConfirmOptions {
    pub(crate) target_commitment: Option<String>,
    pub(crate) timeout_ms: Option<u64>,
    pub(crate) poll_interval_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SettlementRequest {
    pub(crate) schema_version: String,
    pub(crate) request_id: Option<String>,
    pub(crate) cluster: Option<String>,
    pub(crate) contract_id: Option<String>,
    pub(crate) escrow_id: Option<String>,
    pub(crate) action: SettlementAction,
    pub(crate) transaction: String,
    pub(crate) encoding: Option<String>,
    pub(crate) commitment: Option<String>,
    pub(crate) skip_preflight: Option<bool>,
    pub(crate) max_retries: Option<usize>,
    pub(crate) min_context_slot: Option<u64>,
    pub(crate) confirm: Option<ConfirmOptions>,
    pub(crate) intent_digest: Option<String>,
    pub(crate) memo: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResolutionRequest {
    pub(crate) schema_version: String,
    pub(crate) request_id: Option<String>,
    pub(crate) cluster: Option<String>,
    pub(crate) dispute_id: Option<String>,
    pub(crate) escrow_id: Option<String>,
    pub(crate) decision: ResolutionDecision,
    pub(crate) action: SettlementAction,
    pub(crate) arbiter: Option<String>,
    pub(crate) arbiter_required_signer: Option<bool>,
    pub(crate) transaction: String,
    pub(crate) encoding: Option<String>,
    pub(crate) commitment: Option<String>,
    pub(crate) skip_preflight: Option<bool>,
    pub(crate) max_retries: Option<usize>,
    pub(crate) min_context_slot: Option<u64>,
    pub(crate) confirm: Option<ConfirmOptions>,
    pub(crate) rationale: Option<String>,
}

/// Common fields needed to drive simulate/send for a settlement-style request.
pub(crate) struct SettlementCore {
    pub(crate) request_id: Option<String>,
    pub(crate) cluster: Option<String>,
    pub(crate) transaction: String,
    pub(crate) encoding: Option<String>,
    pub(crate) commitment: Option<String>,
    pub(crate) skip_preflight: Option<bool>,
    pub(crate) max_retries: Option<usize>,
    pub(crate) min_context_slot: Option<u64>,
}

impl SettlementCore {
    /// Builds a TransactionRpcRequest so settlement paths reuse the audited
    /// validate/simulate/send helpers. `for_simulate` flips on
    /// replaceRecentBlockhash so dry-runs don't need a fresh blockhash.
    pub(crate) fn tx_request(&self, for_simulate: bool) -> TransactionRpcRequest {
        TransactionRpcRequest {
            request_id: self.request_id.clone(),
            cluster: self.cluster.clone(),
            transaction: self.transaction.clone(),
            encoding: self.encoding.clone(),
            commitment: self.commitment.clone(),
            sig_verify: Some(false),
            replace_recent_blockhash: Some(for_simulate),
            skip_preflight: self.skip_preflight,
            max_retries: self.max_retries,
            min_context_slot: self.min_context_slot,
        }
    }
}

impl SettlementRequest {
    pub(crate) fn core(&self) -> SettlementCore {
        SettlementCore {
            request_id: self.request_id.clone(),
            cluster: self.cluster.clone(),
            transaction: self.transaction.clone(),
            encoding: self.encoding.clone(),
            commitment: self.commitment.clone(),
            skip_preflight: self.skip_preflight,
            max_retries: self.max_retries,
            min_context_slot: self.min_context_slot,
        }
    }
}

impl ResolutionRequest {
    pub(crate) fn core(&self) -> SettlementCore {
        SettlementCore {
            request_id: self.request_id.clone(),
            cluster: self.cluster.clone(),
            transaction: self.transaction.clone(),
            encoding: self.encoding.clone(),
            commitment: self.commitment.clone(),
            skip_preflight: self.skip_preflight,
            max_retries: self.max_retries,
            min_context_slot: self.min_context_slot,
        }
    }
}

pub(crate) fn resolve_confirm_target(
    options: &Option<ConfirmOptions>,
) -> Result<(String, u64, u64), String> {
    let (target, timeout_ms, poll_interval_ms) = match options {
        Some(options) => (
            normalize_confirm_commitment(options.target_commitment.as_deref())?,
            options.timeout_ms.unwrap_or(DEFAULT_CONFIRM_TIMEOUT_MS),
            options
                .poll_interval_ms
                .unwrap_or(DEFAULT_CONFIRM_POLL_INTERVAL_MS),
        ),
        None => (
            "confirmed".to_string(),
            DEFAULT_CONFIRM_TIMEOUT_MS,
            DEFAULT_CONFIRM_POLL_INTERVAL_MS,
        ),
    };
    Ok((target, timeout_ms, poll_interval_ms))
}

/// Shared validation for the settlement-style transaction core. Returns the
/// validated encoding plus decoded byte length, or a list of errors.
pub(crate) fn validate_settlement_core(
    core: &SettlementCore,
    default_cluster: &str,
) -> Result<(String, &'static str, usize), Vec<String>> {
    let mut errors = Vec::new();
    let tx = core.tx_request(false);
    let cluster = match normalize_request_cluster(core.cluster.as_deref(), default_cluster) {
        Ok(cluster) => cluster,
        Err(error) => {
            errors.push(error);
            default_cluster.to_string()
        }
    };
    if let Err(error) = normalize_commitment(core.commitment.as_deref()) {
        errors.push(error);
    }
    match validate_signed_transaction(&tx) {
        Ok((encoding, decoded_len)) => {
            if errors.is_empty() {
                Ok((cluster, encoding, decoded_len))
            } else {
                Err(errors)
            }
        }
        Err(error) => {
            errors.push(error);
            Err(errors)
        }
    }
}

pub(crate) async fn simulate_settlement_http(
    State(state): State<AppState>,
    Json(request): Json<SettlementRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if request.schema_version != SETTLEMENT_SCHEMA_VERSION {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "ok": false, "error": format!("schemaVersion must be {SETTLEMENT_SCHEMA_VERSION}") }),
        );
    }
    let core = request.core();
    let (cluster, encoding, decoded_bytes) =
        match validate_settlement_core(&core, &state.default_cluster) {
            Ok(validated) => validated,
            Err(errors) => {
                state
                    .metrics
                    .policy_rejections_total
                    .fetch_add(1, Ordering::Relaxed);
                return json_response(
                    StatusCode::BAD_REQUEST,
                    json!({ "ok": false, "errors": errors }),
                );
            }
        };
    let tx = core.tx_request(true);
    let params = match simulate_params(&tx, encoding) {
        Ok(params) => params,
        Err(error) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "ok": false, "error": error }),
            )
        }
    };
    match solana_rpc(&state, "simulateTransaction", params).await {
        Ok(result) => json_response(
            StatusCode::OK,
            json!({
                "ok": true,
                "requestId": request_id(request.request_id.as_ref(), "contract-settlement-simulate"),
                "schemaVersion": SETTLEMENT_SCHEMA_VERSION,
                "cluster": cluster,
                "action": request.action.as_str(),
                "encoding": encoding,
                "transactionBytes": decoded_bytes,
                "result": result,
                "generatedAtMs": now_ms()
            }),
        ),
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            json_response(
                StatusCode::BAD_GATEWAY,
                json!({ "ok": false, "error": error }),
            )
        }
    }
}

pub(crate) async fn settle_http(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(request): Json<SettlementRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .settlements_total
        .fetch_add(1, Ordering::Relaxed);

    let req_id = request_id(request.request_id.as_ref(), "contract-settlement");

    if !state.settlement_enabled {
        state
            .metrics
            .policy_rejections_total
            .fetch_add(1, Ordering::Relaxed);
        return json_response(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({
                "ok": false,
                "requestId": req_id,
                "error": "settlement is disabled; set SOLANA_SETTLEMENT_ENABLED=true to permit /settle",
                "generatedAtMs": now_ms()
            }),
        );
    }
    if let Err((status, error)) = authorize_settlement(&headers, &state) {
        state
            .metrics
            .send_auth_failures_total
            .fetch_add(1, Ordering::Relaxed);
        state
            .metrics
            .policy_rejections_total
            .fetch_add(1, Ordering::Relaxed);
        return json_response(
            status,
            json!({ "ok": false, "requestId": req_id, "error": error }),
        );
    }
    if request.schema_version != SETTLEMENT_SCHEMA_VERSION {
        state
            .metrics
            .settlement_errors_total
            .fetch_add(1, Ordering::Relaxed);
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "ok": false, "requestId": req_id, "error": format!("schemaVersion must be {SETTLEMENT_SCHEMA_VERSION}") }),
        );
    }
    let core = request.core();
    let (cluster, encoding, decoded_bytes) =
        match validate_settlement_core(&core, &state.default_cluster) {
            Ok(validated) => validated,
            Err(errors) => {
                state
                    .metrics
                    .settlement_errors_total
                    .fetch_add(1, Ordering::Relaxed);
                state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                return json_response(
                    StatusCode::BAD_REQUEST,
                    json!({ "ok": false, "requestId": req_id, "errors": errors }),
                );
            }
        };
    let (confirm_target, confirm_timeout, confirm_interval) =
        match resolve_confirm_target(&request.confirm) {
            Ok(values) => values,
            Err(error) => {
                return json_response(
                    StatusCode::BAD_REQUEST,
                    json!({ "ok": false, "requestId": req_id, "error": error }),
                )
            }
        };

    // Idempotency: only an explicitly provided request id guards a broadcast.
    let idem_key =
        explicit_request_id(request.request_id.as_ref()).map(|key| format!("settle:{key}"));
    if let Some(key) = &idem_key {
        if !state.claim_idempotency_key(key) {
            state
                .metrics
                .settlement_idempotent_hits_total
                .fetch_add(1, Ordering::Relaxed);
            return json_response(
                StatusCode::CONFLICT,
                json!({
                    "ok": false,
                    "requestId": req_id,
                    "error": "duplicate settlement requestId within the idempotency window; broadcast suppressed",
                    "idempotent": true,
                    "generatedAtMs": now_ms()
                }),
            );
        }
    }
    let release = |state: &AppState| {
        if let Some(key) = &idem_key {
            state.release_idempotency_key(key);
        }
    };

    let tx = core.tx_request(false);
    let send = match send_params(&tx, encoding, state.allow_skip_preflight) {
        Ok(params) => params,
        Err(error) => {
            release(&state);
            state
                .metrics
                .settlement_errors_total
                .fetch_add(1, Ordering::Relaxed);
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "ok": false, "requestId": req_id, "error": error }),
            );
        }
    };
    let signature_value = match solana_rpc(&state, "sendTransaction", send).await {
        Ok(value) => value,
        Err(error) => {
            release(&state);
            state
                .metrics
                .settlement_errors_total
                .fetch_add(1, Ordering::Relaxed);
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            publish_runtime_critical_event(
                &state,
                "contract-settlement-send-failed",
                "Settlement sendTransaction failed.",
                json!({ "requestId": req_id, "action": request.action.as_str(), "error": error }),
            )
            .await;
            return json_response(
                StatusCode::BAD_GATEWAY,
                json!({ "ok": false, "requestId": req_id, "error": error }),
            );
        }
    };
    let signature = signature_value.as_str().unwrap_or_default().to_string();
    if signature.is_empty() {
        release(&state);
        state
            .metrics
            .settlement_errors_total
            .fetch_add(1, Ordering::Relaxed);
        return json_response(
            StatusCode::BAD_GATEWAY,
            json!({ "ok": false, "requestId": req_id, "error": "sendTransaction did not return a signature" }),
        );
    }
    let confirmation = bounded_confirm(
        &state,
        &signature,
        &confirm_target,
        confirm_timeout,
        confirm_interval,
    )
    .await;

    let outcome = json!({
        "messageKind": "solana.settlement.outcome",
        "source": SERVICE_NAME,
        "ok": confirmation.reached,
        "requestId": req_id,
        "schemaVersion": SETTLEMENT_SCHEMA_VERSION,
        "cluster": cluster,
        "kind": "settlement",
        "action": request.action.as_str(),
        "contractId": request.contract_id,
        "escrowId": request.escrow_id,
        "intentDigest": request.intent_digest,
        "memo": request.memo,
        "encoding": encoding,
        "transactionBytes": decoded_bytes,
        "signature": signature,
        "confirmation": confirmation,
        "generatedAtMs": now_ms()
    });
    publish_settlement_outcome(&state, outcome.clone()).await;
    publish_contract_event(
        &state,
        "solana.contract.settlement",
        &req_id,
        confirmation.reached,
    )
    .await;
    json_response(StatusCode::OK, outcome)
}

pub(crate) async fn resolve_http(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(request): Json<ResolutionRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .resolutions_total
        .fetch_add(1, Ordering::Relaxed);

    let req_id = request_id(request.request_id.as_ref(), "contract-resolution");

    if !state.resolution_enabled {
        state
            .metrics
            .policy_rejections_total
            .fetch_add(1, Ordering::Relaxed);
        return json_response(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({
                "ok": false,
                "requestId": req_id,
                "error": "resolution is disabled; set SOLANA_RESOLUTION_ENABLED=true to permit /resolve",
                "generatedAtMs": now_ms()
            }),
        );
    }
    if let Err((status, error)) = authorize_settlement(&headers, &state) {
        state
            .metrics
            .send_auth_failures_total
            .fetch_add(1, Ordering::Relaxed);
        state
            .metrics
            .policy_rejections_total
            .fetch_add(1, Ordering::Relaxed);
        return json_response(
            status,
            json!({ "ok": false, "requestId": req_id, "error": error }),
        );
    }

    let mut errors = Vec::new();
    if request.schema_version != RESOLUTION_SCHEMA_VERSION {
        errors.push(format!("schemaVersion must be {RESOLUTION_SCHEMA_VERSION}"));
    }
    if !request.decision.allowed_actions().contains(&request.action) {
        errors.push(format!(
            "decision {} does not permit settlement action {}",
            request.decision.as_str(),
            request.action.as_str()
        ));
    }
    if let Some(arbiter) = &request.arbiter {
        if let Err(error) = validate_pubkey(arbiter, "arbiter") {
            errors.push(error);
        }
    } else if request.arbiter_required_signer == Some(true) {
        errors.push("arbiter pubkey is required when arbiterRequiredSigner is true".to_string());
    }
    if let Some(rationale) = &request.rationale {
        if rationale.len() > MAX_RATIONALE_BYTES {
            errors.push(format!(
                "rationale must be at most {MAX_RATIONALE_BYTES} bytes"
            ));
        }
    }
    let core = request.core();
    let (cluster, encoding, decoded_bytes) =
        match validate_settlement_core(&core, &state.default_cluster) {
            Ok(validated) => validated,
            Err(core_errors) => {
                errors.extend(core_errors);
                (state.default_cluster.clone(), "base64", 0)
            }
        };
    let (confirm_target, confirm_timeout, confirm_interval) =
        match resolve_confirm_target(&request.confirm) {
            Ok(values) => values,
            Err(error) => {
                errors.push(error);
                (
                    "confirmed".to_string(),
                    DEFAULT_CONFIRM_TIMEOUT_MS,
                    DEFAULT_CONFIRM_POLL_INTERVAL_MS,
                )
            }
        };
    if !errors.is_empty() {
        state
            .metrics
            .resolution_errors_total
            .fetch_add(1, Ordering::Relaxed);
        state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "ok": false, "requestId": req_id, "errors": errors }),
        );
    }

    let idem_key =
        explicit_request_id(request.request_id.as_ref()).map(|key| format!("resolve:{key}"));
    if let Some(key) = &idem_key {
        if !state.claim_idempotency_key(key) {
            state
                .metrics
                .settlement_idempotent_hits_total
                .fetch_add(1, Ordering::Relaxed);
            return json_response(
                StatusCode::CONFLICT,
                json!({
                    "ok": false,
                    "requestId": req_id,
                    "error": "duplicate resolution requestId within the idempotency window; broadcast suppressed",
                    "idempotent": true,
                    "generatedAtMs": now_ms()
                }),
            );
        }
    }
    let release = |state: &AppState| {
        if let Some(key) = &idem_key {
            state.release_idempotency_key(key);
        }
    };

    let tx = core.tx_request(false);
    let send = match send_params(&tx, encoding, state.allow_skip_preflight) {
        Ok(params) => params,
        Err(error) => {
            release(&state);
            state
                .metrics
                .resolution_errors_total
                .fetch_add(1, Ordering::Relaxed);
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "ok": false, "requestId": req_id, "error": error }),
            );
        }
    };
    let signature_value = match solana_rpc(&state, "sendTransaction", send).await {
        Ok(value) => value,
        Err(error) => {
            release(&state);
            state
                .metrics
                .resolution_errors_total
                .fetch_add(1, Ordering::Relaxed);
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            publish_runtime_critical_event(
                &state,
                "contract-resolution-send-failed",
                "Resolution sendTransaction failed.",
                json!({ "requestId": req_id, "decision": request.decision.as_str(), "action": request.action.as_str(), "error": error }),
            )
            .await;
            return json_response(
                StatusCode::BAD_GATEWAY,
                json!({ "ok": false, "requestId": req_id, "error": error }),
            );
        }
    };
    let signature = signature_value.as_str().unwrap_or_default().to_string();
    if signature.is_empty() {
        release(&state);
        state
            .metrics
            .resolution_errors_total
            .fetch_add(1, Ordering::Relaxed);
        return json_response(
            StatusCode::BAD_GATEWAY,
            json!({ "ok": false, "requestId": req_id, "error": "sendTransaction did not return a signature" }),
        );
    }
    let confirmation = bounded_confirm(
        &state,
        &signature,
        &confirm_target,
        confirm_timeout,
        confirm_interval,
    )
    .await;

    let outcome = json!({
        "messageKind": "solana.resolution.outcome",
        "source": SERVICE_NAME,
        "ok": confirmation.reached,
        "requestId": req_id,
        "schemaVersion": RESOLUTION_SCHEMA_VERSION,
        "cluster": cluster,
        "kind": "resolution",
        "decision": request.decision.as_str(),
        "action": request.action.as_str(),
        "disputeId": request.dispute_id,
        "escrowId": request.escrow_id,
        "arbiter": request.arbiter,
        "encoding": encoding,
        "transactionBytes": decoded_bytes,
        "signature": signature,
        "confirmation": confirmation,
        "generatedAtMs": now_ms()
    });
    publish_settlement_outcome(&state, outcome.clone()).await;
    publish_contract_event(
        &state,
        "solana.contract.resolution",
        &req_id,
        confirmation.reached,
    )
    .await;
    json_response(StatusCode::OK, outcome)
}
