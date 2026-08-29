//! Wallet management — **watch-only**. Registers addresses with a label and
//! metadata, lists them, and reports balances by reading chain RPC. No private
//! keys are ever stored; signing stays with the caller via [`super::SignerBackend`].

use std::sync::atomic::Ordering;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{
    chain_label, evict_oldest, gen_id, json_err, json_ok, parse_chain, record_request,
    require_enabled, validate_chain_address, validate_label, ChainKind, MAX_WALLETS,
};
use crate::AppState;

pub(crate) struct WalletRecord {
    pub id: String,
    pub chain: ChainKind,
    pub address: String,
    pub label: String,
    pub created_ms: u128,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegisterRequest {
    chain: String,
    address: String,
    label: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WalletView {
    id: String,
    chain: String,
    address: String,
    label: String,
    created_ms: String,
}

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/wallet/register", post(register_http))
        .route("/wallet/list", get(list_http))
        .route("/wallet/:id/balance", post(balance_http))
}

async fn register_http(
    State(state): State<AppState>,
    Json(body): Json<RegisterRequest>,
) -> axum::response::Response {
    let bc = &state.blockchain;
    record_request(bc);
    if let Err(resp) = require_enabled(bc, bc.config().wallet_enabled, "BLOCKCHAIN_WALLET_ENABLED")
    {
        return resp;
    }
    let chain = match parse_chain(&body.chain) {
        Ok(kind) => kind,
        Err(error) => return json_err(StatusCode::BAD_REQUEST, &error),
    };
    let address = match validate_chain_address(chain, &body.address) {
        Ok(value) => value,
        Err(error) => return json_err(StatusCode::BAD_REQUEST, &error),
    };
    let label = match validate_label(&body.label, "label") {
        Ok(value) => value,
        Err(error) => return json_err(StatusCode::BAD_REQUEST, &error),
    };

    let id = gen_id("wallet");
    let now = crate::now_ms();
    {
        let mut wallets = match bc.inner().wallets.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        // Reject duplicate (chain, address) registrations.
        if wallets
            .values()
            .any(|w| w.chain == chain && w.address == address)
        {
            return json_err(
                StatusCode::CONFLICT,
                "wallet already registered for this chain",
            );
        }
        evict_oldest(&mut wallets, MAX_WALLETS, |w| w.created_ms);
        wallets.insert(
            id.clone(),
            WalletRecord {
                id: id.clone(),
                chain,
                address: address.clone(),
                label: label.clone(),
                created_ms: now,
            },
        );
    }
    bc.metrics()
        .wallets_registered_total
        .fetch_add(1, Ordering::Relaxed);
    json_ok(json!({
        "ok": true,
        "wallet": { "id": id, "chain": chain_label(chain), "address": address, "label": label },
        "custody": "watch-only",
    }))
}

async fn list_http(State(state): State<AppState>) -> axum::response::Response {
    let bc = &state.blockchain;
    record_request(bc);
    if let Err(resp) = require_enabled(bc, bc.config().wallet_enabled, "BLOCKCHAIN_WALLET_ENABLED")
    {
        return resp;
    }
    let wallets = match bc.inner().wallets.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let mut views: Vec<WalletView> = wallets
        .values()
        .map(|w| WalletView {
            id: w.id.clone(),
            chain: chain_label(w.chain).to_string(),
            address: w.address.clone(),
            label: w.label.clone(),
            created_ms: w.created_ms.to_string(),
        })
        .collect();
    views.sort_by(|a, b| b.created_ms.cmp(&a.created_ms));
    json_ok(json!({ "ok": true, "count": views.len(), "wallets": views }))
}

async fn balance_http(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> axum::response::Response {
    let bc = &state.blockchain;
    record_request(bc);
    if let Err(resp) = require_enabled(bc, bc.config().wallet_enabled, "BLOCKCHAIN_WALLET_ENABLED")
    {
        return resp;
    }
    let (chain, address) = {
        let wallets = match bc.inner().wallets.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        match wallets.get(&id) {
            Some(w) => (w.chain, w.address.clone()),
            None => return json_err(StatusCode::NOT_FOUND, "wallet not found"),
        }
    };

    match chain {
        ChainKind::Solana => {
            match crate::solana_rpc(&state, "getBalance", json!([address])).await {
                Ok(result) => json_ok(json!({
                    "ok": true, "chain": "solana", "address": address, "lamports": result,
                })),
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
            match bc
                .evm_rpc("eth_getBalance", json!([address, "latest"]))
                .await
            {
                Ok(result) => json_ok(json!({
                    "ok": true, "chain": "evm", "address": address, "weiHex": result,
                })),
                Err(error) => json_err(StatusCode::BAD_GATEWAY, &error),
            }
        }
    }
}
