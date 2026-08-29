use std::sync::atomic::Ordering;

use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use base64::{engine::general_purpose, Engine as _};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::confirm::{bounded_confirm, normalize_confirm_commitment};
use crate::rpc::solana_rpc;
use crate::settlement::{RESOLUTION_DECISIONS, SETTLEMENT_ACTIONS};
use crate::shared::{json_response, log_warn, now_ms, request_id};
use crate::state::{
    AppState, DEFAULT_CONFIRM_POLL_INTERVAL_MS, DEFAULT_CONFIRM_TIMEOUT_MS,
    MAX_ACCOUNTS_PER_INSTRUCTION, MAX_COMPUTE_UNITS_PER_INSTRUCTION, MAX_CONFIRM_SIGNATURES,
    MAX_CONFIRM_TIMEOUT_MS, MAX_INSTRUCTIONS, MAX_LABEL_LEN, MAX_MEMO_BYTES, MAX_RATIONALE_BYTES,
    MAX_RENT_EXEMPTION_BYTES, MAX_REQUEST_ID_LEN, MAX_SEND_RETRIES, MAX_SIGNED_TRANSACTION_BYTES,
    MIN_CONFIRM_POLL_INTERVAL_MS, RESOLUTION_SCHEMA_VERSION, SCHEMA_VERSION, SERVICE_NAME,
    SETTLEMENT_SCHEMA_VERSION,
};
use crate::validation::{
    authorize_send, normalize_commitment_or_default, normalize_request_cluster, send_params,
    simulate_params, validate_contract_request, validate_pubkey, validate_signature,
    validate_signed_transaction, ContractRequest, TransactionRpcRequest,
};

// ---------------------------------------------------------------------------
// Read-only Solana RPC surface
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BlockhashQuery {
    cluster: Option<String>,
    commitment: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccountInfoRequest {
    request_id: Option<String>,
    cluster: Option<String>,
    pubkey: String,
    encoding: Option<String>,
    commitment: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BalanceRequest {
    request_id: Option<String>,
    cluster: Option<String>,
    pubkey: String,
    /// "sol" (default) reads the lamport balance; "token" reads an SPL token
    /// account balance via getTokenAccountBalance.
    kind: Option<String>,
    commitment: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FeeForMessageRequest {
    request_id: Option<String>,
    cluster: Option<String>,
    /// Base64-encoded compiled message (not a full signed transaction).
    message: String,
    commitment: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RentExemptionQuery {
    cluster: Option<String>,
    bytes: u64,
    commitment: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TransactionLookupRequest {
    request_id: Option<String>,
    cluster: Option<String>,
    signature: String,
    commitment: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConfirmRequest {
    request_id: Option<String>,
    cluster: Option<String>,
    signatures: Vec<String>,
    target_commitment: Option<String>,
    timeout_ms: Option<u64>,
    poll_interval_ms: Option<u64>,
}

pub(crate) async fn home(State(state): State<AppState>) -> impl IntoResponse {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(json!({
        "service": "dd-contract-service",
        "runtime": "rust",
        "chain": "solana",
        "schemaVersion": SCHEMA_VERSION,
        "settlementSchemaVersion": SETTLEMENT_SCHEMA_VERSION,
        "resolutionSchemaVersion": RESOLUTION_SCHEMA_VERSION,
        "cluster": state.default_cluster,
        "sendEnabled": state.send_enabled,
        "skipPreflightAllowed": state.allow_skip_preflight,
        "settlementEnabled": state.settlement_enabled,
        "resolutionEnabled": state.resolution_enabled,
        "natsSettlementEnabled": state.nats_settlement_enabled,
        "mainnetSettlementEnabled": state.mainnet_settlement_enabled,
        "routes": {
            "health": "/healthz",
            "readiness": "/readyz",
            "capabilities": "/capabilities",
            "metrics": "/metrics",
            "status": "/status",
            "schema": "/schema",
            "settlementSchema": "/schema/settlement",
            "resolutionSchema": "/schema/resolution",
            "example": "/example",
            "settlementExample": "/example/settlement",
            "validate": "POST /validate",
            "simulate": "POST /simulate",
            "send": "POST /send",
            "blockhash": "GET /blockhash",
            "account": "POST /account",
            "balance": "POST /balance",
            "fee": "POST /fee",
            "rentExemption": "GET /rent-exemption",
            "transaction": "POST /transaction",
            "confirm": "POST /confirm",
            "simulateSettlement": "POST /simulate-settlement",
            "settle": "POST /settle",
            "resolve": "POST /resolve",
            "inspectProgram": "POST /program/inspect",
            "verifyProgram": "POST /program/verify",
            "inspectEscrow": "POST /escrow/inspect",
            "signatureHistory": "POST /chain/signatures",
            "priorityFees": "POST /chain/priority-fees"
        },
        "nats": {
            "resultSubject": state.result_subject,
            "settlementResultSubject": state.settlement_result_subject,
            "eventSubject": state.event_subject
        }
    }))
}

pub(crate) async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(json!({
        "ok": true,
        "service": "dd-contract-service",
        "chain": "solana",
        "cluster": state.default_cluster,
        "rpcConfigured": !state.solana_rpc_url.trim().is_empty(),
        "sendEnabled": state.send_enabled,
        "skipPreflightAllowed": state.allow_skip_preflight
    }))
}

pub(crate) async fn readyz(State(state): State<AppState>) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let (chain, coordination, formal_methods) = tokio::join!(
        solana_rpc(&state, "getHealth", json!([])),
        state.coordination.readiness(),
        state.solana_features.readiness(),
    );
    let ok = chain.is_ok() && coordination.is_ok() && formal_methods.is_ok();
    if !ok {
        log_warn(
            "contract-service-not-ready",
            "Contract service dependency readiness check failed.",
            json!({
                "solana": chain.as_ref().err(),
                "coordination": coordination.as_ref().err(),
                "formalMethods": formal_methods.as_ref().err(),
            }),
        );
    }
    json_response(
        if ok {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        json!({
            "ok": ok,
            "service": SERVICE_NAME,
            "dependencies": {
                "solanaRpc": chain.is_ok(),
                "postgresAndFiducia": coordination.is_ok(),
                "formalMethods": formal_methods.is_ok(),
            }
        }),
    )
}

pub(crate) async fn status_http(State(state): State<AppState>) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);

    let health = solana_rpc(&state, "getHealth", json!([])).await;
    let version = solana_rpc(&state, "getVersion", json!([])).await;
    let ok = health.is_ok() && version.is_ok();
    let status = if ok {
        StatusCode::OK
    } else {
        StatusCode::BAD_GATEWAY
    };
    if !ok {
        state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
    }

    json_response(
        status,
        json!({
            "ok": ok,
            "service": "dd-contract-service",
            "cluster": state.default_cluster,
            "sendEnabled": state.send_enabled,
            "settlementEnabled": state.settlement_enabled,
            "resolutionEnabled": state.resolution_enabled,
            "natsSettlementEnabled": state.nats_settlement_enabled,
            "mainnetSettlementEnabled": state.mainnet_settlement_enabled,
            "skipPreflightAllowed": state.allow_skip_preflight,
            "rpcHealth": health.map_err(|error| error.to_string()),
            "rpcVersion": version.map_err(|error| error.to_string()),
            "generatedAtMs": now_ms()
        }),
    )
}

pub(crate) async fn schema_http(State(state): State<AppState>) -> impl IntoResponse {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(contract_schema())
}

pub(crate) async fn example_http(State(state): State<AppState>) -> impl IntoResponse {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(contract_example())
}

pub(crate) async fn settlement_schema_http(State(state): State<AppState>) -> impl IntoResponse {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(settlement_schema())
}

pub(crate) async fn resolution_schema_http(State(state): State<AppState>) -> impl IntoResponse {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(resolution_schema())
}

pub(crate) async fn settlement_example_http(State(state): State<AppState>) -> impl IntoResponse {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(settlement_example())
}

pub(crate) async fn validate_http(
    State(state): State<AppState>,
    Json(request): Json<ContractRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .validations_total
        .fetch_add(1, Ordering::Relaxed);

    match validate_contract_request(&request, &state.default_cluster) {
        Ok(response) => json_response(StatusCode::OK, json!(response)),
        Err(errors) => {
            state
                .metrics
                .validation_errors_total
                .fetch_add(1, Ordering::Relaxed);
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            log_warn(
                "contract-validation-rejected",
                "Contract validation request was rejected.",
                json!({
                    "requestId": request_id(request.request_id.as_ref(), "contract-validation"),
                    "errorCount": errors.len(),
                }),
            );
            json_response(
                StatusCode::BAD_REQUEST,
                json!({
                    "ok": false,
                    "requestId": request_id(request.request_id.as_ref(), "contract-validation"),
                    "errors": errors,
                    "generatedAtMs": now_ms()
                }),
            )
        }
    }
}

pub(crate) async fn simulate_http(
    State(state): State<AppState>,
    Json(request): Json<TransactionRpcRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);

    let cluster =
        match normalize_request_cluster(request.cluster.as_deref(), &state.default_cluster) {
            Ok(cluster) => cluster,
            Err(error) => {
                state
                    .metrics
                    .policy_rejections_total
                    .fetch_add(1, Ordering::Relaxed);
                log_warn(
                    "contract-simulate-policy-rejected",
                    "Signed transaction simulation was rejected by policy.",
                    json!({
                        "requestId": request_id(request.request_id.as_ref(), "contract-simulate"),
                        "reason": "cluster_mismatch",
                    }),
                );
                return json_response(
                    StatusCode::BAD_REQUEST,
                    json!({ "ok": false, "error": error }),
                );
            }
        };
    let (encoding, decoded_bytes) = match validate_signed_transaction(&request) {
        Ok(validated) => validated,
        Err(error) => {
            state
                .metrics
                .policy_rejections_total
                .fetch_add(1, Ordering::Relaxed);
            log_warn(
                "contract-simulate-policy-rejected",
                "Signed transaction simulation was rejected by policy.",
                json!({
                    "requestId": request_id(request.request_id.as_ref(), "contract-simulate"),
                    "reason": "transaction_invalid",
                    "error": error.clone(),
                }),
            );
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "ok": false, "error": error }),
            );
        }
    };
    let params = match simulate_params(&request, encoding) {
        Ok(params) => params,
        Err(error) => {
            state
                .metrics
                .policy_rejections_total
                .fetch_add(1, Ordering::Relaxed);
            log_warn(
                "contract-simulate-policy-rejected",
                "Signed transaction simulation was rejected by policy.",
                json!({
                    "requestId": request_id(request.request_id.as_ref(), "contract-simulate"),
                    "reason": "simulate_params_invalid",
                    "error": error.clone(),
                }),
            );
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "ok": false, "error": error }),
            );
        }
    };

    match solana_rpc(&state, "simulateTransaction", params).await {
        Ok(result) => json_response(
            StatusCode::OK,
            json!({
                "ok": true,
                "requestId": request_id(request.request_id.as_ref(), "contract-simulate"),
                "cluster": cluster,
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
                json!({
                    "ok": false,
                    "requestId": request_id(request.request_id.as_ref(), "contract-simulate"),
                    "error": error,
                    "generatedAtMs": now_ms()
                }),
            )
        }
    }
}

pub(crate) async fn send_http(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(request): Json<TransactionRpcRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);

    if !state.send_enabled {
        state
            .metrics
            .send_blocked_total
            .fetch_add(1, Ordering::Relaxed);
        state
            .metrics
            .policy_rejections_total
            .fetch_add(1, Ordering::Relaxed);
        log_warn(
            "contract-send-disabled",
            "Raw transaction send was blocked because sending is disabled.",
            json!({
                "requestId": request_id(request.request_id.as_ref(), "contract-send"),
            }),
        );
        return json_response(
            StatusCode::FORBIDDEN,
            json!({
                "ok": false,
                "requestId": request_id(request.request_id.as_ref(), "contract-send"),
                "error": "transaction sending is disabled; set SOLANA_SEND_ENABLED=true to permit sendTransaction",
                "generatedAtMs": now_ms()
            }),
        );
    }

    if let Err((status, error)) = authorize_send(&headers, &state) {
        state
            .metrics
            .send_blocked_total
            .fetch_add(1, Ordering::Relaxed);
        state
            .metrics
            .send_auth_failures_total
            .fetch_add(1, Ordering::Relaxed);
        state
            .metrics
            .policy_rejections_total
            .fetch_add(1, Ordering::Relaxed);
        log_warn(
            "contract-send-auth-failed",
            "Raw transaction send authorization failed.",
            json!({
                "requestId": request_id(request.request_id.as_ref(), "contract-send"),
                "status": status.as_u16(),
            }),
        );
        return json_response(
            status,
            json!({
                "ok": false,
                "requestId": request_id(request.request_id.as_ref(), "contract-send"),
                "error": error,
                "generatedAtMs": now_ms()
            }),
        );
    }

    let cluster =
        match normalize_request_cluster(request.cluster.as_deref(), &state.default_cluster) {
            Ok(cluster) => cluster,
            Err(error) => {
                state
                    .metrics
                    .policy_rejections_total
                    .fetch_add(1, Ordering::Relaxed);
                log_warn(
                    "contract-send-policy-rejected",
                    "Raw transaction send was rejected by policy.",
                    json!({
                        "requestId": request_id(request.request_id.as_ref(), "contract-send"),
                        "reason": "cluster_mismatch",
                    }),
                );
                return json_response(
                    StatusCode::BAD_REQUEST,
                    json!({ "ok": false, "error": error }),
                );
            }
        };
    let (encoding, decoded_bytes) = match validate_signed_transaction(&request) {
        Ok(validated) => validated,
        Err(error) => {
            state
                .metrics
                .policy_rejections_total
                .fetch_add(1, Ordering::Relaxed);
            log_warn(
                "contract-send-policy-rejected",
                "Raw transaction send was rejected by policy.",
                json!({
                    "requestId": request_id(request.request_id.as_ref(), "contract-send"),
                    "reason": "transaction_invalid",
                    "error": error.clone(),
                }),
            );
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "ok": false, "error": error }),
            );
        }
    };
    let params = match send_params(&request, encoding, state.allow_skip_preflight) {
        Ok(params) => params,
        Err(error) => {
            state
                .metrics
                .policy_rejections_total
                .fetch_add(1, Ordering::Relaxed);
            log_warn(
                "contract-send-policy-rejected",
                "Raw transaction send was rejected by policy.",
                json!({
                    "requestId": request_id(request.request_id.as_ref(), "contract-send"),
                    "reason": "send_params_invalid",
                    "error": error.clone(),
                }),
            );
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "ok": false, "error": error }),
            );
        }
    };

    match solana_rpc(&state, "sendTransaction", params).await {
        Ok(signature) => json_response(
            StatusCode::OK,
            json!({
                "ok": true,
                "requestId": request_id(request.request_id.as_ref(), "contract-send"),
                "cluster": cluster,
                "encoding": encoding,
                "transactionBytes": decoded_bytes,
                "signature": signature,
                "generatedAtMs": now_ms()
            }),
        ),
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            json_response(
                StatusCode::BAD_GATEWAY,
                json!({
                    "ok": false,
                    "requestId": request_id(request.request_id.as_ref(), "contract-send"),
                    "error": error,
                    "generatedAtMs": now_ms()
                }),
            )
        }
    }
}

#[allow(clippy::result_large_err)]
fn enforce_cluster(
    state: &AppState,
    cluster: Option<&str>,
    metrics_prefix: &str,
) -> Result<String, Response> {
    normalize_request_cluster(cluster, &state.default_cluster).map_err(|error| {
        state
            .metrics
            .policy_rejections_total
            .fetch_add(1, Ordering::Relaxed);
        log_warn(
            "contract-read-policy-rejected",
            "Read RPC request was rejected by policy.",
            json!({ "reason": "cluster_mismatch", "scope": metrics_prefix }),
        );
        json_response(
            StatusCode::BAD_REQUEST,
            json!({ "ok": false, "error": error }),
        )
    })
}

async fn read_rpc_response(
    state: &AppState,
    method: &str,
    params: Value,
    request_id: String,
    cluster: String,
) -> Response {
    match solana_rpc(state, method, params).await {
        Ok(result) => json_response(
            StatusCode::OK,
            json!({
                "ok": true,
                "requestId": request_id,
                "cluster": cluster,
                "rpcMethod": method,
                "result": result,
                "generatedAtMs": now_ms()
            }),
        ),
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            json_response(
                StatusCode::BAD_GATEWAY,
                json!({
                    "ok": false,
                    "requestId": request_id,
                    "rpcMethod": method,
                    "error": error,
                    "generatedAtMs": now_ms()
                }),
            )
        }
    }
}

pub(crate) async fn blockhash_http(
    State(state): State<AppState>,
    Query(query): Query<BlockhashQuery>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let cluster = match enforce_cluster(&state, query.cluster.as_deref(), "blockhash") {
        Ok(cluster) => cluster,
        Err(response) => return response,
    };
    let commitment = match normalize_commitment_or_default(query.commitment.as_deref()) {
        Ok(commitment) => commitment,
        Err(error) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "ok": false, "error": error }),
            )
        }
    };
    let params = json!([{ "commitment": commitment }]);
    read_rpc_response(
        &state,
        "getLatestBlockhash",
        params,
        "contract-blockhash".to_string(),
        cluster,
    )
    .await
}

pub(crate) async fn account_http(
    State(state): State<AppState>,
    Json(request): Json<AccountInfoRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let cluster = match enforce_cluster(&state, request.cluster.as_deref(), "account") {
        Ok(cluster) => cluster,
        Err(response) => return response,
    };
    if let Err(error) = validate_pubkey(&request.pubkey, "pubkey") {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "ok": false, "error": error }),
        );
    }
    let encoding = match request.encoding.as_deref().map(str::trim) {
        Some("base64") | None => "base64",
        Some("base58") => "base58",
        Some("jsonParsed") => "jsonParsed",
        Some(other) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "ok": false, "error": format!("encoding must be base64, base58, or jsonParsed: {other}") }),
            )
        }
    };
    let commitment = match normalize_commitment_or_default(request.commitment.as_deref()) {
        Ok(commitment) => commitment,
        Err(error) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "ok": false, "error": error }),
            )
        }
    };
    let params = json!([
        request.pubkey.trim(),
        { "encoding": encoding, "commitment": commitment }
    ]);
    read_rpc_response(
        &state,
        "getAccountInfo",
        params,
        request_id(request.request_id.as_ref(), "contract-account"),
        cluster,
    )
    .await
}

pub(crate) async fn balance_http(
    State(state): State<AppState>,
    Json(request): Json<BalanceRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let cluster = match enforce_cluster(&state, request.cluster.as_deref(), "balance") {
        Ok(cluster) => cluster,
        Err(response) => return response,
    };
    if let Err(error) = validate_pubkey(&request.pubkey, "pubkey") {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "ok": false, "error": error }),
        );
    }
    let commitment = match normalize_commitment_or_default(request.commitment.as_deref()) {
        Ok(commitment) => commitment,
        Err(error) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "ok": false, "error": error }),
            )
        }
    };
    let (method, params) = match request.kind.as_deref().map(str::trim).unwrap_or("sol") {
        "sol" => (
            "getBalance",
            json!([request.pubkey.trim(), { "commitment": commitment }]),
        ),
        "token" => (
            "getTokenAccountBalance",
            json!([request.pubkey.trim(), { "commitment": commitment }]),
        ),
        other => {
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "ok": false, "error": format!("kind must be sol or token: {other}") }),
            )
        }
    };
    read_rpc_response(
        &state,
        method,
        params,
        request_id(request.request_id.as_ref(), "contract-balance"),
        cluster,
    )
    .await
}

pub(crate) async fn fee_http(
    State(state): State<AppState>,
    Json(request): Json<FeeForMessageRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let cluster = match enforce_cluster(&state, request.cluster.as_deref(), "fee") {
        Ok(cluster) => cluster,
        Err(response) => return response,
    };
    let message = request.message.trim();
    if message.is_empty() {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "ok": false, "error": "message must not be empty" }),
        );
    }
    match general_purpose::STANDARD.decode(message) {
        Ok(bytes) if bytes.len() <= MAX_SIGNED_TRANSACTION_BYTES => {}
        Ok(_) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "ok": false, "error": "message exceeds maximum size" }),
            )
        }
        Err(error) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "ok": false, "error": format!("message must be valid base64: {error}") }),
            )
        }
    }
    let commitment = match normalize_commitment_or_default(request.commitment.as_deref()) {
        Ok(commitment) => commitment,
        Err(error) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "ok": false, "error": error }),
            )
        }
    };
    let params = json!([message, { "commitment": commitment }]);
    read_rpc_response(
        &state,
        "getFeeForMessage",
        params,
        request_id(request.request_id.as_ref(), "contract-fee"),
        cluster,
    )
    .await
}

pub(crate) async fn rent_exemption_http(
    State(state): State<AppState>,
    Query(query): Query<RentExemptionQuery>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let cluster = match enforce_cluster(&state, query.cluster.as_deref(), "rent-exemption") {
        Ok(cluster) => cluster,
        Err(response) => return response,
    };
    if query.bytes > MAX_RENT_EXEMPTION_BYTES {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "ok": false, "error": format!("bytes must be at most {MAX_RENT_EXEMPTION_BYTES}") }),
        );
    }
    let commitment = match normalize_commitment_or_default(query.commitment.as_deref()) {
        Ok(commitment) => commitment,
        Err(error) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "ok": false, "error": error }),
            )
        }
    };
    let params = json!([query.bytes, { "commitment": commitment }]);
    read_rpc_response(
        &state,
        "getMinimumBalanceForRentExemption",
        params,
        "contract-rent-exemption".to_string(),
        cluster,
    )
    .await
}

pub(crate) async fn transaction_http(
    State(state): State<AppState>,
    Json(request): Json<TransactionLookupRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let cluster = match enforce_cluster(&state, request.cluster.as_deref(), "transaction") {
        Ok(cluster) => cluster,
        Err(response) => return response,
    };
    let signature = match validate_signature(&request.signature, "signature") {
        Ok(signature) => signature,
        Err(error) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "ok": false, "error": error }),
            )
        }
    };
    let commitment = match normalize_confirm_commitment(request.commitment.as_deref()) {
        Ok(commitment) => commitment,
        Err(error) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "ok": false, "error": error }),
            )
        }
    };
    let params = json!([
        signature,
        { "commitment": commitment, "maxSupportedTransactionVersion": 0, "encoding": "json" }
    ]);
    read_rpc_response(
        &state,
        "getTransaction",
        params,
        request_id(request.request_id.as_ref(), "contract-transaction"),
        cluster,
    )
    .await
}

pub(crate) async fn confirm_http(
    State(state): State<AppState>,
    Json(request): Json<ConfirmRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let cluster = match enforce_cluster(&state, request.cluster.as_deref(), "confirm") {
        Ok(cluster) => cluster,
        Err(response) => return response,
    };
    if request.signatures.is_empty() {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "ok": false, "error": "signatures must contain at least one signature" }),
        );
    }
    if request.signatures.len() > MAX_CONFIRM_SIGNATURES {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "ok": false, "error": format!("signatures must contain at most {MAX_CONFIRM_SIGNATURES} signatures") }),
        );
    }
    let mut signatures = Vec::with_capacity(request.signatures.len());
    for (index, signature) in request.signatures.iter().enumerate() {
        match validate_signature(signature, &format!("signatures[{index}]")) {
            Ok(signature) => signatures.push(signature),
            Err(error) => {
                return json_response(
                    StatusCode::BAD_REQUEST,
                    json!({ "ok": false, "error": error }),
                )
            }
        }
    }
    let target = match normalize_confirm_commitment(request.target_commitment.as_deref()) {
        Ok(target) => target,
        Err(error) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "ok": false, "error": error }),
            )
        }
    };
    let timeout_ms = request.timeout_ms.unwrap_or(DEFAULT_CONFIRM_TIMEOUT_MS);
    let poll_interval_ms = request
        .poll_interval_ms
        .unwrap_or(DEFAULT_CONFIRM_POLL_INTERVAL_MS);

    // Confirm the batch concurrently so wall-clock is bounded by a single
    // timeout window, not the sum across signatures.
    let outcomes = futures_util::future::join_all(signatures.iter().map(|signature| {
        bounded_confirm(&state, signature, &target, timeout_ms, poll_interval_ms)
    }))
    .await;
    let all_reached = outcomes.iter().all(|outcome| outcome.reached);
    json_response(
        StatusCode::OK,
        json!({
            "ok": all_reached,
            "requestId": request_id(request.request_id.as_ref(), "contract-confirm"),
            "cluster": cluster,
            "targetCommitment": target,
            "outcomes": outcomes,
            "generatedAtMs": now_ms()
        }),
    )
}

fn contract_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "dd-contract-service Solana contract request",
        "type": "object",
        "required": ["schemaVersion", "programId", "instructions"],
        "properties": {
            "schemaVersion": { "const": SCHEMA_VERSION },
            "requestId": { "type": "string", "maxLength": MAX_REQUEST_ID_LEN },
            "cluster": { "enum": ["mainnet-beta", "devnet", "testnet", "localnet", "custom"] },
            "programId": { "type": "string", "description": "Base58 Solana program public key" },
            "payer": { "type": "string", "description": "Optional base58 fee payer public key" },
            "recentBlockhash": { "type": "string", "description": "Optional recent blockhash to include before signing" },
            "commitment": { "enum": ["processed", "confirmed", "finalized"] },
            "memo": { "type": "string", "maxLength": MAX_MEMO_BYTES },
            "instructions": {
                "type": "array",
                "minItems": 1,
                "maxItems": MAX_INSTRUCTIONS,
                "items": {
                    "type": "object",
                    "required": ["name", "accounts"],
                    "properties": {
                        "name": { "type": "string", "maxLength": MAX_LABEL_LEN },
                        "programId": { "type": "string" },
                        "accounts": {
                            "type": "array",
                            "maxItems": MAX_ACCOUNTS_PER_INSTRUCTION,
                            "items": {
                                "type": "object",
                                "required": ["pubkey"],
                                "properties": {
                                    "pubkey": { "type": "string" },
                                    "isSigner": { "type": "boolean" },
                                    "isWritable": { "type": "boolean" },
                                    "label": { "type": "string", "maxLength": MAX_LABEL_LEN }
                                }
                            }
                        },
                        "dataBase64": { "type": "string" },
                        "dataBase58": { "type": "string" },
                        "computeUnits": { "type": "integer", "minimum": 0, "maximum": MAX_COMPUTE_UNITS_PER_INSTRUCTION }
                    }
                }
            }
        }
    })
}

fn contract_example() -> Value {
    json!({
        "schemaVersion": SCHEMA_VERSION,
        "requestId": "contract-demo",
        "cluster": "devnet",
        "programId": "11111111111111111111111111111111",
        "payer": "11111111111111111111111111111111",
        "recentBlockhash": "11111111111111111111111111111111",
        "commitment": "confirmed",
        "memo": "example contract instruction envelope",
        "instructions": [
            {
                "name": "system-transfer-shape",
                "accounts": [
                    {
                        "label": "from",
                        "pubkey": "11111111111111111111111111111111",
                        "isSigner": true,
                        "isWritable": true
                    },
                    {
                        "label": "to",
                        "pubkey": "11111111111111111111111111111111",
                        "isSigner": false,
                        "isWritable": true
                    }
                ],
                "dataBase64": "AQID",
                "computeUnits": 200000
            }
        ]
    })
}

fn settlement_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "dd-contract-service Solana settlement request",
        "type": "object",
        "required": ["schemaVersion", "action", "transaction"],
        "properties": {
            "schemaVersion": { "const": SETTLEMENT_SCHEMA_VERSION },
            "requestId": { "type": "string", "maxLength": MAX_REQUEST_ID_LEN, "description": "Explicit ids guard at-most-once broadcast within the idempotency window." },
            "cluster": { "enum": ["mainnet-beta", "devnet", "testnet", "localnet", "custom"] },
            "contractId": { "type": "string" },
            "escrowId": { "type": "string" },
            "action": { "enum": SETTLEMENT_ACTIONS },
            "transaction": { "type": "string", "description": "Signed transaction, base64 (default) or base58" },
            "encoding": { "enum": ["base64", "base58"] },
            "commitment": { "enum": ["processed", "confirmed", "finalized"] },
            "skipPreflight": { "type": "boolean" },
            "maxRetries": { "type": "integer", "minimum": 0, "maximum": MAX_SEND_RETRIES },
            "minContextSlot": { "type": "integer", "minimum": 0 },
            "intentDigest": { "type": "string" },
            "memo": { "type": "string", "maxLength": MAX_MEMO_BYTES },
            "confirm": {
                "type": "object",
                "properties": {
                    "targetCommitment": { "enum": ["confirmed", "finalized"] },
                    "timeoutMs": { "type": "integer", "minimum": 0, "maximum": MAX_CONFIRM_TIMEOUT_MS },
                    "pollIntervalMs": { "type": "integer", "minimum": MIN_CONFIRM_POLL_INTERVAL_MS }
                }
            }
        }
    })
}

fn resolution_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "dd-contract-service Solana dispute resolution request",
        "type": "object",
        "required": ["schemaVersion", "decision", "action", "transaction"],
        "properties": {
            "schemaVersion": { "const": RESOLUTION_SCHEMA_VERSION },
            "requestId": { "type": "string", "maxLength": MAX_REQUEST_ID_LEN },
            "cluster": { "enum": ["mainnet-beta", "devnet", "testnet", "localnet", "custom"] },
            "disputeId": { "type": "string" },
            "escrowId": { "type": "string" },
            "decision": { "enum": RESOLUTION_DECISIONS, "description": "Dispute outcome; constrains which settlement action may enact it." },
            "action": { "enum": SETTLEMENT_ACTIONS },
            "arbiter": { "type": "string", "description": "Base58 arbiter public key" },
            "arbiterRequiredSigner": { "type": "boolean" },
            "transaction": { "type": "string" },
            "encoding": { "enum": ["base64", "base58"] },
            "commitment": { "enum": ["processed", "confirmed", "finalized"] },
            "skipPreflight": { "type": "boolean" },
            "maxRetries": { "type": "integer", "minimum": 0, "maximum": MAX_SEND_RETRIES },
            "minContextSlot": { "type": "integer", "minimum": 0 },
            "rationale": { "type": "string", "maxLength": MAX_RATIONALE_BYTES },
            "confirm": {
                "type": "object",
                "properties": {
                    "targetCommitment": { "enum": ["confirmed", "finalized"] },
                    "timeoutMs": { "type": "integer", "minimum": 0, "maximum": MAX_CONFIRM_TIMEOUT_MS },
                    "pollIntervalMs": { "type": "integer", "minimum": MIN_CONFIRM_POLL_INTERVAL_MS }
                }
            }
        }
    })
}

fn settlement_example() -> Value {
    json!({
        "schemaVersion": SETTLEMENT_SCHEMA_VERSION,
        "requestId": "settlement-demo",
        "cluster": "devnet",
        "escrowId": "escrow-demo",
        "action": "release",
        "transaction": "<base64-encoded signed settlement transaction>",
        "encoding": "base64",
        "commitment": "confirmed",
        "intentDigest": "solana:0011223344556677",
        "confirm": { "targetCommitment": "finalized", "timeoutMs": 30000 }
    })
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
