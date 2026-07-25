use std::sync::atomic::Ordering;

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

use crate::backends::{
    contract_service_body, contract_service_call, contract_service_health, rpc_json, send_params,
    simulate_params, ContractServiceOp,
};
use crate::config::{
    SettlementBackend, DEFAULT_COMMITMENT, MAX_ESCROW_ID_LEN, MAX_HTTP_BODY_BYTES, MAX_MEMO_BYTES,
    MAX_METADATA_BYTES, MAX_MILESTONES, MAX_PARTIES, MAX_REQUEST_ID_LEN, MAX_SEND_RETRIES,
    MAX_SIGNED_TRANSACTION_BYTES, SCHEMA_VERSION, SERVICE_NAME, SETTLEMENT_AUTH_HEADER,
};
use crate::domain::{
    kind_catalog, kind_spec, AssetType, EscrowKind, PartyRole, ReleaseMode, SettlementAction,
    ESCROW_KINDS,
};
use crate::nats::{publish_escrow_event, publish_runtime_critical_event};
use crate::settlement::{authorize_settlement, validate_resolution, validate_settlement_request};
use crate::state::AppState;
use crate::types::{
    EscrowAsset, EscrowAuditResponse, EscrowIntentRequest, EscrowParty, EscrowSettlementRequest,
    EscrowTerms, ResolutionRequest, ResolutionResponse, SettlementPlan,
};
use crate::util::{json_error, now_ms, now_unix_seconds};
use crate::validation::{
    normalize_commitment, normalize_encoding, normalize_request_cluster, request_id,
    validate_escrow_intent,
};

pub(crate) async fn home() -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": SCHEMA_VERSION,
        "supportedKinds": kind_catalog(),
        "endpoints": {
            "types": "/types",
            "capabilities": "/capabilities",
            "schema": "/schema",
            "example": "/example",
            "validate": "POST /validate",
            "audit": "POST /audit",
            "resolve": "POST /resolve",
            "simulateSettlement": "POST /simulate-settlement",
            "settle": "POST /settle",
            "status": "/status",
            "metrics": "/metrics"
        }
    }))
}

pub(crate) async fn healthz() -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": SCHEMA_VERSION,
    }))
}

pub(crate) async fn types_http() -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "kinds": kind_catalog(),
    }))
}

pub(crate) async fn capabilities_http(State(state): State<AppState>) -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": SCHEMA_VERSION,
        "supportedKinds": kind_catalog(),
        "settlement": {
            "enabled": state.settlement_enabled,
            "requiresIntent": state.settlement_require_intent,
            "authHeader": SETTLEMENT_AUTH_HEADER,
            "skipPreflightAllowed": state.allow_skip_preflight,
            "allowedProgramCount": state.allowed_program_ids.len(),
            "clientSignedTransactionsOnly": true,
            "privateKeysStored": false,
            "backend": state.settlement_backend.as_str(),
            "contractServiceConfigured": state.contract_service_url.is_some(),
            "contractServiceSendConfigured": state.contract_service_send_secret.is_some()
        },
        "resolutionOutcomes": ["release", "refund", "split", "dispute-award", "expire", "cancel"],
        "limits": {
            "maxHttpBodyBytes": MAX_HTTP_BODY_BYTES,
            "maxSignedTransactionBytes": MAX_SIGNED_TRANSACTION_BYTES,
            "maxParties": MAX_PARTIES,
            "maxMilestones": MAX_MILESTONES,
            "maxMemoBytes": MAX_MEMO_BYTES,
            "maxMetadataBytes": MAX_METADATA_BYTES,
            "maxSendRetries": MAX_SEND_RETRIES
        },
        "generatedAtMs": now_ms(),
    }))
}

pub(crate) async fn schema_http() -> impl IntoResponse {
    Json(json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://dd.local/schemas/solana.escrow.v1.json",
        "title": "solana.escrow.v1",
        "type": "object",
        "required": ["schemaVersion", "kind", "escrowId", "parties", "asset", "terms"],
        "properties": {
            "schemaVersion": { "const": SCHEMA_VERSION },
            "requestId": { "type": "string", "maxLength": MAX_REQUEST_ID_LEN },
            "cluster": { "enum": ["mainnet-beta", "devnet", "testnet", "localnet", "custom"] },
            "kind": { "enum": ESCROW_KINDS.iter().map(|kind| kind.as_str()).collect::<Vec<_>>() },
            "escrowId": { "type": "string", "maxLength": MAX_ESCROW_ID_LEN },
            "parties": { "type": "array", "minItems": 2, "maxItems": MAX_PARTIES },
            "asset": { "type": "object" },
            "terms": { "type": "object" },
            "settlementPlan": { "type": "object" },
            "memo": { "type": "string", "maxLength": MAX_MEMO_BYTES },
            "metadata": { "type": "object" }
        }
    }))
}

pub(crate) fn example_request() -> EscrowIntentRequest {
    let system_program = "11111111111111111111111111111111".to_string();
    EscrowIntentRequest {
        schema_version: SCHEMA_VERSION.to_string(),
        request_id: Some("escrow-demo".to_string()),
        cluster: Some("devnet".to_string()),
        kind: EscrowKind::MarketplaceOrder,
        escrow_id: "order.demo.001".to_string(),
        parties: vec![
            EscrowParty {
                role: PartyRole::Buyer,
                pubkey: system_program.clone(),
                label: Some("buyer".to_string()),
                required_signer: Some(true),
                payout_bps: None,
            },
            EscrowParty {
                role: PartyRole::Seller,
                pubkey: system_program.clone(),
                label: Some("seller".to_string()),
                required_signer: Some(false),
                payout_bps: Some(10_000),
            },
        ],
        asset: EscrowAsset {
            asset_type: AssetType::Sol,
            mint: None,
            amount_lamports: Some(1_000_000),
            token_amount: None,
            decimals: None,
            collection: None,
            escrow_vault: Some(system_program.clone()),
        },
        terms: EscrowTerms {
            release_mode: ReleaseMode::BuyerApproval,
            settlement_actions: Some(vec![
                SettlementAction::Fund,
                SettlementAction::Release,
                SettlementAction::Refund,
                SettlementAction::DisputeAward,
            ]),
            dispute_window_seconds: Some(7 * 24 * 60 * 60),
            inspection_period_seconds: Some(48 * 60 * 60),
            timeout_unix_seconds: Some(now_unix_seconds() + 30 * 24 * 60 * 60),
            milestones: None,
            required_approvals: Some(vec![PartyRole::Buyer]),
            max_partial_releases: None,
            delivery_required: Some(true),
        },
        settlement_plan: Some(SettlementPlan {
            program_id: system_program,
            vault_pubkey: None,
            fee_bps: Some(50),
            memo_required: Some(true),
        }),
        memo: Some("example marketplace escrow intent".to_string()),
        metadata: Some(json!({ "source": "dd-escrow-rs-example" })),
    }
}

/// Worked `collab-show` intent: two creators (60/40 revenue split) each stake a
/// commitment bond into a shared vault that also holds a prize pool, with a
/// required arbiter who awards or splits funds on a no-show or rule violation.
/// The per-creator stake, pool size, show date, and rules live in `metadata`;
/// the on-chain program (`settlementPlan.programId`) enforces the amounts.
/// Quoted in the readme and exercised by tests; not served over HTTP.
#[cfg(test)]
pub(crate) fn collab_show_example() -> EscrowIntentRequest {
    let system_program = "11111111111111111111111111111111".to_string();
    let creator_a = system_program.clone();
    let creator_b = system_program.clone();
    let arbiter = system_program.clone();
    let show_date = now_unix_seconds() + 14 * 24 * 60 * 60;
    EscrowIntentRequest {
        schema_version: SCHEMA_VERSION.to_string(),
        request_id: Some("collab-show-demo".to_string()),
        cluster: Some("devnet".to_string()),
        kind: EscrowKind::CollabShow,
        escrow_id: "collab.show.001".to_string(),
        parties: vec![
            EscrowParty {
                role: PartyRole::Creator,
                pubkey: creator_a.clone(),
                label: Some("creator-a".to_string()),
                required_signer: Some(true),
                payout_bps: Some(6_000),
            },
            EscrowParty {
                role: PartyRole::Creator,
                pubkey: creator_b.clone(),
                label: Some("creator-b".to_string()),
                required_signer: Some(true),
                payout_bps: Some(4_000),
            },
            EscrowParty {
                role: PartyRole::Arbitrator,
                pubkey: arbiter,
                label: Some("arbiter".to_string()),
                required_signer: Some(false),
                payout_bps: None,
            },
        ],
        asset: EscrowAsset {
            asset_type: AssetType::Sol,
            mint: None,
            // Total locked value = both creators' stakes + the shared prize pool
            // (2_000_000 + 2_000_000 + 5_000_000), broken down in `metadata`.
            amount_lamports: Some(9_000_000),
            token_amount: None,
            decimals: None,
            collection: None,
            escrow_vault: Some(system_program.clone()),
        },
        terms: EscrowTerms {
            release_mode: ReleaseMode::ArbiterDecision,
            settlement_actions: Some(vec![
                SettlementAction::Fund,
                SettlementAction::SplitRelease,
                SettlementAction::Refund,
                SettlementAction::DisputeAward,
                SettlementAction::Expire,
                SettlementAction::Cancel,
            ]),
            dispute_window_seconds: Some(7 * 24 * 60 * 60),
            inspection_period_seconds: None,
            timeout_unix_seconds: Some(show_date),
            milestones: None,
            required_approvals: Some(vec![PartyRole::Creator]),
            max_partial_releases: None,
            delivery_required: Some(true),
        },
        settlement_plan: Some(SettlementPlan {
            program_id: system_program,
            vault_pubkey: None,
            fee_bps: Some(50),
            memo_required: Some(true),
        }),
        memo: Some("collab show: two creators stake + shared pool, split on success".to_string()),
        metadata: Some(json!({
            "product": "collab-show",
            "showTitle": "Creator A x Creator B live collab",
            "showDateUnix": show_date,
            "stakeLamports": { "creator-a": 2_000_000, "creator-b": 2_000_000 },
            "prizePoolLamports": 5_000_000,
            "revenueSplitBps": { "creator-a": 6_000, "creator-b": 4_000 },
            "rules": [
                {
                    "id": "attendance",
                    "description": "Both creators must join the live show at showDateUnix",
                    "onBreach": "dispute-award-to-other"
                },
                {
                    "id": "conduct",
                    "description": "No content-policy or conduct violations during the show",
                    "onBreach": "arbiter-decision"
                }
            ],
            "arbiterPolicy": "Arbiter rules within disputeWindowSeconds: dispute-award the breacher's stake to the wronged creator, or set a split."
        })),
    }
}

pub(crate) async fn example_http() -> impl IntoResponse {
    Json(example_request())
}

pub(crate) async fn validate_http(
    State(state): State<AppState>,
    Json(request): Json<EscrowIntentRequest>,
) -> Response {
    state
        .metrics
        .validations_total
        .fetch_add(1, Ordering::Relaxed);
    match validate_escrow_intent(&request, &state.default_cluster, &state.allowed_program_ids) {
        Ok(response) => Json(response).into_response(),
        Err(errors) => {
            state
                .metrics
                .validation_errors_total
                .fetch_add(1, Ordering::Relaxed);
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            json_error(
                StatusCode::BAD_REQUEST,
                "escrow intent validation failed",
                json!({ "errors": errors }),
            )
        }
    }
}

pub(crate) async fn audit_http(
    State(state): State<AppState>,
    Json(request): Json<EscrowIntentRequest>,
) -> impl IntoResponse {
    let request_id = request_id(request.request_id.as_ref(), "escrow-audit");
    let cluster = normalize_request_cluster(request.cluster.as_deref(), &state.default_cluster)
        .unwrap_or_else(|_| state.default_cluster.clone());
    match validate_escrow_intent(&request, &state.default_cluster, &state.allowed_program_ids) {
        Ok(validation) => {
            let warnings = validation.warnings.clone();
            Json(EscrowAuditResponse {
                ok: true,
                request_id,
                schema_version: SCHEMA_VERSION,
                cluster,
                escrow_id: request.escrow_id,
                kind: request.kind,
                validation: Some(validation),
                errors: Vec::new(),
                warnings,
                generated_at_ms: now_ms(),
            })
        }
        Err(errors) => Json(EscrowAuditResponse {
            ok: false,
            request_id,
            schema_version: SCHEMA_VERSION,
            cluster,
            escrow_id: request.escrow_id,
            kind: request.kind,
            validation: None,
            errors,
            warnings: Vec::new(),
            generated_at_ms: now_ms(),
        }),
    }
}

pub(crate) async fn resolve_http(
    State(state): State<AppState>,
    Json(request): Json<ResolutionRequest>,
) -> impl IntoResponse {
    state
        .metrics
        .resolution_validations_total
        .fetch_add(1, Ordering::Relaxed);
    let req_id = request_id(request.request_id.as_ref(), "escrow-resolution");
    let cluster = normalize_request_cluster(request.cluster.as_deref(), &state.default_cluster)
        .unwrap_or_else(|_| state.default_cluster.clone());
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    if request.schema_version != SCHEMA_VERSION {
        errors.push(format!(
            "schemaVersion must be {SCHEMA_VERSION}, got {}",
            request.schema_version
        ));
    }
    let spec = kind_spec(request.intent.kind);
    match validate_escrow_intent(
        &request.intent,
        &state.default_cluster,
        &state.allowed_program_ids,
    ) {
        Ok(intent_response) => warnings.extend(intent_response.warnings),
        Err(intent_errors) => errors.extend(
            intent_errors
                .into_iter()
                .map(|error| format!("intent.{error}")),
        ),
    }
    validate_resolution(
        request.action,
        &request.resolution,
        &request.intent.parties,
        &spec,
        request.intent.terms.release_mode,
        &mut errors,
        &mut warnings,
    );

    let ok = errors.is_empty();
    if !ok {
        state
            .metrics
            .resolution_errors_total
            .fetch_add(1, Ordering::Relaxed);
        state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
    }
    let status = if ok {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    (
        status,
        Json(ResolutionResponse {
            ok,
            request_id: req_id,
            schema_version: SCHEMA_VERSION,
            cluster,
            escrow_id: request.intent.escrow_id.clone(),
            kind: request.intent.kind,
            action: request.action,
            outcome: request.resolution.outcome.as_str(),
            errors,
            warnings,
            generated_at_ms: now_ms(),
        }),
    )
}

pub(crate) async fn simulate_settlement_http(
    State(state): State<AppState>,
    Json(request): Json<EscrowSettlementRequest>,
) -> Response {
    state
        .metrics
        .simulations_total
        .fetch_add(1, Ordering::Relaxed);
    let validation = match validate_settlement_request(
        &request,
        &state.default_cluster,
        state.allow_skip_preflight,
        &state.allowed_program_ids,
        false,
    ) {
        Ok(validation) => validation,
        Err(errors) => {
            state
                .metrics
                .settlement_errors_total
                .fetch_add(1, Ordering::Relaxed);
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            return json_error(
                StatusCode::BAD_REQUEST,
                "settlement simulation validation failed",
                json!({ "errors": errors }),
            );
        }
    };
    let encoding = normalize_encoding(request.encoding.as_deref()).unwrap_or("base64");
    let commitment = normalize_commitment(request.commitment.as_deref())
        .unwrap_or_else(|_| DEFAULT_COMMITMENT.to_string());
    let backend_result = match state.settlement_backend {
        SettlementBackend::SolanaRpc => {
            rpc_json(
                &state,
                "simulateTransaction",
                simulate_params(&request, encoding, &commitment),
                &validation.request_id,
            )
            .await
        }
        SettlementBackend::ContractService => {
            let body = contract_service_body(
                &request,
                ContractServiceOp::Simulate,
                &validation.cluster,
                encoding,
                &commitment,
                &validation.request_id,
            );
            contract_service_call(&state, ContractServiceOp::Simulate, body).await
        }
    };
    match backend_result {
        Ok(result) => Json(json!({
            "ok": true,
            "requestId": validation.request_id,
            "schemaVersion": SCHEMA_VERSION,
            "cluster": validation.cluster,
            "escrowId": request.escrow_id,
            "kind": request.kind,
            "action": request.action,
            "transactionBytes": validation.transaction_bytes.len(),
            "transactionDigest": validation.transaction_digest,
            "backend": state.settlement_backend.as_str(),
            "rpcMethod": "simulateTransaction",
            "result": result,
            "warnings": validation.warnings,
            "generatedAtMs": now_ms(),
        }))
        .into_response(),
        Err(error) => {
            state
                .metrics
                .settlement_errors_total
                .fetch_add(1, Ordering::Relaxed);
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            json_error(
                StatusCode::BAD_GATEWAY,
                "Solana settlement simulation failed",
                json!({ "error": error }),
            )
        }
    }
}

pub(crate) async fn settle_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<EscrowSettlementRequest>,
) -> Response {
    state
        .metrics
        .settlements_total
        .fetch_add(1, Ordering::Relaxed);
    if !state.settlement_enabled {
        state
            .metrics
            .policy_rejections_total
            .fetch_add(1, Ordering::Relaxed);
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "on-chain settlement sending is disabled; set SOLANA_SETTLEMENT_ENABLED=true to enable it",
            json!({}),
        );
    }
    if let Err((status, message)) = authorize_settlement(&headers, &state) {
        state
            .metrics
            .auth_failures_total
            .fetch_add(1, Ordering::Relaxed);
        return json_error(status, message, json!({}));
    }
    let validation = match validate_settlement_request(
        &request,
        &state.default_cluster,
        state.allow_skip_preflight,
        &state.allowed_program_ids,
        state.settlement_require_intent,
    ) {
        Ok(validation) => validation,
        Err(errors) => {
            if errors
                .iter()
                .any(|error| error.contains("intent is required"))
            {
                state
                    .metrics
                    .policy_rejections_total
                    .fetch_add(1, Ordering::Relaxed);
            }
            state
                .metrics
                .settlement_errors_total
                .fetch_add(1, Ordering::Relaxed);
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            return json_error(
                StatusCode::BAD_REQUEST,
                "settlement request validation failed",
                json!({ "errors": errors }),
            );
        }
    };
    let encoding = normalize_encoding(request.encoding.as_deref()).unwrap_or("base64");
    let commitment = normalize_commitment(request.commitment.as_deref())
        .unwrap_or_else(|_| DEFAULT_COMMITMENT.to_string());
    let backend_result = match state.settlement_backend {
        SettlementBackend::SolanaRpc => {
            rpc_json(
                &state,
                "sendTransaction",
                send_params(&request, encoding, &commitment),
                &validation.request_id,
            )
            .await
        }
        SettlementBackend::ContractService => {
            let body = contract_service_body(
                &request,
                ContractServiceOp::Send,
                &validation.cluster,
                encoding,
                &commitment,
                &validation.request_id,
            );
            contract_service_call(&state, ContractServiceOp::Send, body).await
        }
    };
    match backend_result {
        Ok(result) => {
            publish_escrow_event(
                &state,
                "solana.escrow.settlement",
                &validation.request_id,
                true,
            )
            .await;
            Json(json!({
                "ok": true,
                "requestId": validation.request_id,
                "schemaVersion": SCHEMA_VERSION,
                "cluster": validation.cluster,
                "escrowId": request.escrow_id,
                "kind": request.kind,
                "action": request.action,
                "transactionBytes": validation.transaction_bytes.len(),
                "transactionDigest": validation.transaction_digest,
                "backend": state.settlement_backend.as_str(),
                "rpcMethod": "sendTransaction",
                "result": result,
                "warnings": validation.warnings,
                "generatedAtMs": now_ms(),
            }))
            .into_response()
        }
        Err(error) => {
            state
                .metrics
                .settlement_errors_total
                .fetch_add(1, Ordering::Relaxed);
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            publish_runtime_critical_event(
                &state,
                "escrow-settlement-send-failed",
                "Escrow settlement sendTransaction failed.",
                json!({
                    "requestId": validation.request_id,
                    "escrowId": request.escrow_id,
                    "kind": request.kind.as_str(),
                    "action": request.action.as_str(),
                    "error": error,
                }),
            )
            .await;
            json_error(
                StatusCode::BAD_GATEWAY,
                "Solana settlement send failed",
                json!({}),
            )
        }
    }
}

pub(crate) async fn status_http(State(state): State<AppState>) -> impl IntoResponse {
    // The direct Solana RPC probe requires public-internet egress, which is only
    // provisioned for the solana-rpc backend. When delegating to the contract
    // service, readiness is gated on its reachability and the Solana probe is
    // skipped entirely so /status never depends on egress the pod does not have.
    let (health, version) = match state.settlement_backend {
        SettlementBackend::SolanaRpc => (
            Some(rpc_json(&state, "getHealth", json!([]), "escrow-status-health").await),
            Some(rpc_json(&state, "getVersion", json!([]), "escrow-status-version").await),
        ),
        SettlementBackend::ContractService => (None, None),
    };
    let contract_service = match state.settlement_backend {
        SettlementBackend::ContractService => Some(contract_service_health(&state).await),
        SettlementBackend::SolanaRpc => None,
    };
    let ok = match state.settlement_backend {
        SettlementBackend::SolanaRpc => {
            health.as_ref().map(Result::is_ok).unwrap_or(false)
                && version.as_ref().map(Result::is_ok).unwrap_or(false)
        }
        SettlementBackend::ContractService => {
            contract_service.as_ref().map(Result::is_ok).unwrap_or(false)
        }
    };
    let solana = match (health, version) {
        (Some(health), Some(version)) => json!({
            "health": health.unwrap_or_else(|error| json!({ "error": error })),
            "version": version.unwrap_or_else(|error| json!({ "error": error })),
        }),
        _ => json!({ "probe": "skipped", "reason": "delegated to dd-contract-service" }),
    };
    Json(json!({
        "ok": ok,
        "service": SERVICE_NAME,
        "schemaVersion": SCHEMA_VERSION,
        "cluster": state.default_cluster,
        "settlementEnabled": state.settlement_enabled,
        "settlementRequiresIntent": state.settlement_require_intent,
        "settlementBackend": state.settlement_backend.as_str(),
        "contractServiceConfigured": state.contract_service_url.is_some(),
        "allowedProgramCount": state.allowed_program_ids.len(),
        "skipPreflightAllowed": state.allow_skip_preflight,
        "natsEnabled": state.nats.is_some(),
        "validateSubject": state.validate_subject,
        "resultSubject": state.result_subject,
        "contractService": contract_service
            .map(|result| result.unwrap_or_else(|error| json!({ "error": error }))),
        "solana": solana,
        "generatedAtMs": now_ms(),
    }))
}

pub(crate) async fn api_docs_html() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("../generated/api-docs.html"))
}

pub(crate) async fn api_docs_json() -> impl axum::response::IntoResponse {
    (
        [("content-type", "application/json; charset=utf-8")],
        include_str!("../generated/api-docs.json"),
    )
}
