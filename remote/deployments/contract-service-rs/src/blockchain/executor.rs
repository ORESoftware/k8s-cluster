//! Smart-contract executor. Simulates contract calls on Solana (via the existing
//! `simulateTransaction` surface) and EVM (`eth_call` + `eth_estimateGas`), and —
//! only when gated on — broadcasts an **already client-signed** raw transaction.
//! The executor never signs (keyless); execution is relaying a signed payload.

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
use std::sync::atomic::Ordering;

use super::{
    authorize_execute, evm, json_err, json_ok, parse_chain, record_request, require_enabled,
    validate_chain_address, ChainKind,
};
use crate::AppState;

const MAX_EVM_CALLDATA_BYTES: usize = 128 * 1024;
const MAX_RAW_TX_BYTES: usize = 256 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SimulateRequest {
    chain: String,
    /// EVM: target contract address. Solana: ignored (encoded in the tx).
    #[serde(default)]
    to: Option<String>,
    /// EVM: pre-encoded calldata (`0x…`). ABI encoding is the caller's job.
    #[serde(default)]
    data: Option<String>,
    /// EVM: optional value in wei hex. Solana: unused.
    #[serde(default)]
    value: Option<String>,
    /// Solana: signed/base64 transaction to simulate.
    #[serde(default)]
    transaction: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecuteRequest {
    chain: String,
    request_id: String,
    /// An already client-signed raw transaction. Solana: base64. EVM: `0x…`.
    signed_transaction: String,
}

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/executor/simulate", post(simulate_http))
        .route("/executor/execute", post(execute_http))
}

async fn simulate_http(
    State(state): State<AppState>,
    Json(body): Json<SimulateRequest>,
) -> axum::response::Response {
    let bc = &state.blockchain;
    record_request(bc);
    if let Err(resp) = require_enabled(
        bc,
        bc.config().executor_enabled,
        "BLOCKCHAIN_EXECUTOR_ENABLED",
    ) {
        return resp;
    }
    let chain = match parse_chain(&body.chain) {
        Ok(kind) => kind,
        Err(error) => return json_err(StatusCode::BAD_REQUEST, &error),
    };
    bc.metrics()
        .executor_simulations_total
        .fetch_add(1, Ordering::Relaxed);

    match chain {
        ChainKind::Solana => {
            let Some(tx) = body
                .transaction
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
            else {
                return json_err(
                    StatusCode::BAD_REQUEST,
                    "solana simulate requires `transaction`",
                );
            };
            if tx.len() > MAX_RAW_TX_BYTES {
                return json_err(StatusCode::BAD_REQUEST, "transaction too large");
            }
            let params = json!([tx, { "encoding": "base64", "sigVerify": false }]);
            match crate::solana_rpc(&state, "simulateTransaction", params).await {
                Ok(result) => {
                    json_ok(json!({ "ok": true, "chain": "solana", "simulation": result }))
                }
                Err(error) => json_err(StatusCode::BAD_GATEWAY, &error),
            }
        }
        ChainKind::Evm => {
            if !bc.config().evm_configured() {
                return json_err(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "EVM RPC is not configured (set EVM_RPC_URL)",
                );
            }
            let Some(to) = body.to.as_deref() else {
                return json_err(StatusCode::BAD_REQUEST, "evm simulate requires `to`");
            };
            let to = match validate_chain_address(ChainKind::Evm, to) {
                Ok(value) => value,
                Err(error) => return json_err(StatusCode::BAD_REQUEST, &error),
            };
            let data = match body.data.as_deref() {
                Some(raw) => match evm::validate_hex_blob(raw, "data", MAX_EVM_CALLDATA_BYTES) {
                    Ok(value) => value,
                    Err(error) => return json_err(StatusCode::BAD_REQUEST, &error),
                },
                None => "0x".to_string(),
            };
            let mut call = json!({ "to": to, "data": data });
            if let Some(value) = body.value.as_deref().filter(|v| !v.is_empty()) {
                call["value"] = json!(value);
            }
            let call_result = bc.evm_rpc("eth_call", json!([call, "latest"])).await;
            let gas_result = bc.evm_rpc("eth_estimateGas", json!([call])).await;
            match (call_result, gas_result) {
                (Ok(ret), gas) => json_ok(json!({
                    "ok": true,
                    "chain": "evm",
                    "to": to,
                    "result": ret,
                    "gasEstimate": gas.ok(),
                })),
                (Err(error), _) => json_err(StatusCode::BAD_GATEWAY, &error),
            }
        }
    }
}

async fn execute_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ExecuteRequest>,
) -> axum::response::Response {
    let bc = &state.blockchain;
    record_request(bc);
    // Gate 1: feature flag for the broadcast path.
    if let Err(resp) = require_enabled(
        bc,
        bc.config().executor_execute_enabled,
        "BLOCKCHAIN_EXECUTOR_EXECUTE_ENABLED",
    ) {
        bc.metrics()
            .broadcast_blocked_total
            .fetch_add(1, Ordering::Relaxed);
        return resp;
    }
    // Gate 2: shared execute/broadcast auth secret.
    if let Err(resp) = authorize_execute(bc, &headers) {
        return resp;
    }
    let chain = match parse_chain(&body.chain) {
        Ok(kind) => kind,
        Err(error) => return json_err(StatusCode::BAD_REQUEST, &error),
    };
    let request_id = body.request_id.trim();
    if request_id.is_empty() || request_id.len() > 128 {
        return json_err(
            StatusCode::BAD_REQUEST,
            "requestId must be 1..=128 characters",
        );
    }
    let signed = body.signed_transaction.trim();
    if signed.is_empty() || signed.len() > MAX_RAW_TX_BYTES {
        return json_err(
            StatusCode::BAD_REQUEST,
            "signedTransaction missing or too large",
        );
    }

    // Idempotency: claim once within the TTL window so retries don't double-send.
    let idem_key = format!("blockchain-executor:{request_id}");
    if !state.claim_idempotency_key(&idem_key) {
        return json_err(
            StatusCode::CONFLICT,
            "duplicate requestId within idempotency window",
        );
    }

    let outcome = match chain {
        ChainKind::Solana => crate::solana_rpc(
            &state,
            "sendTransaction",
            json!([signed, { "encoding": "base64", "skipPreflight": false }]),
        )
        .await
        .map(|sig| json!({ "chain": "solana", "signature": sig })),
        ChainKind::Evm => {
            if !bc.config().evm_configured() {
                state.release_idempotency_key(&idem_key);
                return json_err(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "EVM RPC is not configured (set EVM_RPC_URL)",
                );
            }
            let raw = match evm::validate_hex_blob(signed, "signedTransaction", MAX_RAW_TX_BYTES) {
                Ok(value) => value,
                Err(error) => {
                    state.release_idempotency_key(&idem_key);
                    return json_err(StatusCode::BAD_REQUEST, &error);
                }
            };
            bc.evm_rpc("eth_sendRawTransaction", json!([raw]))
                .await
                .map(|hash| json!({ "chain": "evm", "txHash": hash }))
        }
    };

    match outcome {
        Ok(value) => json_ok(json!({ "ok": true, "requestId": request_id, "broadcast": value })),
        Err(error) => {
            // Allow a legitimately failed broadcast to be retried with the same id.
            state.release_idempotency_key(&idem_key);
            json_err(StatusCode::BAD_GATEWAY, &error)
        }
    }
}
