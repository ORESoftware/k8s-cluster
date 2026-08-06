use base64::{engine::general_purpose, Engine as _};
use futures_util::StreamExt;
use serde_json::{json, Value};

use crate::coordination;
use crate::metrics::{record_rpc_error, record_rpc_request};
use crate::shared::{log_error, log_warn, now_ms};
use crate::state::{AppState, MAX_RPC_RESPONSE_BYTES, MAX_SIGNED_TRANSACTION_BYTES};
use crate::validation::normalize_encoding;

pub(crate) fn signed_transaction_bytes_from_rpc_params(params: &Value) -> Result<Vec<u8>, String> {
    let transaction = params
        .get(0)
        .and_then(Value::as_str)
        .ok_or_else(|| "sendTransaction params must begin with a signed transaction".to_string())?
        .trim();
    let encoding = normalize_encoding(
        params
            .get(1)
            .and_then(|config| config.get("encoding"))
            .and_then(Value::as_str),
    )?;
    let bytes = match encoding {
        "base64" => general_purpose::STANDARD
            .decode(transaction)
            .map_err(|error| format!("transaction is not valid base64: {error}"))?,
        "base58" => bs58::decode(transaction)
            .into_vec()
            .map_err(|error| format!("transaction is not valid base58: {error}"))?,
        _ => unreachable!("encoding already validated"),
    };
    if bytes.is_empty() || bytes.len() > MAX_SIGNED_TRANSACTION_BYTES {
        return Err(format!(
            "signed transaction must be 1..={MAX_SIGNED_TRANSACTION_BYTES} bytes"
        ));
    }
    Ok(bytes)
}

pub(crate) async fn solana_rpc(
    state: &AppState,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    record_rpc_request(&state.metrics, method);
    let _rpc_permit = state.rpc_slots.clone().try_acquire_owned().map_err(|_| {
        record_rpc_error(&state.metrics, method);
        "solana rpc concurrency limit reached".to_string()
    })?;

    let coordination = if method == "sendTransaction" && state.coordination.enabled() {
        let signed_transaction = signed_transaction_bytes_from_rpc_params(&params)?;
        match state
            .coordination
            .begin_broadcast(&signed_transaction)
            .await?
        {
            coordination::BeginOutcome::Acquired(lease) => Some(lease),
            coordination::BeginOutcome::Replay(result) => return Ok(result),
        }
    } else {
        None
    };

    let result = solana_rpc_request(state, method, params).await;
    match (coordination, result) {
        (Some(lease), Ok(result)) => {
            if let Err(error) = lease.complete(&result).await {
                log_error(
                    "solana-broadcast-coordination-complete-failed",
                    "Solana broadcast succeeded but its Fiducia idempotency record did not complete.",
                    json!({ "rpcMethod": method, "error": error }),
                );
            }
            Ok(result)
        }
        (Some(lease), Err(error)) => {
            lease.abandon().await;
            Err(error)
        }
        (None, result) => result,
    }
}

async fn solana_rpc_request(
    state: &AppState,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": format!("dd-contract-service-{}", now_ms()),
        "method": method,
        "params": params,
    });

    let response = state
        .rpc_client
        .post(&state.solana_rpc_url)
        .json(&payload)
        .send()
        .await
        .map_err(|error| {
            record_rpc_error(&state.metrics, method);
            log_error(
                "solana-rpc-request-failed",
                "Solana RPC request failed.",
                json!({
                    "rpcMethod": method,
                    "error": error.to_string(),
                }),
            );
            "solana rpc request failed".to_string()
        })?;

    let status = response.status();
    if response.content_length().unwrap_or(0) > MAX_RPC_RESPONSE_BYTES as u64 {
        record_rpc_error(&state.metrics, method);
        return Err("solana rpc response exceeded size limit".to_string());
    }
    let mut stream = response.bytes_stream();
    let mut body_bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| {
            record_rpc_error(&state.metrics, method);
            log_error(
                "solana-rpc-response-read-failed",
                "Solana RPC response body could not be read.",
                json!({ "rpcMethod": method, "error": error.to_string() }),
            );
            "solana rpc response read failed".to_string()
        })?;
        if body_bytes.len().saturating_add(chunk.len()) > MAX_RPC_RESPONSE_BYTES {
            record_rpc_error(&state.metrics, method);
            return Err("solana rpc response exceeded size limit".to_string());
        }
        body_bytes.extend_from_slice(&chunk);
    }
    let body = serde_json::from_slice::<Value>(&body_bytes).map_err(|error| {
        record_rpc_error(&state.metrics, method);
        log_error(
            "solana-rpc-response-json-failed",
            "Solana RPC response body was not JSON.",
            json!({
                "rpcMethod": method,
                "error": error.to_string(),
            }),
        );
        "solana rpc response was not json".to_string()
    })?;

    if !status.is_success() {
        record_rpc_error(&state.metrics, method);
        log_warn(
            "solana-rpc-http-error",
            "Solana RPC returned a non-success HTTP status.",
            json!({
                "rpcMethod": method,
                "status": status.as_u16(),
            }),
        );
        return Err(format!("solana rpc returned HTTP {status}"));
    }
    if let Some(error) = body.get("error") {
        let code = error
            .get("code")
            .and_then(Value::as_i64)
            .map(|code| code.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("upstream rpc error");
        record_rpc_error(&state.metrics, method);
        log_warn(
            "solana-rpc-upstream-error",
            "Solana RPC returned an upstream JSON-RPC error.",
            json!({
                "rpcMethod": method,
                "rpcErrorCode": code,
            }),
        );
        return Err(format!(
            "solana rpc {method} returned error code={code}: {message}"
        ));
    }

    Ok(body.get("result").cloned().unwrap_or(body))
}
