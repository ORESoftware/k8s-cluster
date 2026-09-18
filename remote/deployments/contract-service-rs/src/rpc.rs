use base64::{engine::general_purpose, Engine as _};
use futures_util::StreamExt;
use serde_json::{json, Value};

use crate::coordination;
use crate::metrics::{record_rpc_error, record_rpc_request};
use crate::shared::{log_error, log_warn, now_ms};
use crate::state::{AppState, MAX_RPC_RESPONSE_BYTES, MAX_SIGNED_TRANSACTION_BYTES};
use crate::validation::normalize_encoding;

pub(crate) fn signed_transaction_bytes_from_rpc_params(params: &Value) -> Result<Vec<u8>, String> {
    const ROUTINE_ID: &str = "ores-routine-4W3IjOe6arFzEQW6oOw66";

    let transaction = match params.get(0).and_then(Value::as_str) {
        Some(transaction) => transaction.trim(),
        None => {
            log_warn(
                "solana-rpc-params-missing-transaction",
                "sendTransaction params did not begin with a signed transaction.",
                json!({
                    "oresTraceId": "ores-trace-ADprQLONqY9q4TUjizKTf",
                    "oresRoutineId": ROUTINE_ID,
                }),
            );
            return Err("sendTransaction params must begin with a signed transaction".to_string());
        }
    };
    let encoding = match normalize_encoding(
        params
            .get(1)
            .and_then(|config| config.get("encoding"))
            .and_then(Value::as_str),
    ) {
        Ok(encoding) => encoding,
        Err(error) => {
            log_warn(
                "solana-rpc-params-encoding-rejected",
                "sendTransaction params carried an unsupported encoding.",
                json!({
                    "oresTraceId": "ores-trace-hsspSaYz7fCAesV6XRGAK",
                    "oresRoutineId": ROUTINE_ID,
                }),
            );
            return Err(error);
        }
    };
    let bytes = match encoding {
        "base64" => match general_purpose::STANDARD.decode(transaction) {
            Ok(bytes) => bytes,
            Err(error) => {
                log_warn(
                    "solana-rpc-params-base64-decode-failed",
                    "sendTransaction params were not valid base64.",
                    json!({
                        "encoding": encoding,
                        "oresTraceId": "ores-trace-oX8X1BP58Nr5ROTEAvAIm",
                        "oresRoutineId": ROUTINE_ID,
                    }),
                );
                return Err(format!("transaction is not valid base64: {error}"));
            }
        },
        "base58" => match bs58::decode(transaction).into_vec() {
            Ok(bytes) => bytes,
            Err(error) => {
                log_warn(
                    "solana-rpc-params-base58-decode-failed",
                    "sendTransaction params were not valid base58.",
                    json!({
                        "encoding": encoding,
                        "oresTraceId": "ores-trace-s1r-KG0qqt-zB45TCGOPy",
                        "oresRoutineId": ROUTINE_ID,
                    }),
                );
                return Err(format!("transaction is not valid base58: {error}"));
            }
        },
        _ => unreachable!("encoding already validated"),
    };
    if bytes.is_empty() || bytes.len() > MAX_SIGNED_TRANSACTION_BYTES {
        log_warn(
            "solana-rpc-params-transaction-size-rejected",
            "sendTransaction signed transaction size was outside the accepted range.",
            json!({
                "encoding": encoding,
                "oresTraceId": "ores-trace-yF7Vi9H0XKdob6IuGBi1c",
                "oresRoutineId": ROUTINE_ID,
            }),
        );
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
    const ROUTINE_ID: &str = "ores-routine-fVPZSxu7VZg5SIDE_nzJv";

    record_rpc_request(&state.metrics, method);
    let _rpc_permit = state.rpc_slots.clone().try_acquire_owned().map_err(|_| {
        record_rpc_error(&state.metrics, method);
        log_warn(
            "solana-rpc-concurrency-limit-reached",
            "Solana RPC dispatch was rejected by the local concurrency limit.",
            json!({
                "rpcMethod": method,
                "oresTraceId": "ores-trace-39U8MKtbrsaXJAlFgjsrt",
                "oresRoutineId": ROUTINE_ID,
            }),
        );
        "solana rpc concurrency limit reached".to_string()
    })?;

    let coordination = if method == "sendTransaction" && state.coordination.enabled() {
        let signed_transaction = match signed_transaction_bytes_from_rpc_params(&params) {
            Ok(signed_transaction) => signed_transaction,
            Err(error) => {
                log_warn(
                    "solana-rpc-dispatch-params-rejected",
                    "Solana RPC dispatch rejected its sendTransaction params.",
                    json!({
                        "rpcMethod": method,
                        "oresTraceId": "ores-trace-QfsPE7fRF_jxNTZiZqUZY",
                        "oresRoutineId": ROUTINE_ID,
                    }),
                );
                return Err(error);
            }
        };
        match state
            .coordination
            .begin_broadcast(&signed_transaction)
            .await
        {
            Ok(coordination::BeginOutcome::Acquired(lease)) => Some(lease),
            Ok(coordination::BeginOutcome::Replay(result)) => return Ok(result),
            Err(error) => {
                log_error(
                    "solana-broadcast-coordination-begin-failed",
                    "Solana broadcast coordination could not begin.",
                    json!({
                        "rpcMethod": method,
                        "oresTraceId": "ores-trace-oL04yMT7U800U5zMxkJRz",
                        "oresRoutineId": ROUTINE_ID,
                    }),
                );
                return Err(error);
            }
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
                    json!({
                        "rpcMethod": method,
                        "error": error,
                        "oresTraceId": "ores-trace-lhI0NYWTt3FyJwpABYZNX",
                        "oresRoutineId": ROUTINE_ID,
                    }),
                );
            }
            Ok(result)
        }
        (Some(lease), Err(error)) => {
            lease.abandon().await;
            log_error(
                "solana-rpc-dispatch-failed-lease-abandoned",
                "Solana RPC dispatch failed and its broadcast lease was abandoned.",
                json!({
                    "rpcMethod": method,
                    "oresTraceId": "ores-trace-cam60sm1i0_QsWfgK1buM",
                    "oresRoutineId": ROUTINE_ID,
                }),
            );
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
    const ROUTINE_ID: &str = "ores-routine-Ml-CznaD4vrS0bCcGJNnI";

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
                    "error": error.without_url().to_string(),
                    "oresTraceId": "ores-trace-DAP9lrN4w-oYV5TjIxnJy",
                    "oresRoutineId": ROUTINE_ID,
                }),
            );
            "solana rpc request failed".to_string()
        })?;

    let status = response.status();
    if response.content_length().unwrap_or(0) > MAX_RPC_RESPONSE_BYTES as u64 {
        record_rpc_error(&state.metrics, method);
        log_warn(
            "solana-rpc-response-too-large",
            "Solana RPC response declared a body larger than the accepted limit.",
            json!({
                "rpcMethod": method,
                "oresTraceId": "ores-trace-T3f69-i9-BrzM6mz9VCeJ",
                "oresRoutineId": ROUTINE_ID,
            }),
        );
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
                json!({
                    "rpcMethod": method,
                    "error": error.without_url().to_string(),
                    "oresTraceId": "ores-trace-XW0L51OyU50yk0LKyR2rH",
                    "oresRoutineId": ROUTINE_ID,
                }),
            );
            "solana rpc response read failed".to_string()
        })?;
        if body_bytes.len().saturating_add(chunk.len()) > MAX_RPC_RESPONSE_BYTES {
            record_rpc_error(&state.metrics, method);
            log_warn(
                "solana-rpc-response-stream-too-large",
                "Solana RPC response body exceeded the accepted limit while streaming.",
                json!({
                    "rpcMethod": method,
                    "oresTraceId": "ores-trace-oH-0EFONCPLn936GuTvsT",
                    "oresRoutineId": ROUTINE_ID,
                }),
            );
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
                "oresTraceId": "ores-trace-7DqYgvUWMHia-vaNH3AwE",
                "oresRoutineId": ROUTINE_ID,
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
                "oresTraceId": "ores-trace-dvQzAPf4pViJxKeNHgRu0",
                "oresRoutineId": ROUTINE_ID,
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
                "oresTraceId": "ores-trace-99sr__UJG72vzA-Zmt8Rl",
                "oresRoutineId": ROUTINE_ID,
            }),
        );
        return Err(format!(
            "solana rpc {method} returned error code={code}: {message}"
        ));
    }

    Ok(body.get("result").cloned().unwrap_or(body))
}
