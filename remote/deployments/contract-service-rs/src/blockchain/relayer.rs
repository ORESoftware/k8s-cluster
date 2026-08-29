//! Transaction relayer. Accepts an **already client-signed** raw transaction,
//! records it, and (only when gated on) broadcasts it and tracks status. The
//! relayer never signs — sponsored/meta-transaction signing is deferred to the
//! custody phase. Submissions are idempotent on `requestId`.

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
    authorize_execute, evict_oldest, evm, gen_id, json_err, json_ok, parse_chain, record_request,
    require_enabled, ChainKind, MAX_RELAYS,
};
use crate::AppState;

const MAX_RAW_TX_BYTES: usize = 256 * 1024;

pub(crate) struct RelayRecord {
    pub id: String,
    pub chain: ChainKind,
    pub request_id: String,
    pub status: &'static str,
    pub reference: Option<String>,
    pub created_ms: u128,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubmitRequest {
    chain: String,
    request_id: String,
    signed_transaction: String,
}

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/relayer/submit", post(submit_http))
        .route("/relayer/status/:id", get(status_http))
}

async fn submit_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SubmitRequest>,
) -> axum::response::Response {
    let bc = &state.blockchain;
    record_request(bc);
    if let Err(resp) = require_enabled(
        bc,
        bc.config().relayer_enabled,
        "BLOCKCHAIN_RELAYER_ENABLED",
    ) {
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
    if chain == ChainKind::Evm {
        if let Err(error) = evm::validate_hex_blob(signed, "signedTransaction", MAX_RAW_TX_BYTES) {
            return json_err(StatusCode::BAD_REQUEST, &error);
        }
    }

    // Decide whether we will actually broadcast or only stage the submission.
    let broadcast_enabled = bc.config().relayer_broadcast_enabled;
    let evm_ready = chain != ChainKind::Evm || bc.config().evm_configured();
    let mut status: &'static str = "staged";
    let mut reference: Option<String> = None;
    let mut broadcast_error: Option<String> = None;

    if broadcast_enabled {
        // Broadcast requires the shared auth secret, like the executor.
        if let Err(resp) = authorize_execute(bc, &headers) {
            return resp;
        }
        if !evm_ready {
            return json_err(
                StatusCode::SERVICE_UNAVAILABLE,
                "EVM RPC is not configured (set EVM_RPC_URL)",
            );
        }
        let idem_key = format!("blockchain-relayer:{request_id}");
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
            .map(|sig| {
                sig.as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| sig.to_string())
            }),
            ChainKind::Evm => bc
                .evm_rpc("eth_sendRawTransaction", json!([signed]))
                .await
                .map(|hash| {
                    hash.as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| hash.to_string())
                }),
        };
        match outcome {
            Ok(value) => {
                status = "broadcast";
                reference = Some(value);
            }
            Err(error) => {
                state.release_idempotency_key(&idem_key);
                status = "failed";
                broadcast_error = Some(error);
            }
        }
    } else {
        bc.metrics()
            .broadcast_blocked_total
            .fetch_add(1, Ordering::Relaxed);
    }

    let id = gen_id("relay");
    let now = crate::now_ms();
    {
        let mut relays = match bc.inner().relays.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        evict_oldest(&mut relays, MAX_RELAYS, |r| r.created_ms);
        relays.insert(
            id.clone(),
            RelayRecord {
                id: id.clone(),
                chain,
                request_id: request_id.to_string(),
                status,
                reference: reference.clone(),
                created_ms: now,
            },
        );
    }
    bc.metrics()
        .relayer_submissions_total
        .fetch_add(1, Ordering::Relaxed);

    json_ok(json!({
        "ok": broadcast_error.is_none(),
        "id": id,
        "requestId": request_id,
        "status": status,
        "reference": reference,
        "broadcastEnabled": broadcast_enabled,
        "error": broadcast_error,
    }))
}

async fn status_http(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> axum::response::Response {
    let bc = &state.blockchain;
    record_request(bc);
    if let Err(resp) = require_enabled(
        bc,
        bc.config().relayer_enabled,
        "BLOCKCHAIN_RELAYER_ENABLED",
    ) {
        return resp;
    }
    let relays = match bc.inner().relays.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    match relays.get(&id) {
        Some(r) => json_ok(json!({
            "ok": true,
            "id": r.id,
            "chain": super::chain_label(r.chain),
            "requestId": r.request_id,
            "status": r.status,
            "reference": r.reference,
            "createdMs": r.created_ms.to_string(),
        })),
        None => json_err(StatusCode::NOT_FOUND, "relay not found"),
    }
}
