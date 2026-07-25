use std::sync::atomic::Ordering;

use serde_json::{json, Value};

use crate::config::CONTRACT_SEND_AUTH_HEADER;
use crate::state::AppState;
use crate::types::EscrowSettlementRequest;

pub(crate) fn simulate_params(request: &EscrowSettlementRequest, encoding: &str, commitment: &str) -> Value {
    json!([
        request.transaction.trim(),
        {
            "encoding": encoding,
            "commitment": commitment,
            "sigVerify": false,
            "replaceRecentBlockhash": true
        }
    ])
}

pub(crate) fn send_params(request: &EscrowSettlementRequest, encoding: &str, commitment: &str) -> Value {
    let mut config = serde_json::Map::new();
    config.insert("encoding".to_string(), json!(encoding));
    config.insert(
        "skipPreflight".to_string(),
        json!(request.skip_preflight.unwrap_or(false)),
    );
    config.insert("preflightCommitment".to_string(), json!(commitment));
    if let Some(max_retries) = request.max_retries {
        config.insert("maxRetries".to_string(), json!(max_retries));
    }
    if let Some(min_context_slot) = request.min_context_slot {
        config.insert("minContextSlot".to_string(), json!(min_context_slot));
    }
    json!([request.transaction.trim(), Value::Object(config)])
}

pub(crate) async fn rpc_json(
    state: &AppState,
    method: &str,
    params: Value,
    request_id: &str,
) -> Result<Value, String> {
    state
        .metrics
        .rpc_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let payload = json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "method": method,
        "params": params,
    });
    let response = state
        .rpc_client
        .post(&state.solana_rpc_url)
        .json(&payload)
        .send()
        .await
        .map_err(|error| format!("{method} HTTP request failed: {error}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("{method} response body read failed: {error}"))?;
    if !status.is_success() {
        state
            .metrics
            .rpc_errors_total
            .fetch_add(1, Ordering::Relaxed);
        return Err(format!("{method} HTTP status {status}: {body}"));
    }
    let value = serde_json::from_str::<Value>(&body)
        .map_err(|error| format!("{method} response was not JSON: {error}"))?;
    if let Some(error) = value.get("error") {
        state
            .metrics
            .rpc_errors_total
            .fetch_add(1, Ordering::Relaxed);
        return Err(format!("{method} RPC error: {error}"));
    }
    Ok(value.get("result").cloned().unwrap_or(Value::Null))
}

#[derive(Clone, Copy)]
pub(crate) enum ContractServiceOp {
    Simulate,
    Send,
}

impl ContractServiceOp {
    pub(crate) fn path(self) -> &'static str {
        match self {
            ContractServiceOp::Simulate => "/simulate",
            ContractServiceOp::Send => "/send",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            ContractServiceOp::Simulate => "simulate",
            ContractServiceOp::Send => "send",
        }
    }
}

/// Maps an escrow settlement request onto the `dd-contract-service` `TransactionRpcRequest` body
/// (`solana.contract.v1` transaction shape). Simulate forces `sigVerify=false` with
/// `replaceRecentBlockhash=true`, matching the direct-RPC simulate path.
pub(crate) fn contract_service_body(
    request: &EscrowSettlementRequest,
    op: ContractServiceOp,
    cluster: &str,
    encoding: &str,
    commitment: &str,
    request_id: &str,
) -> Value {
    let mut body = serde_json::Map::new();
    body.insert("requestId".to_string(), json!(request_id));
    body.insert("cluster".to_string(), json!(cluster));
    body.insert("transaction".to_string(), json!(request.transaction.trim()));
    body.insert("encoding".to_string(), json!(encoding));
    body.insert("commitment".to_string(), json!(commitment));
    match op {
        ContractServiceOp::Simulate => {
            body.insert("sigVerify".to_string(), json!(false));
            body.insert("replaceRecentBlockhash".to_string(), json!(true));
        }
        ContractServiceOp::Send => {
            body.insert(
                "skipPreflight".to_string(),
                json!(request.skip_preflight.unwrap_or(false)),
            );
            if let Some(max_retries) = request.max_retries {
                body.insert("maxRetries".to_string(), json!(max_retries));
            }
            if let Some(min_context_slot) = request.min_context_slot {
                body.insert("minContextSlot".to_string(), json!(min_context_slot));
            }
        }
    }
    Value::Object(body)
}

/// Delegates an on-chain operation to the in-cluster `dd-contract-service`. The local escrow policy
/// gates (settlement enabled, auth header, intent/resolution validation) run before this is called.
pub(crate) async fn contract_service_call(
    state: &AppState,
    op: ContractServiceOp,
    body: Value,
) -> Result<Value, String> {
    let Some(base) = &state.contract_service_url else {
        state
            .metrics
            .contract_service_errors_total
            .fetch_add(1, Ordering::Relaxed);
        return Err(
            "contract-service backend is not configured with CONTRACT_SERVICE_URL".to_string(),
        );
    };
    match op {
        ContractServiceOp::Simulate => state
            .metrics
            .contract_service_simulate_total
            .fetch_add(1, Ordering::Relaxed),
        ContractServiceOp::Send => state
            .metrics
            .contract_service_send_total
            .fetch_add(1, Ordering::Relaxed),
    };
    let url = format!("{}{}", base.trim_end_matches('/'), op.path());
    let mut builder = state
        .rpc_client
        .post(&url)
        .timeout(state.contract_service_timeout)
        .json(&body);
    if matches!(op, ContractServiceOp::Send) {
        let Some(secret) = &state.contract_service_send_secret else {
            state
                .metrics
                .contract_service_errors_total
                .fetch_add(1, Ordering::Relaxed);
            return Err(
                "contract-service send requires CONTRACT_SERVICE_SEND_AUTH_SECRET".to_string(),
            );
        };
        builder = builder.header(CONTRACT_SEND_AUTH_HEADER, secret);
    }
    let response = builder.send().await.map_err(|error| {
        state
            .metrics
            .contract_service_errors_total
            .fetch_add(1, Ordering::Relaxed);
        format!("contract-service {} request failed: {error}", op.label())
    })?;
    let status = response.status();
    let text = response.text().await.map_err(|error| {
        state
            .metrics
            .contract_service_errors_total
            .fetch_add(1, Ordering::Relaxed);
        format!(
            "contract-service {} response read failed: {error}",
            op.label()
        )
    })?;
    let value = serde_json::from_str::<Value>(&text).map_err(|error| {
        state
            .metrics
            .contract_service_errors_total
            .fetch_add(1, Ordering::Relaxed);
        format!(
            "contract-service {} response was not JSON: {error}",
            op.label()
        )
    })?;
    let upstream_ok = value.get("ok").and_then(Value::as_bool).unwrap_or(false);
    if !status.is_success() || !upstream_ok {
        state
            .metrics
            .contract_service_errors_total
            .fetch_add(1, Ordering::Relaxed);
        let detail = value
            .get("error")
            .cloned()
            .unwrap_or_else(|| Value::String(text.clone()));
        return Err(format!(
            "contract-service {} failed (status {status}): {detail}",
            op.label()
        ));
    }
    Ok(value)
}

pub(crate) async fn contract_service_health(state: &AppState) -> Result<Value, String> {
    let Some(base) = &state.contract_service_url else {
        return Err("contract-service backend is not configured with CONTRACT_SERVICE_URL".to_string());
    };
    let url = format!("{}/healthz", base.trim_end_matches('/'));
    let response = state
        .rpc_client
        .get(&url)
        .timeout(state.contract_service_timeout)
        .send()
        .await
        .map_err(|error| format!("contract-service healthz request failed: {error}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("contract-service healthz read failed: {error}"))?;
    if !status.is_success() {
        return Err(format!("contract-service healthz status {status}: {body}"));
    }
    serde_json::from_str::<Value>(&body)
        .map_err(|error| format!("contract-service healthz was not JSON: {error}"))
}
