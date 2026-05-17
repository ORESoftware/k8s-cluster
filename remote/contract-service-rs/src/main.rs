use std::{
    env,
    error::Error,
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{DefaultBodyLimit, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose, Engine as _};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

const SCHEMA_VERSION: &str = "solana.contract.v1";
const MAX_HTTP_BODY_BYTES: usize = 512 * 1024;
const MAX_NATS_PAYLOAD_BYTES: usize = 512 * 1024;
const MAX_SIGNED_TRANSACTION_BYTES: usize = 256 * 1024;
const MAX_INSTRUCTIONS: usize = 16;
const MAX_ACCOUNTS_PER_INSTRUCTION: usize = 64;
const MAX_INSTRUCTION_DATA_BYTES: usize = 16 * 1024;
const MAX_MEMO_BYTES: usize = 512;
const MAX_REQUEST_ID_LEN: usize = 128;
const MAX_LABEL_LEN: usize = 64;
const DEFAULT_COMPUTE_UNITS: u32 = 200_000;
const MAX_COMPUTE_UNITS_PER_INSTRUCTION: u32 = 1_400_000;
const MAX_TRANSACTION_COMPUTE_UNITS: u64 = 1_400_000;
const MAX_SEND_RETRIES: usize = 20;

#[derive(Clone)]
struct AppState {
    rpc_client: reqwest::Client,
    solana_rpc_url: String,
    default_cluster: String,
    send_enabled: bool,
    nats: Option<async_nats::Client>,
    result_subject: String,
    event_subject: String,
    metrics: Arc<Metrics>,
}

#[derive(Default)]
struct Metrics {
    http_requests_total: AtomicU64,
    validations_total: AtomicU64,
    validation_errors_total: AtomicU64,
    rpc_requests_total: AtomicU64,
    rpc_errors_total: AtomicU64,
    nats_messages_total: AtomicU64,
    send_blocked_total: AtomicU64,
    errors_total: AtomicU64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ContractRequest {
    schema_version: String,
    request_id: Option<String>,
    cluster: Option<String>,
    program_id: String,
    payer: Option<String>,
    recent_blockhash: Option<String>,
    commitment: Option<String>,
    memo: Option<String>,
    instructions: Vec<ContractInstructionInput>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ContractInstructionInput {
    name: String,
    program_id: Option<String>,
    accounts: Vec<AccountMetaInput>,
    data_base64: Option<String>,
    data_base58: Option<String>,
    compute_units: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct AccountMetaInput {
    pubkey: String,
    is_signer: Option<bool>,
    is_writable: Option<bool>,
    label: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ContractValidationResponse {
    ok: bool,
    request_id: String,
    schema_version: String,
    cluster: String,
    program_id: String,
    instruction_count: usize,
    account_count: usize,
    estimated_compute_units: u64,
    digest: String,
    unsigned_only: bool,
    instructions: Vec<InstructionSummary>,
    warnings: Vec<String>,
    generated_at_ms: u128,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct InstructionSummary {
    name: String,
    program_id: String,
    account_count: usize,
    signer_count: usize,
    writable_count: usize,
    data_bytes: usize,
    compute_units: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TransactionRpcRequest {
    request_id: Option<String>,
    cluster: Option<String>,
    transaction: String,
    encoding: Option<String>,
    commitment: Option<String>,
    sig_verify: Option<bool>,
    replace_recent_blockhash: Option<bool>,
    skip_preflight: Option<bool>,
    max_retries: Option<usize>,
    min_context_slot: Option<u64>,
}

fn env_value(key: &str, fallback: &str) -> String {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn env_bool(key: &str, fallback: bool) -> bool {
    env::var(key)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes"
            )
        })
        .unwrap_or(fallback)
}

fn env_u64(key: &str, fallback: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn request_id(input: Option<&String>, prefix: &str) -> String {
    input
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or(prefix)
        .to_string()
}

fn normalize_cluster(input: Option<&str>, fallback: &str) -> Result<String, String> {
    let value = input
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback);
    let normalized = value.to_ascii_lowercase();
    match normalized.as_str() {
        "mainnet-beta" | "devnet" | "testnet" | "localnet" | "custom" => Ok(normalized),
        _ => Err(format!(
            "cluster must be one of mainnet-beta, devnet, testnet, localnet, or custom: {value}"
        )),
    }
}

fn normalize_commitment(input: Option<&str>) -> Result<Option<String>, String> {
    let Some(value) = input.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let normalized = value.to_ascii_lowercase();
    match normalized.as_str() {
        "processed" | "confirmed" | "finalized" => Ok(Some(normalized)),
        _ => Err(format!(
            "commitment must be processed, confirmed, or finalized: {value}"
        )),
    }
}

fn normalize_encoding(input: Option<&str>) -> Result<&'static str, String> {
    let value = input
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("base64");
    match value.to_ascii_lowercase().as_str() {
        "base64" => Ok("base64"),
        "base58" => Ok("base58"),
        _ => Err(format!("encoding must be base64 or base58: {value}")),
    }
}

fn validate_label(value: &str, label: &str, errors: &mut Vec<String>) {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        errors.push(format!("{label} must not be empty"));
        return;
    }
    if trimmed.len() > MAX_LABEL_LEN {
        errors.push(format!("{label} must be at most {MAX_LABEL_LEN} bytes"));
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        errors.push(format!(
            "{label} may contain only ASCII letters, numbers, '.', '_', and '-'"
        ));
    }
}

fn validate_pubkey(value: &str, label: &str) -> Result<(), String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{label} must not be empty"));
    }
    let decoded = bs58::decode(trimmed)
        .into_vec()
        .map_err(|error| format!("{label} must be valid base58: {error}"))?;
    if decoded.len() != 32 {
        return Err(format!(
            "{label} must decode to a 32 byte Solana public key, got {} bytes",
            decoded.len()
        ));
    }
    Ok(())
}

fn decode_instruction_data(instruction: &ContractInstructionInput) -> Result<usize, String> {
    let data_base64 = instruction
        .data_base64
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let data_base58 = instruction
        .data_base58
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    let bytes = match (data_base64, data_base58) {
        (Some(_), Some(_)) => {
            return Err("instruction data must use dataBase64 or dataBase58, not both".to_string())
        }
        (Some(value), None) => general_purpose::STANDARD
            .decode(value)
            .map_err(|error| format!("dataBase64 is not valid base64: {error}"))?,
        (None, Some(value)) => bs58::decode(value)
            .into_vec()
            .map_err(|error| format!("dataBase58 is not valid base58: {error}"))?,
        (None, None) => Vec::new(),
    };

    if bytes.len() > MAX_INSTRUCTION_DATA_BYTES {
        return Err(format!(
            "instruction data must be at most {MAX_INSTRUCTION_DATA_BYTES} bytes, got {}",
            bytes.len()
        ));
    }
    Ok(bytes.len())
}

fn contract_digest(request: &ContractRequest) -> String {
    let canonical = serde_json::to_vec(request).unwrap_or_default();
    let digest = Sha256::digest(canonical);
    format!("solana:{}", hex::encode(&digest[..16]))
}

fn validate_contract_request(
    request: &ContractRequest,
    default_cluster: &str,
) -> Result<ContractValidationResponse, Vec<String>> {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    if request.schema_version != SCHEMA_VERSION {
        errors.push(format!(
            "schemaVersion must be {SCHEMA_VERSION}, got {}",
            request.schema_version
        ));
    }

    if let Some(id) = &request.request_id {
        if id.len() > MAX_REQUEST_ID_LEN {
            errors.push(format!(
                "requestId must be at most {MAX_REQUEST_ID_LEN} bytes"
            ));
        }
    }

    let cluster = match normalize_cluster(request.cluster.as_deref(), default_cluster) {
        Ok(cluster) => cluster,
        Err(error) => {
            errors.push(error);
            default_cluster.to_string()
        }
    };

    if let Err(error) = validate_pubkey(&request.program_id, "programId") {
        errors.push(error);
    }
    if let Some(payer) = &request.payer {
        if let Err(error) = validate_pubkey(payer, "payer") {
            errors.push(error);
        }
    }
    if let Some(blockhash) = &request.recent_blockhash {
        if let Err(error) = validate_pubkey(blockhash, "recentBlockhash") {
            errors.push(error);
        }
    }
    if let Err(error) = normalize_commitment(request.commitment.as_deref()) {
        errors.push(error);
    }
    if let Some(memo) = &request.memo {
        if memo.len() > MAX_MEMO_BYTES {
            errors.push(format!("memo must be at most {MAX_MEMO_BYTES} bytes"));
        }
    }

    if request.instructions.is_empty() {
        errors.push("instructions must contain at least one instruction".to_string());
    }
    if request.instructions.len() > MAX_INSTRUCTIONS {
        errors.push(format!(
            "instructions must contain at most {MAX_INSTRUCTIONS} instructions"
        ));
    }

    let mut account_count = 0usize;
    let mut estimated_compute_units = 0u64;
    let mut summaries = Vec::new();

    for (index, instruction) in request.instructions.iter().enumerate() {
        let label = format!("instructions[{index}].name");
        validate_label(&instruction.name, &label, &mut errors);

        let program_id = instruction
            .program_id
            .as_deref()
            .unwrap_or(request.program_id.as_str())
            .trim()
            .to_string();
        if let Err(error) =
            validate_pubkey(&program_id, &format!("instructions[{index}].programId"))
        {
            errors.push(error);
        }

        if instruction.accounts.len() > MAX_ACCOUNTS_PER_INSTRUCTION {
            errors.push(format!(
                "instructions[{index}].accounts must contain at most {MAX_ACCOUNTS_PER_INSTRUCTION} accounts"
            ));
        }

        let mut signer_count = 0usize;
        let mut writable_count = 0usize;
        for (account_index, account) in instruction.accounts.iter().enumerate() {
            if let Err(error) = validate_pubkey(
                &account.pubkey,
                &format!("instructions[{index}].accounts[{account_index}].pubkey"),
            ) {
                errors.push(error);
            }
            if account.is_signer.unwrap_or(false) {
                signer_count += 1;
            }
            if account.is_writable.unwrap_or(false) {
                writable_count += 1;
            }
            if let Some(label) = &account.label {
                validate_label(
                    label,
                    &format!("instructions[{index}].accounts[{account_index}].label"),
                    &mut errors,
                );
            }
        }

        let data_bytes = match decode_instruction_data(instruction) {
            Ok(data_bytes) => data_bytes,
            Err(error) => {
                errors.push(format!("instructions[{index}]: {error}"));
                0
            }
        };

        let compute_units = instruction.compute_units.unwrap_or(DEFAULT_COMPUTE_UNITS);
        if compute_units > MAX_COMPUTE_UNITS_PER_INSTRUCTION {
            errors.push(format!(
                "instructions[{index}].computeUnits must be at most {MAX_COMPUTE_UNITS_PER_INSTRUCTION}"
            ));
        }
        estimated_compute_units += u64::from(compute_units);
        account_count += instruction.accounts.len();

        summaries.push(InstructionSummary {
            name: instruction.name.clone(),
            program_id,
            account_count: instruction.accounts.len(),
            signer_count,
            writable_count,
            data_bytes,
            compute_units,
        });
    }

    if estimated_compute_units > MAX_TRANSACTION_COMPUTE_UNITS {
        warnings.push(format!(
            "estimated compute units exceed the default Solana transaction budget of {MAX_TRANSACTION_COMPUTE_UNITS}"
        ));
    }
    if request.payer.is_none() {
        warnings
            .push("payer is not set; this service does not hold private keys or sign".to_string());
    }
    if request.recent_blockhash.is_none() {
        warnings.push(
            "recentBlockhash is not set; clients must add a fresh blockhash before signing"
                .to_string(),
        );
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    Ok(ContractValidationResponse {
        ok: true,
        request_id: request_id(request.request_id.as_ref(), "contract-validation"),
        schema_version: SCHEMA_VERSION.to_string(),
        cluster,
        program_id: request.program_id.clone(),
        instruction_count: request.instructions.len(),
        account_count,
        estimated_compute_units,
        digest: contract_digest(request),
        unsigned_only: true,
        instructions: summaries,
        warnings,
        generated_at_ms: now_ms(),
    })
}

fn validate_signed_transaction(
    request: &TransactionRpcRequest,
) -> Result<(&'static str, usize), String> {
    let encoding = normalize_encoding(request.encoding.as_deref())?;
    let payload = request.transaction.trim();
    if payload.is_empty() {
        return Err("transaction must not be empty".to_string());
    }
    let decoded_len = match encoding {
        "base64" => general_purpose::STANDARD
            .decode(payload)
            .map_err(|error| format!("transaction is not valid base64: {error}"))?
            .len(),
        "base58" => bs58::decode(payload)
            .into_vec()
            .map_err(|error| format!("transaction is not valid base58: {error}"))?
            .len(),
        _ => unreachable!("encoding already validated"),
    };
    if decoded_len > MAX_SIGNED_TRANSACTION_BYTES {
        return Err(format!(
            "transaction must be at most {MAX_SIGNED_TRANSACTION_BYTES} bytes, got {decoded_len}"
        ));
    }
    Ok((encoding, decoded_len))
}

fn simulate_params(
    request: &TransactionRpcRequest,
    encoding: &'static str,
) -> Result<Value, String> {
    let mut config = Map::new();
    config.insert("encoding".to_string(), json!(encoding));
    if let Some(commitment) = normalize_commitment(request.commitment.as_deref())? {
        config.insert("commitment".to_string(), json!(commitment));
    }
    config.insert(
        "sigVerify".to_string(),
        json!(request.sig_verify.unwrap_or(false)),
    );
    config.insert(
        "replaceRecentBlockhash".to_string(),
        json!(request.replace_recent_blockhash.unwrap_or(false)),
    );
    if let Some(min_context_slot) = request.min_context_slot {
        config.insert("minContextSlot".to_string(), json!(min_context_slot));
    }
    Ok(json!([request.transaction.trim(), Value::Object(config)]))
}

fn send_params(request: &TransactionRpcRequest, encoding: &'static str) -> Result<Value, String> {
    let max_retries = request.max_retries.unwrap_or(3);
    if max_retries > MAX_SEND_RETRIES {
        return Err(format!("maxRetries must be at most {MAX_SEND_RETRIES}"));
    }

    let mut config = Map::new();
    config.insert("encoding".to_string(), json!(encoding));
    config.insert(
        "skipPreflight".to_string(),
        json!(request.skip_preflight.unwrap_or(false)),
    );
    config.insert("maxRetries".to_string(), json!(max_retries));
    if let Some(commitment) = normalize_commitment(request.commitment.as_deref())? {
        config.insert("preflightCommitment".to_string(), json!(commitment));
    }
    if let Some(min_context_slot) = request.min_context_slot {
        config.insert("minContextSlot".to_string(), json!(min_context_slot));
    }
    Ok(json!([request.transaction.trim(), Value::Object(config)]))
}

async fn solana_rpc(state: &AppState, method: &str, params: Value) -> Result<Value, String> {
    state
        .metrics
        .rpc_requests_total
        .fetch_add(1, Ordering::Relaxed);

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
        .map_err(|error| format!("solana rpc request failed: {error}"))?;

    let status = response.status();
    let body = response
        .json::<Value>()
        .await
        .map_err(|error| format!("solana rpc response was not json: {error}"))?;

    if !status.is_success() {
        return Err(format!("solana rpc returned HTTP {status}: {body}"));
    }
    if let Some(error) = body.get("error") {
        return Err(format!("solana rpc {method} returned error: {error}"));
    }

    Ok(body.get("result").cloned().unwrap_or(body))
}

fn json_response(status: StatusCode, value: Value) -> Response {
    (status, Json(value)).into_response()
}

async fn home(State(state): State<AppState>) -> impl IntoResponse {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(json!({
        "service": "dd-contract-service",
        "runtime": "rust",
        "chain": "solana",
        "schemaVersion": SCHEMA_VERSION,
        "cluster": state.default_cluster,
        "sendEnabled": state.send_enabled,
        "routes": {
            "health": "/healthz",
            "metrics": "/metrics",
            "status": "/status",
            "schema": "/schema",
            "example": "/example",
            "validate": "POST /validate",
            "simulate": "POST /simulate",
            "send": "POST /send"
        },
        "nats": {
            "resultSubject": state.result_subject,
            "eventSubject": state.event_subject
        }
    }))
}

async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
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
        "sendEnabled": state.send_enabled
    }))
}

async fn status_http(State(state): State<AppState>) -> Response {
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
        state
            .metrics
            .rpc_errors_total
            .fetch_add(1, Ordering::Relaxed);
    }

    json_response(
        status,
        json!({
            "ok": ok,
            "service": "dd-contract-service",
            "cluster": state.default_cluster,
            "rpcHealth": health.map_err(|error| error.to_string()),
            "rpcVersion": version.map_err(|error| error.to_string()),
            "generatedAtMs": now_ms()
        }),
    )
}

async fn schema_http(State(state): State<AppState>) -> impl IntoResponse {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(contract_schema())
}

async fn example_http(State(state): State<AppState>) -> impl IntoResponse {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(contract_example())
}

async fn validate_http(
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

async fn simulate_http(
    State(state): State<AppState>,
    Json(request): Json<TransactionRpcRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);

    let cluster = match normalize_cluster(request.cluster.as_deref(), &state.default_cluster) {
        Ok(cluster) => cluster,
        Err(error) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "ok": false, "error": error }),
            )
        }
    };
    let (encoding, decoded_bytes) = match validate_signed_transaction(&request) {
        Ok(validated) => validated,
        Err(error) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "ok": false, "error": error }),
            )
        }
    };
    let params = match simulate_params(&request, encoding) {
        Ok(params) => params,
        Err(error) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "ok": false, "error": error }),
            )
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
            state
                .metrics
                .rpc_errors_total
                .fetch_add(1, Ordering::Relaxed);
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

async fn send_http(
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

    let cluster = match normalize_cluster(request.cluster.as_deref(), &state.default_cluster) {
        Ok(cluster) => cluster,
        Err(error) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "ok": false, "error": error }),
            )
        }
    };
    let (encoding, decoded_bytes) = match validate_signed_transaction(&request) {
        Ok(validated) => validated,
        Err(error) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "ok": false, "error": error }),
            )
        }
    };
    let params = match send_params(&request, encoding) {
        Ok(params) => params,
        Err(error) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "ok": false, "error": error }),
            )
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
            state
                .metrics
                .rpc_errors_total
                .fetch_add(1, Ordering::Relaxed);
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

async fn metrics(State(state): State<AppState>) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let body = format!(
        "\
# HELP dd_contract_service_http_requests_total HTTP requests handled by the Solana contract service.\n\
# TYPE dd_contract_service_http_requests_total counter\n\
dd_contract_service_http_requests_total {}\n\
# HELP dd_contract_service_validations_total Contract validation requests handled.\n\
# TYPE dd_contract_service_validations_total counter\n\
dd_contract_service_validations_total {}\n\
# HELP dd_contract_service_validation_errors_total Contract validation requests rejected.\n\
# TYPE dd_contract_service_validation_errors_total counter\n\
dd_contract_service_validation_errors_total {}\n\
# HELP dd_contract_service_rpc_requests_total Solana JSON-RPC requests sent.\n\
# TYPE dd_contract_service_rpc_requests_total counter\n\
dd_contract_service_rpc_requests_total {}\n\
# HELP dd_contract_service_rpc_errors_total Solana JSON-RPC requests that failed.\n\
# TYPE dd_contract_service_rpc_errors_total counter\n\
dd_contract_service_rpc_errors_total {}\n\
# HELP dd_contract_service_nats_messages_total NATS validation messages received.\n\
# TYPE dd_contract_service_nats_messages_total counter\n\
dd_contract_service_nats_messages_total {}\n\
# HELP dd_contract_service_send_blocked_total Raw transaction sends blocked by policy.\n\
# TYPE dd_contract_service_send_blocked_total counter\n\
dd_contract_service_send_blocked_total {}\n\
# HELP dd_contract_service_errors_total Contract service errors observed.\n\
# TYPE dd_contract_service_errors_total counter\n\
dd_contract_service_errors_total {}\n",
        state.metrics.http_requests_total.load(Ordering::Relaxed),
        state.metrics.validations_total.load(Ordering::Relaxed),
        state
            .metrics
            .validation_errors_total
            .load(Ordering::Relaxed),
        state.metrics.rpc_requests_total.load(Ordering::Relaxed),
        state.metrics.rpc_errors_total.load(Ordering::Relaxed),
        state.metrics.nats_messages_total.load(Ordering::Relaxed),
        state.metrics.send_blocked_total.load(Ordering::Relaxed),
        state.metrics.errors_total.load(Ordering::Relaxed),
    );
    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], body).into_response()
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

async fn publish_contract_result(state: &AppState, payload: Value) {
    let Some(nats) = &state.nats else {
        return;
    };
    let Ok(encoded) = serde_json::to_vec(&payload) else {
        state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
        return;
    };
    if let Err(error) = nats
        .publish(state.result_subject.clone(), encoded.into())
        .await
    {
        state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
        eprintln!("failed to publish contract result: {error}");
    }
}

async fn publish_contract_event(state: &AppState, event_type: &str, request_id: &str, ok: bool) {
    let Some(nats) = &state.nats else {
        return;
    };
    let payload = json!({
        "type": event_type,
        "source": "dd-contract-service",
        "requestId": request_id,
        "ok": ok,
        "chain": "solana",
        "atMs": now_ms(),
    });
    if let Err(error) = nats
        .publish(state.event_subject.clone(), payload.to_string().into())
        .await
    {
        state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
        eprintln!("failed to publish contract event: {error}");
    }
}

async fn run_nats_loop(state: AppState, subject: String, queue_group: String) {
    let Some(nats) = state.nats.clone() else {
        println!("contract service nats loop disabled: NATS_URL is not configured");
        return;
    };
    println!(
        "contract service nats loop starting: subject={subject} queue_group={queue_group} resultSubject={}",
        state.result_subject
    );
    let mut subscription = match nats.queue_subscribe(subject, queue_group).await {
        Ok(subscription) => subscription,
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            eprintln!("contract service nats subscribe failed: {error}");
            return;
        }
    };

    while let Some(message) = subscription.next().await {
        state
            .metrics
            .nats_messages_total
            .fetch_add(1, Ordering::Relaxed);
        let payload = message.payload.to_vec();
        if payload.len() > MAX_NATS_PAYLOAD_BYTES {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            eprintln!(
                "contract service rejected oversize nats request: bytes={} max={MAX_NATS_PAYLOAD_BYTES}",
                payload.len()
            );
            continue;
        }

        match serde_json::from_slice::<ContractRequest>(&payload) {
            Ok(request) => {
                state
                    .metrics
                    .validations_total
                    .fetch_add(1, Ordering::Relaxed);
                let request_id = request_id(request.request_id.as_ref(), "contract-validation");
                let result = match validate_contract_request(&request, &state.default_cluster) {
                    Ok(response) => {
                        json!({
                            "messageKind": "solana.contract.validation.result",
                            "source": "dd-contract-service",
                            "result": response
                        })
                    }
                    Err(errors) => {
                        state
                            .metrics
                            .validation_errors_total
                            .fetch_add(1, Ordering::Relaxed);
                        state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                        json!({
                            "messageKind": "solana.contract.validation.result",
                            "source": "dd-contract-service",
                            "result": {
                                "ok": false,
                                "requestId": request_id,
                                "errors": errors,
                                "generatedAtMs": now_ms()
                            }
                        })
                    }
                };
                let ok = result
                    .pointer("/result/ok")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                publish_contract_result(&state, result).await;
                publish_contract_event(&state, "solana.contract.validation", &request_id, ok).await;
            }
            Err(error) => {
                state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                eprintln!("contract service invalid nats request: {error}");
            }
        }
    }
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        eprintln!("failed to install Ctrl-C handler: {error}");
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let host = env_value("HOST", "0.0.0.0");
    let port = env_value("PORT", "8101");
    let default_cluster = normalize_cluster(Some(&env_value("SOLANA_CLUSTER", "devnet")), "devnet")
        .unwrap_or_else(|_| "devnet".to_string());
    let solana_rpc_url = env_value("SOLANA_RPC_URL", "https://api.devnet.solana.com");
    let send_enabled = env_bool("SOLANA_SEND_ENABLED", false);
    let rpc_timeout_seconds = env_u64("SOLANA_RPC_TIMEOUT_SECONDS", 20);
    let result_subject = env_value(
        "CONTRACT_RESULT_SUBJECT",
        "dd.remote.contracts.solana.results",
    );
    let event_subject = env_value("CONTRACT_EVENT_SUBJECT", "dd.remote.events");
    let validate_subject = env_value(
        "CONTRACT_VALIDATE_SUBJECT",
        "dd.remote.contracts.solana.validate",
    );
    let queue_group = env_value("CONTRACT_QUEUE_GROUP", "dd-contract-service");

    let rpc_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(rpc_timeout_seconds))
        .build()?;

    let nats_url = env::var("NATS_URL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let nats = match nats_url {
        Some(url) => match async_nats::connect(url.clone()).await {
            Ok(client) => Some(client),
            Err(error) => {
                eprintln!("dd-contract-service failed to connect to NATS at {url}: {error}");
                None
            }
        },
        None => None,
    };

    let state = AppState {
        rpc_client,
        solana_rpc_url,
        default_cluster,
        send_enabled,
        nats,
        result_subject,
        event_subject,
        metrics: Arc::new(Metrics::default()),
    };

    if state.nats.is_some() {
        tokio::spawn(run_nats_loop(state.clone(), validate_subject, queue_group));
    }

    let app = Router::new()
        .route("/", get(home))
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics))
        .route("/status", get(status_http))
        .route("/schema", get(schema_http))
        .route("/example", get(example_http))
        .route("/validate", post(validate_http))
        .route("/simulate", post(simulate_http))
        .route("/send", post(send_http))
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .with_state(state);

    let address: SocketAddr = format!("{host}:{port}").parse()?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    println!("dd-contract-service listening on http://{address}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}
