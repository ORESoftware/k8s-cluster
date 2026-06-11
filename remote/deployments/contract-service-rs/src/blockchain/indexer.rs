//! Blockchain indexing service. Maintains a bounded watch list of addresses and
//! a bounded ring of recently indexed events. Watched addresses can be polled on
//! demand (Solana `getSignaturesForAddress`, EVM `eth_getLogs`) and the results
//! are recorded for querying. Read-only — it never signs or broadcasts.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
use std::sync::atomic::Ordering;

use super::{
    chain_label, gen_id, json_err, json_ok, parse_chain, record_request, require_enabled,
    validate_chain_address, ChainKind, MAX_INDEX_EVENTS, MAX_WATCHES,
};
use crate::AppState;

const MAX_QUERY_LIMIT: usize = 200;
const POLL_SIGNATURE_LIMIT: u64 = 25;

pub(crate) struct WatchItem {
    // `id`/`created_ms` are recorded for the watch registry and surfaced in the
    // creation response; not all fields are read back yet in this scaffold.
    #[allow(dead_code)]
    pub id: String,
    pub chain: ChainKind,
    pub address: String,
    #[allow(dead_code)]
    pub created_ms: u128,
}

pub(crate) struct IndexedEvent {
    pub chain: ChainKind,
    pub address: String,
    pub reference: String,
    pub recorded_ms: u128,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WatchRequest {
    chain: String,
    address: String,
    /// When true and the feature is enabled, poll the chain once immediately.
    #[serde(default)]
    poll: bool,
}

#[derive(Deserialize)]
struct QueryParams {
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/index/watch", post(watch_http))
        .route("/index/query", get(query_http))
}

async fn watch_http(
    State(state): State<AppState>,
    Json(body): Json<WatchRequest>,
) -> axum::response::Response {
    let bc = &state.blockchain;
    record_request(bc);
    if let Err(resp) = require_enabled(bc, bc.config().indexer_enabled, "BLOCKCHAIN_INDEXER_ENABLED")
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

    let id = gen_id("watch");
    let now = crate::now_ms();
    {
        let mut watches = match bc.inner().watches.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if watches.iter().any(|w| w.chain == chain && w.address == address) {
            return json_err(StatusCode::CONFLICT, "address already watched");
        }
        if watches.len() >= MAX_WATCHES {
            watches.remove(0);
        }
        watches.push(WatchItem {
            id: id.clone(),
            chain,
            address: address.clone(),
            created_ms: now,
        });
    }

    // Optional one-shot poll so a watch produces something queryable immediately.
    let mut polled = 0usize;
    if body.poll {
        polled = poll_address(&state, chain, &address).await.unwrap_or(0);
    }

    json_ok(json!({
        "ok": true,
        "id": id,
        "chain": chain_label(chain),
        "address": address,
        "polledEvents": polled,
    }))
}

async fn query_http(
    State(state): State<AppState>,
    Query(params): Query<QueryParams>,
) -> axum::response::Response {
    let bc = &state.blockchain;
    record_request(bc);
    if let Err(resp) = require_enabled(bc, bc.config().indexer_enabled, "BLOCKCHAIN_INDEXER_ENABLED")
    {
        return resp;
    }
    let limit = params.limit.unwrap_or(50).clamp(1, MAX_QUERY_LIMIT);
    let filter = params.address.as_deref().map(str::trim).filter(|a| !a.is_empty());

    let events = match bc.inner().index_events.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let rows: Vec<_> = events
        .iter()
        .rev()
        .filter(|event| filter.map_or(true, |addr| event.address == addr))
        .take(limit)
        .map(|event| {
            json!({
                "chain": chain_label(event.chain),
                "address": event.address,
                "reference": event.reference,
                "recordedMs": event.recorded_ms.to_string(),
            })
        })
        .collect();
    json_ok(json!({ "ok": true, "count": rows.len(), "events": rows }))
}

/// Polls one address once and records up to `POLL_SIGNATURE_LIMIT` references.
/// Returns the number of events recorded. Reuses the parent service's Solana RPC
/// and the suite's EVM RPC; both are read-only methods.
async fn poll_address(state: &AppState, chain: ChainKind, address: &str) -> Result<usize, String> {
    let bc = &state.blockchain;
    let references: Vec<String> = match chain {
        ChainKind::Solana => {
            let params = json!([address, { "limit": POLL_SIGNATURE_LIMIT }]);
            let result = crate::solana_rpc(state, "getSignaturesForAddress", params).await?;
            result
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.get("signature").and_then(|s| s.as_str()))
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default()
        }
        ChainKind::Evm => {
            if !bc.config().evm_configured() {
                return Err("EVM RPC is not configured (set EVM_RPC_URL)".to_string());
            }
            let filter = json!([{ "address": address, "fromBlock": "latest", "toBlock": "latest" }]);
            let result = bc.evm_rpc("eth_getLogs", filter).await?;
            result
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.get("transactionHash").and_then(|h| h.as_str()))
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default()
        }
    };

    let now = crate::now_ms();
    let mut recorded = 0usize;
    let mut published: Vec<String> = Vec::new();
    {
        let mut events = match bc.inner().index_events.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        for reference in references {
            if events.len() >= MAX_INDEX_EVENTS {
                events.pop_front();
            }
            events.push_back(IndexedEvent {
                chain,
                address: address.to_string(),
                reference: reference.clone(),
                recorded_ms: now,
            });
            published.push(reference);
            recorded += 1;
        }
    }
    bc.metrics()
        .index_events_total
        .fetch_add(recorded as u64, Ordering::Relaxed);
    // Publish-only: surface indexed references on the generated NATS subject.
    if recorded > 0 {
        crate::publish_blockchain_event(
            state,
            &bc.config().index_events_subject,
            json!({
                "type": "blockchain.index.events",
                "chain": chain_label(chain),
                "address": address,
                "references": published,
                "recordedMs": now.to_string(),
            }),
        )
        .await;
    }
    Ok(recorded)
}
