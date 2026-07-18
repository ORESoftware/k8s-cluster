//! Cross-chain bridge coordinator. Tracks a lock-on-source / release-on-dest
//! flow by recording transfer intents and verifying source-chain lock
//! confirmations (attestations) via read-only RPC. It signals when a transfer is
//! ready for destination-chain release, which is performed by an **external
//! signer** — the coordinator never custodies bridged funds and never signs.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;

use super::{
    chain_label, evict_oldest, gen_id, json_err, json_ok, parse_chain, record_request,
    require_enabled, validate_chain_address, ChainKind, MAX_BRIDGES,
};
use crate::AppState;

const MAX_AMOUNT_LEN: usize = 80;
const MAX_REF_LEN: usize = 256;

pub(crate) struct BridgeTransfer {
    pub id: String,
    pub source_chain: ChainKind,
    pub dest_chain: ChainKind,
    pub recipient: String,
    pub amount: String,
    pub status: &'static str,
    pub source_lock_ref: Option<String>,
    pub created_ms: u128,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TransferRequest {
    source_chain: String,
    dest_chain: String,
    recipient: String,
    amount: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttestRequest {
    /// Source-chain reference (Solana signature or EVM tx hash) of the lock.
    source_lock_ref: String,
}

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/bridge/transfer", post(transfer_http))
        .route("/bridge/transfer/:id", get(status_http))
        .route("/bridge/transfer/:id/attest", post(attest_http))
}

async fn transfer_http(
    State(state): State<AppState>,
    Json(body): Json<TransferRequest>,
) -> axum::response::Response {
    let bc = &state.blockchain;
    record_request(bc);
    if let Err(resp) = require_enabled(bc, bc.config().bridge_enabled, "BLOCKCHAIN_BRIDGE_ENABLED")
    {
        return resp;
    }
    let source_chain = match parse_chain(&body.source_chain) {
        Ok(kind) => kind,
        Err(error) => return json_err(StatusCode::BAD_REQUEST, &error),
    };
    let dest_chain = match parse_chain(&body.dest_chain) {
        Ok(kind) => kind,
        Err(error) => return json_err(StatusCode::BAD_REQUEST, &error),
    };
    if source_chain == dest_chain {
        return json_err(
            StatusCode::BAD_REQUEST,
            "source and destination chains must differ",
        );
    }
    let recipient = match validate_chain_address(dest_chain, &body.recipient) {
        Ok(value) => value,
        Err(error) => {
            return json_err(
                StatusCode::BAD_REQUEST,
                &format!("recipient invalid: {error}"),
            )
        }
    };
    let amount = body.amount.trim();
    if amount.is_empty()
        || amount.len() > MAX_AMOUNT_LEN
        || !amount.bytes().all(|b| b.is_ascii_digit())
    {
        return json_err(
            StatusCode::BAD_REQUEST,
            "amount must be a base-10 integer string",
        );
    }

    let id = gen_id("bridge");
    let now = crate::now_ms();
    {
        let mut bridges = match bc.inner().bridges.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        evict_oldest(&mut bridges, MAX_BRIDGES, |t| t.created_ms);
        bridges.insert(
            id.clone(),
            BridgeTransfer {
                id: id.clone(),
                source_chain,
                dest_chain,
                recipient: recipient.clone(),
                amount: amount.to_string(),
                status: "pending-lock",
                source_lock_ref: None,
                created_ms: now,
            },
        );
    }
    bc.metrics()
        .bridge_transfers_total
        .fetch_add(1, Ordering::Relaxed);
    json_ok(json!({
        "ok": true,
        "id": id,
        "sourceChain": chain_label(source_chain),
        "destChain": chain_label(dest_chain),
        "recipient": recipient,
        "amount": amount,
        "status": "pending-lock",
        "custody": "non-custodial coordinator; destination release is externally signed",
    }))
}

async fn attest_http(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<AttestRequest>,
) -> axum::response::Response {
    let bc = &state.blockchain;
    record_request(bc);
    if let Err(resp) = require_enabled(bc, bc.config().bridge_enabled, "BLOCKCHAIN_BRIDGE_ENABLED")
    {
        return resp;
    }
    let lock_ref = body.source_lock_ref.trim();
    if lock_ref.is_empty() || lock_ref.len() > MAX_REF_LEN {
        return json_err(
            StatusCode::BAD_REQUEST,
            "sourceLockRef must be 1..=256 characters",
        );
    }

    // Look up the transfer's source chain (released before any await).
    let source_chain = {
        let bridges = match bc.inner().bridges.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        match bridges.get(&id) {
            Some(t) => t.source_chain,
            None => return json_err(StatusCode::NOT_FOUND, "transfer not found"),
        }
    };

    // Verify the lock is confirmed on the source chain via read-only RPC.
    let confirmed = match verify_source_lock(&state, source_chain, lock_ref).await {
        Ok(confirmed) => confirmed,
        Err(error) => return json_err(StatusCode::BAD_GATEWAY, &error),
    };
    if !confirmed {
        return json_err(
            StatusCode::CONFLICT,
            "source lock reference is not yet confirmed on-chain",
        );
    }

    let (transfer_id, dest_chain) = {
        let mut bridges = match bc.inner().bridges.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let Some(transfer) = bridges.get_mut(&id) else {
            return json_err(StatusCode::NOT_FOUND, "transfer not found");
        };
        transfer.status = "ready-for-release";
        transfer.source_lock_ref = Some(lock_ref.to_string());
        (transfer.id.clone(), transfer.dest_chain)
    };
    bc.metrics()
        .bridge_attestations_total
        .fetch_add(1, Ordering::Relaxed);
    // Publish-only: signal readiness for the externally-signed destination release.
    crate::publish_blockchain_event(
        &state,
        &bc.config().bridge_attestations_subject,
        json!({
            "type": "blockchain.bridge.attestation",
            "id": transfer_id,
            "status": "ready-for-release",
            "sourceChain": chain_label(source_chain),
            "destChain": chain_label(dest_chain),
            "sourceLockRef": lock_ref,
            "releaseSigner": "external",
        }),
    )
    .await;
    json_ok(json!({
        "ok": true,
        "id": transfer_id,
        "status": "ready-for-release",
        "sourceLockRef": lock_ref,
        "releaseSigner": "external",
    }))
}

async fn status_http(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> axum::response::Response {
    let bc = &state.blockchain;
    record_request(bc);
    if let Err(resp) = require_enabled(bc, bc.config().bridge_enabled, "BLOCKCHAIN_BRIDGE_ENABLED")
    {
        return resp;
    }
    let bridges = match bc.inner().bridges.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    match bridges.get(&id) {
        Some(t) => json_ok(json!({
            "ok": true,
            "id": t.id,
            "sourceChain": chain_label(t.source_chain),
            "destChain": chain_label(t.dest_chain),
            "recipient": t.recipient,
            "amount": t.amount,
            "status": t.status,
            "sourceLockRef": t.source_lock_ref,
            "readyForRelease": t.status == "ready-for-release",
            "createdMs": t.created_ms.to_string(),
        })),
        None => json_err(StatusCode::NOT_FOUND, "transfer not found"),
    }
}

/// Confirms a source-chain lock reference exists and is finalized, read-only.
async fn verify_source_lock(
    state: &AppState,
    chain: ChainKind,
    lock_ref: &str,
) -> Result<bool, String> {
    let bc = &state.blockchain;
    match chain {
        ChainKind::Solana => {
            let params = json!([[lock_ref], { "searchTransactionHistory": true }]);
            let result = crate::solana_rpc(state, "getSignatureStatuses", params).await?;
            let confirmed = result
                .get("value")
                .and_then(|value| value.as_array())
                .and_then(|items| items.first())
                .map(|status| {
                    !status.is_null()
                        && status.get("err").map_or(true, Value::is_null)
                        && status
                            .get("confirmationStatus")
                            .and_then(|c| c.as_str())
                            .map_or(false, |c| c == "confirmed" || c == "finalized")
                })
                .unwrap_or(false);
            Ok(confirmed)
        }
        ChainKind::Evm => {
            if !bc.config().evm_configured() {
                return Err("EVM RPC is not configured (set EVM_RPC_URL)".to_string());
            }
            let receipt = bc
                .evm_rpc("eth_getTransactionReceipt", json!([lock_ref]))
                .await?;
            // status "0x1" means the lock tx succeeded and is mined.
            let confirmed = receipt
                .get("status")
                .and_then(|s| s.as_str())
                .map_or(false, |s| s == "0x1");
            Ok(confirmed)
        }
    }
}
