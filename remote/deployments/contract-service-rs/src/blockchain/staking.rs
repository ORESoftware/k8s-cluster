//! Staking management. Validates staking intents (Solana stake
//! delegate/deactivate/withdraw; EVM staking-contract calls), reads on-chain
//! stake positions, and — only when gated on — broadcasts an externally-signed
//! staking transaction. Keyless: the service never signs.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
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

const MAX_RAW_TX_BYTES: usize = 256 * 1024;
const SOLANA_ACTIONS: [&str; 4] = ["delegate", "deactivate", "withdraw", "split"];
const EVM_ACTIONS: [&str; 3] = ["stake", "unstake", "claim-rewards"];

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ValidateRequest {
    chain: String,
    action: String,
    /// Stake account (Solana) or staking contract (EVM).
    stake_target: String,
    /// Validator vote account (Solana) or validator/operator address (EVM).
    #[serde(default)]
    validator: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IntentRequest {
    chain: String,
    request_id: String,
    signed_transaction: String,
}

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/staking/validate", post(validate_http))
        .route("/staking/positions/:chain/:address", get(positions_http))
        .route("/staking/intent", post(intent_http))
}

fn validate_action(chain: ChainKind, action: &str) -> Result<String, String> {
    let lowered = action.trim().to_ascii_lowercase();
    let allowed: &[&str] = match chain {
        ChainKind::Solana => &SOLANA_ACTIONS,
        ChainKind::Evm => &EVM_ACTIONS,
    };
    if allowed.contains(&lowered.as_str()) {
        Ok(lowered)
    } else {
        Err(format!("action must be one of {allowed:?}"))
    }
}

async fn validate_http(
    State(state): State<AppState>,
    Json(body): Json<ValidateRequest>,
) -> axum::response::Response {
    let bc = &state.blockchain;
    record_request(bc);
    if let Err(resp) = require_enabled(
        bc,
        bc.config().staking_enabled,
        "BLOCKCHAIN_STAKING_ENABLED",
    ) {
        return resp;
    }
    let chain = match parse_chain(&body.chain) {
        Ok(kind) => kind,
        Err(error) => return json_err(StatusCode::BAD_REQUEST, &error),
    };
    let action = match validate_action(chain, &body.action) {
        Ok(value) => value,
        Err(error) => return json_err(StatusCode::BAD_REQUEST, &error),
    };
    let stake_target = match validate_chain_address(chain, &body.stake_target) {
        Ok(value) => value,
        Err(error) => return json_err(StatusCode::BAD_REQUEST, &error),
    };
    // `delegate`/`stake` need a validator target; others don't.
    let needs_validator = matches!(action.as_str(), "delegate" | "stake");
    let validator = match (&body.validator, needs_validator) {
        (Some(raw), _) => match validate_chain_address(chain, raw) {
            Ok(value) => Some(value),
            Err(error) => return json_err(StatusCode::BAD_REQUEST, &error),
        },
        (None, true) => {
            return json_err(StatusCode::BAD_REQUEST, "this action requires `validator`")
        }
        (None, false) => None,
    };

    bc.metrics()
        .staking_validations_total
        .fetch_add(1, Ordering::Relaxed);
    json_ok(json!({
        "ok": true,
        "chain": super::chain_label(chain),
        "action": action,
        "stakeTarget": stake_target,
        "validator": validator,
        "executable": bc.config().staking_execute_enabled,
        "custody": "keyless (external signer)",
    }))
}

async fn positions_http(
    State(state): State<AppState>,
    Path((chain, address)): Path<(String, String)>,
) -> axum::response::Response {
    let bc = &state.blockchain;
    record_request(bc);
    if let Err(resp) = require_enabled(
        bc,
        bc.config().staking_enabled,
        "BLOCKCHAIN_STAKING_ENABLED",
    ) {
        return resp;
    }
    let kind = match parse_chain(&chain) {
        Ok(kind) => kind,
        Err(error) => return json_err(StatusCode::BAD_REQUEST, &error),
    };
    let address = match validate_chain_address(kind, &address) {
        Ok(value) => value,
        Err(error) => return json_err(StatusCode::BAD_REQUEST, &error),
    };
    match kind {
        ChainKind::Solana => {
            // Stake accounts owned by the Stake program, filtered by authority.
            let params = json!([
                "Stake11111111111111111111111111111111111111",
                {
                    "encoding": "jsonParsed",
                    "filters": [{ "memcmp": { "offset": 12, "bytes": address } }],
                }
            ]);
            match crate::solana_rpc(&state, "getProgramAccounts", params).await {
                Ok(result) => json_ok(
                    json!({ "ok": true, "chain": "solana", "address": address, "positions": result }),
                ),
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
            // No universal staking ABI; report the staked-balance native view.
            match bc
                .evm_rpc("eth_getBalance", json!([address, "latest"]))
                .await
            {
                Ok(result) => json_ok(json!({
                    "ok": true, "chain": "evm", "address": address, "balanceWeiHex": result,
                    "note": "per-protocol staking position queries require an ABI call via /executor/simulate",
                })),
                Err(error) => json_err(StatusCode::BAD_GATEWAY, &error),
            }
        }
    }
}

async fn intent_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<IntentRequest>,
) -> axum::response::Response {
    let bc = &state.blockchain;
    record_request(bc);
    if let Err(resp) = require_enabled(
        bc,
        bc.config().staking_execute_enabled,
        "BLOCKCHAIN_STAKING_EXECUTE_ENABLED",
    ) {
        bc.metrics()
            .broadcast_blocked_total
            .fetch_add(1, Ordering::Relaxed);
        return resp;
    }
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
    let idem_key = format!("blockchain-staking:{request_id}");
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
            state.release_idempotency_key(&idem_key);
            json_err(StatusCode::BAD_GATEWAY, &error)
        }
    }
}
