use std::{
    env,
    error::Error,
    net::{IpAddr, SocketAddr},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{DefaultBodyLimit, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose, Engine as _};
use dd_nats_subject_defs::{
    CONTRACTS_SOLANA_RESULTS_SUBJECT, CONTRACTS_SOLANA_VALIDATE_QUEUE_GROUP,
    CONTRACTS_SOLANA_VALIDATE_SUBJECT, RUNTIME_CRITICAL_EVENTS_SUBJECT, RUNTIME_EVENTS_SUBJECT,
};
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
const DEFAULT_COMMITMENT: &str = "confirmed";
const SEND_AUTH_HEADER: &str = "x-contract-send-auth";
const SERVICE_NAME: &str = "dd-contract-service";
const SERVICE_NAMESPACE: &str = "remote-dev";
const LOG_SCHEMA: &str = "dd.log.v1";
const LOG_SCOPE: &str = "contract-service-rs";

#[derive(Clone)]
struct AppState {
    rpc_client: reqwest::Client,
    solana_rpc_url: String,
    default_cluster: String,
    send_enabled: bool,
    send_auth_secret: Option<String>,
    allow_skip_preflight: bool,
    nats: Option<async_nats::Client>,
    result_subject: String,
    event_subject: String,
    critical_event_subject: String,
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
    nats_payload_rejected_total: AtomicU64,
    nats_results_published_total: AtomicU64,
    nats_events_published_total: AtomicU64,
    nats_critical_events_published_total: AtomicU64,
    nats_publish_errors_total: AtomicU64,
    send_blocked_total: AtomicU64,
    send_auth_failures_total: AtomicU64,
    policy_rejections_total: AtomicU64,
    errors_total: AtomicU64,
    rpc_get_health_requests_total: AtomicU64,
    rpc_get_health_errors_total: AtomicU64,
    rpc_get_version_requests_total: AtomicU64,
    rpc_get_version_errors_total: AtomicU64,
    rpc_simulate_transaction_requests_total: AtomicU64,
    rpc_simulate_transaction_errors_total: AtomicU64,
    rpc_send_transaction_requests_total: AtomicU64,
    rpc_send_transaction_errors_total: AtomicU64,
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

fn env_secret(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn now_unix_nano() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

fn severity_number(severity: &str) -> i32 {
    match severity {
        "FATAL" => 24,
        "ERROR" => 17,
        "WARN" => 13,
        "INFO" => 9,
        "DEBUG" => 5,
        _ => 1,
    }
}

fn structured_log_record(severity: &str, event_name: &str, body: &str, attributes: Value) -> Value {
    json!({
        "schema": LOG_SCHEMA,
        "time_unix_nano": now_unix_nano().to_string(),
        "severity_text": severity,
        "severity_number": severity_number(severity),
        "body": body,
        "resource_service_name": SERVICE_NAME,
        "resource_service_namespace": SERVICE_NAMESPACE,
        "scope_name": LOG_SCOPE,
        "event_name": event_name,
        "attributes": attributes,
    })
}

fn write_structured_log_to_stdout(severity: &str, event_name: &str, body: &str, attributes: Value) {
    let record = structured_log_record(severity, event_name, body, attributes);
    match serde_json::to_string(&record) {
        Ok(line) => println!("{line}"),
        Err(error) => println!(
            "{{\"schema\":\"{LOG_SCHEMA}\",\"severity_text\":\"ERROR\",\"body\":\"structured log serialization failed\",\"resource_service_name\":\"{SERVICE_NAME}\",\"event_name\":\"structured-log-serialize-failed\",\"attributes\":{{\"error\":\"{error}\"}}}}"
        ),
    }
}

fn write_structured_log_to_stderr(severity: &str, event_name: &str, body: &str, attributes: Value) {
    let record = structured_log_record(severity, event_name, body, attributes);
    match serde_json::to_string(&record) {
        Ok(line) => eprintln!("{line}"),
        Err(error) => eprintln!(
            "{{\"schema\":\"{LOG_SCHEMA}\",\"severity_text\":\"ERROR\",\"body\":\"structured log serialization failed\",\"resource_service_name\":\"{SERVICE_NAME}\",\"event_name\":\"structured-log-serialize-failed\",\"attributes\":{{\"error\":\"{error}\"}}}}"
        ),
    }
}

fn log_info(event_name: &str, body: &str, attributes: Value) {
    write_structured_log_to_stdout("INFO", event_name, body, attributes);
}

fn log_warn(event_name: &str, body: &str, attributes: Value) {
    write_structured_log_to_stderr("WARN", event_name, body, attributes);
}

fn log_error(event_name: &str, body: &str, attributes: Value) {
    write_structured_log_to_stderr("ERROR", event_name, body, attributes);
}

fn request_id(input: Option<&String>, prefix: &str) -> String {
    input
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or(prefix)
        .to_string()
}

fn validate_request_id(input: Option<&String>, errors: &mut Vec<String>) {
    let Some(value) = input else {
        return;
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        errors.push("requestId must not be empty when provided".to_string());
        return;
    }
    if trimmed.len() != value.len() {
        errors.push("requestId must not contain leading or trailing whitespace".to_string());
    }
    if trimmed.len() > MAX_REQUEST_ID_LEN {
        errors.push(format!(
            "requestId must be at most {MAX_REQUEST_ID_LEN} bytes"
        ));
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':'))
    {
        errors.push(
            "requestId may contain only ASCII letters, numbers, '.', '_', '-', and ':'".to_string(),
        );
    }
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

fn normalize_request_cluster(
    input: Option<&str>,
    configured_cluster: &str,
) -> Result<String, String> {
    let cluster = normalize_cluster(input, configured_cluster)?;
    if cluster != configured_cluster {
        return Err(format!(
            "cluster must match configured SOLANA_CLUSTER ({configured_cluster}), got {cluster}"
        ));
    }
    Ok(cluster)
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

fn normalize_commitment_or_default(input: Option<&str>) -> Result<String, String> {
    Ok(normalize_commitment(input)?.unwrap_or_else(|| DEFAULT_COMMITMENT.to_string()))
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
    if trimmed.len() != value.len() {
        return Err(format!(
            "{label} must not contain leading or trailing whitespace"
        ));
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

fn sensitive_eq(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    let mut diff = left.len() ^ right.len();
    let max_len = left.len().max(right.len());
    for index in 0..max_len {
        let left_byte = left.get(index).copied().unwrap_or(0);
        let right_byte = right.get(index).copied().unwrap_or(0);
        diff |= usize::from(left_byte ^ right_byte);
    }
    diff == 0
}

fn rpc_method_counters<'a>(metrics: &'a Metrics, method: &str) -> (&'a AtomicU64, &'a AtomicU64) {
    match method {
        "getHealth" => (
            &metrics.rpc_get_health_requests_total,
            &metrics.rpc_get_health_errors_total,
        ),
        "getVersion" => (
            &metrics.rpc_get_version_requests_total,
            &metrics.rpc_get_version_errors_total,
        ),
        "simulateTransaction" => (
            &metrics.rpc_simulate_transaction_requests_total,
            &metrics.rpc_simulate_transaction_errors_total,
        ),
        "sendTransaction" => (
            &metrics.rpc_send_transaction_requests_total,
            &metrics.rpc_send_transaction_errors_total,
        ),
        _ => (&metrics.rpc_requests_total, &metrics.rpc_errors_total),
    }
}

fn record_rpc_request(metrics: &Metrics, method: &str) {
    metrics.rpc_requests_total.fetch_add(1, Ordering::Relaxed);
    let (requests, _) = rpc_method_counters(metrics, method);
    requests.fetch_add(1, Ordering::Relaxed);
}

fn record_rpc_error(metrics: &Metrics, method: &str) {
    metrics.rpc_errors_total.fetch_add(1, Ordering::Relaxed);
    let (_, errors) = rpc_method_counters(metrics, method);
    errors.fetch_add(1, Ordering::Relaxed);
}

fn authorize_send(headers: &HeaderMap, state: &AppState) -> Result<(), (StatusCode, &'static str)> {
    let Some(secret) = &state.send_auth_secret else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "transaction sending is not configured with CONTRACT_SEND_AUTH_SECRET",
        ));
    };
    let Some(value) = headers
        .get(SEND_AUTH_HEADER)
        .and_then(|value| value.to_str().ok())
    else {
        return Err((
            StatusCode::UNAUTHORIZED,
            "missing x-contract-send-auth header",
        ));
    };
    if !sensitive_eq(value.trim(), secret) {
        return Err((
            StatusCode::UNAUTHORIZED,
            "invalid x-contract-send-auth header",
        ));
    }
    Ok(())
}

fn config_error(message: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, message.into())
}

fn validate_solana_rpc_url(raw: &str, allow_private_rpc: bool) -> Result<String, String> {
    let parsed = reqwest::Url::parse(raw)
        .map_err(|error| format!("SOLANA_RPC_URL must be an absolute URL: {error}"))?;
    match parsed.scheme() {
        "https" => {}
        "http" if allow_private_rpc => {}
        "http" => {
            return Err(
                "SOLANA_RPC_URL must use https unless SOLANA_ALLOW_PRIVATE_RPC=true".to_string(),
            )
        }
        scheme => {
            return Err(format!(
                "SOLANA_RPC_URL scheme must be https or http, got {scheme}"
            ))
        }
    }

    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("SOLANA_RPC_URL must not include credentials".to_string());
    }
    let Some(host) = parsed.host_str() else {
        return Err("SOLANA_RPC_URL must include a host".to_string());
    };

    if !allow_private_rpc {
        let host_lower = host.to_ascii_lowercase();
        if matches!(
            host_lower.as_str(),
            "localhost" | "metadata.google.internal"
        ) || host_lower.ends_with(".local")
            || host_lower.ends_with(".cluster.local")
        {
            return Err(
                "SOLANA_RPC_URL points at a private host; set SOLANA_ALLOW_PRIVATE_RPC=true to allow it"
                    .to_string(),
            );
        }
        if let Ok(ip) = host.parse::<IpAddr>() {
            let private_ip = match ip {
                IpAddr::V4(address) => {
                    address.is_private()
                        || address.is_loopback()
                        || address.is_link_local()
                        || address.is_broadcast()
                        || address.is_unspecified()
                }
                IpAddr::V6(address) => {
                    address.is_loopback()
                        || address.is_unspecified()
                        || address.is_unique_local()
                        || address.is_unicast_link_local()
                }
            };
            if private_ip {
                return Err(
                    "SOLANA_RPC_URL points at a private IP; set SOLANA_ALLOW_PRIVATE_RPC=true to allow it"
                        .to_string(),
                );
            }
        }
    }

    Ok(parsed.to_string())
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

    validate_request_id(request.request_id.as_ref(), &mut errors);

    let cluster = match normalize_request_cluster(request.cluster.as_deref(), default_cluster) {
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
    let mut errors = Vec::new();
    validate_request_id(request.request_id.as_ref(), &mut errors);
    if !errors.is_empty() {
        return Err(errors.join("; "));
    }

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
    config.insert(
        "commitment".to_string(),
        json!(normalize_commitment_or_default(
            request.commitment.as_deref()
        )?),
    );
    let sig_verify = request.sig_verify.unwrap_or(false);
    let replace_recent_blockhash = request.replace_recent_blockhash.unwrap_or(false);
    if sig_verify && replace_recent_blockhash {
        return Err(
            "sigVerify and replaceRecentBlockhash cannot both be true because blockhash replacement invalidates signatures"
                .to_string(),
        );
    }
    config.insert("sigVerify".to_string(), json!(sig_verify));
    config.insert(
        "replaceRecentBlockhash".to_string(),
        json!(replace_recent_blockhash),
    );
    if let Some(min_context_slot) = request.min_context_slot {
        config.insert("minContextSlot".to_string(), json!(min_context_slot));
    }
    Ok(json!([request.transaction.trim(), Value::Object(config)]))
}

fn send_params(
    request: &TransactionRpcRequest,
    encoding: &'static str,
    allow_skip_preflight: bool,
) -> Result<Value, String> {
    let max_retries = request.max_retries.unwrap_or(3);
    if max_retries > MAX_SEND_RETRIES {
        return Err(format!("maxRetries must be at most {MAX_SEND_RETRIES}"));
    }
    let skip_preflight = request.skip_preflight.unwrap_or(false);
    if skip_preflight && !allow_skip_preflight {
        return Err(
            "skipPreflight is disabled by policy; set SOLANA_ALLOW_SKIP_PREFLIGHT=true to permit it"
                .to_string(),
        );
    }

    let mut config = Map::new();
    config.insert("encoding".to_string(), json!(encoding));
    config.insert("skipPreflight".to_string(), json!(skip_preflight));
    config.insert("maxRetries".to_string(), json!(max_retries));
    config.insert(
        "preflightCommitment".to_string(),
        json!(normalize_commitment_or_default(
            request.commitment.as_deref()
        )?),
    );
    if let Some(min_context_slot) = request.min_context_slot {
        config.insert("minContextSlot".to_string(), json!(min_context_slot));
    }
    Ok(json!([request.transaction.trim(), Value::Object(config)]))
}

async fn solana_rpc(state: &AppState, method: &str, params: Value) -> Result<Value, String> {
    record_rpc_request(&state.metrics, method);

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
    let body = response.text().await.map_err(|error| {
        record_rpc_error(&state.metrics, method);
        log_error(
            "solana-rpc-response-read-failed",
            "Solana RPC response body could not be read.",
            json!({
                "rpcMethod": method,
                "error": error.to_string(),
            }),
        );
        "solana rpc response read failed".to_string()
    })?;
    let body = serde_json::from_str::<Value>(&body).map_err(|error| {
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
        "skipPreflightAllowed": state.allow_skip_preflight,
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
        "sendEnabled": state.send_enabled,
        "skipPreflightAllowed": state.allow_skip_preflight
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
        state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
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
            log_warn(
                "contract-validation-rejected",
                "Contract validation request was rejected.",
                json!({
                    "requestId": request_id(request.request_id.as_ref(), "contract-validation"),
                    "errorCount": errors.len(),
                }),
            );
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

    let cluster =
        match normalize_request_cluster(request.cluster.as_deref(), &state.default_cluster) {
            Ok(cluster) => cluster,
            Err(error) => {
                state
                    .metrics
                    .policy_rejections_total
                    .fetch_add(1, Ordering::Relaxed);
                log_warn(
                    "contract-simulate-policy-rejected",
                    "Signed transaction simulation was rejected by policy.",
                    json!({
                        "requestId": request_id(request.request_id.as_ref(), "contract-simulate"),
                        "reason": "cluster_mismatch",
                    }),
                );
                return json_response(
                    StatusCode::BAD_REQUEST,
                    json!({ "ok": false, "error": error }),
                );
            }
        };
    let (encoding, decoded_bytes) = match validate_signed_transaction(&request) {
        Ok(validated) => validated,
        Err(error) => {
            state
                .metrics
                .policy_rejections_total
                .fetch_add(1, Ordering::Relaxed);
            log_warn(
                "contract-simulate-policy-rejected",
                "Signed transaction simulation was rejected by policy.",
                json!({
                    "requestId": request_id(request.request_id.as_ref(), "contract-simulate"),
                    "reason": "transaction_invalid",
                    "error": error.clone(),
                }),
            );
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "ok": false, "error": error }),
            );
        }
    };
    let params = match simulate_params(&request, encoding) {
        Ok(params) => params,
        Err(error) => {
            state
                .metrics
                .policy_rejections_total
                .fetch_add(1, Ordering::Relaxed);
            log_warn(
                "contract-simulate-policy-rejected",
                "Signed transaction simulation was rejected by policy.",
                json!({
                    "requestId": request_id(request.request_id.as_ref(), "contract-simulate"),
                    "reason": "simulate_params_invalid",
                    "error": error.clone(),
                }),
            );
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "ok": false, "error": error }),
            );
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
    headers: HeaderMap,
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
        state
            .metrics
            .policy_rejections_total
            .fetch_add(1, Ordering::Relaxed);
        log_warn(
            "contract-send-disabled",
            "Raw transaction send was blocked because sending is disabled.",
            json!({
                "requestId": request_id(request.request_id.as_ref(), "contract-send"),
            }),
        );
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

    if let Err((status, error)) = authorize_send(&headers, &state) {
        state
            .metrics
            .send_blocked_total
            .fetch_add(1, Ordering::Relaxed);
        state
            .metrics
            .send_auth_failures_total
            .fetch_add(1, Ordering::Relaxed);
        state
            .metrics
            .policy_rejections_total
            .fetch_add(1, Ordering::Relaxed);
        log_warn(
            "contract-send-auth-failed",
            "Raw transaction send authorization failed.",
            json!({
                "requestId": request_id(request.request_id.as_ref(), "contract-send"),
                "status": status.as_u16(),
            }),
        );
        return json_response(
            status,
            json!({
                "ok": false,
                "requestId": request_id(request.request_id.as_ref(), "contract-send"),
                "error": error,
                "generatedAtMs": now_ms()
            }),
        );
    }

    let cluster =
        match normalize_request_cluster(request.cluster.as_deref(), &state.default_cluster) {
            Ok(cluster) => cluster,
            Err(error) => {
                state
                    .metrics
                    .policy_rejections_total
                    .fetch_add(1, Ordering::Relaxed);
                log_warn(
                    "contract-send-policy-rejected",
                    "Raw transaction send was rejected by policy.",
                    json!({
                        "requestId": request_id(request.request_id.as_ref(), "contract-send"),
                        "reason": "cluster_mismatch",
                    }),
                );
                return json_response(
                    StatusCode::BAD_REQUEST,
                    json!({ "ok": false, "error": error }),
                );
            }
        };
    let (encoding, decoded_bytes) = match validate_signed_transaction(&request) {
        Ok(validated) => validated,
        Err(error) => {
            state
                .metrics
                .policy_rejections_total
                .fetch_add(1, Ordering::Relaxed);
            log_warn(
                "contract-send-policy-rejected",
                "Raw transaction send was rejected by policy.",
                json!({
                    "requestId": request_id(request.request_id.as_ref(), "contract-send"),
                    "reason": "transaction_invalid",
                    "error": error.clone(),
                }),
            );
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "ok": false, "error": error }),
            );
        }
    };
    let params = match send_params(&request, encoding, state.allow_skip_preflight) {
        Ok(params) => params,
        Err(error) => {
            state
                .metrics
                .policy_rejections_total
                .fetch_add(1, Ordering::Relaxed);
            log_warn(
                "contract-send-policy-rejected",
                "Raw transaction send was rejected by policy.",
                json!({
                    "requestId": request_id(request.request_id.as_ref(), "contract-send"),
                    "reason": "send_params_invalid",
                    "error": error.clone(),
                }),
            );
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "ok": false, "error": error }),
            );
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

fn bool_label(value: bool) -> &'static str {
    if value {
        "true"
    } else {
        "false"
    }
}

fn metrics_body(state: &AppState) -> String {
    format!(
        "\
# HELP dd_contract_service_info Static service configuration labels for the Solana contract service.\n\
# TYPE dd_contract_service_info gauge\n\
dd_contract_service_info{{cluster=\"{}\",send_enabled=\"{}\",skip_preflight_allowed=\"{}\"}} 1\n\
# HELP dd_contract_service_http_requests_total HTTP requests handled by the Solana contract service.\n\
# TYPE dd_contract_service_http_requests_total counter\n\
dd_contract_service_http_requests_total {}\n\
# HELP dd_contract_service_validations_total Contract validation requests handled.\n\
# TYPE dd_contract_service_validations_total counter\n\
dd_contract_service_validations_total {}\n\
# HELP dd_contract_service_validation_errors_total Contract validation requests rejected.\n\
# TYPE dd_contract_service_validation_errors_total counter\n\
dd_contract_service_validation_errors_total {}\n\
# HELP dd_contract_service_policy_rejections_total Requests rejected by contract service safety policy before upstream RPC.\n\
# TYPE dd_contract_service_policy_rejections_total counter\n\
dd_contract_service_policy_rejections_total {}\n\
# HELP dd_contract_service_rpc_requests_total Solana JSON-RPC requests sent.\n\
# TYPE dd_contract_service_rpc_requests_total counter\n\
dd_contract_service_rpc_requests_total {}\n\
# HELP dd_contract_service_rpc_errors_total Solana JSON-RPC requests that failed.\n\
# TYPE dd_contract_service_rpc_errors_total counter\n\
dd_contract_service_rpc_errors_total {}\n\
# HELP dd_contract_service_rpc_requests_by_method_total Solana JSON-RPC requests sent by low-cardinality method.\n\
# TYPE dd_contract_service_rpc_requests_by_method_total counter\n\
dd_contract_service_rpc_requests_by_method_total{{rpc_method=\"getHealth\"}} {}\n\
dd_contract_service_rpc_requests_by_method_total{{rpc_method=\"getVersion\"}} {}\n\
dd_contract_service_rpc_requests_by_method_total{{rpc_method=\"simulateTransaction\"}} {}\n\
dd_contract_service_rpc_requests_by_method_total{{rpc_method=\"sendTransaction\"}} {}\n\
# HELP dd_contract_service_rpc_errors_by_method_total Solana JSON-RPC failures by low-cardinality method.\n\
# TYPE dd_contract_service_rpc_errors_by_method_total counter\n\
dd_contract_service_rpc_errors_by_method_total{{rpc_method=\"getHealth\"}} {}\n\
dd_contract_service_rpc_errors_by_method_total{{rpc_method=\"getVersion\"}} {}\n\
dd_contract_service_rpc_errors_by_method_total{{rpc_method=\"simulateTransaction\"}} {}\n\
dd_contract_service_rpc_errors_by_method_total{{rpc_method=\"sendTransaction\"}} {}\n\
# HELP dd_contract_service_nats_messages_total NATS validation messages received.\n\
# TYPE dd_contract_service_nats_messages_total counter\n\
dd_contract_service_nats_messages_total {}\n\
# HELP dd_contract_service_nats_payload_rejected_total NATS validation messages rejected before contract validation.\n\
# TYPE dd_contract_service_nats_payload_rejected_total counter\n\
dd_contract_service_nats_payload_rejected_total {}\n\
# HELP dd_contract_service_nats_published_total NATS messages published by subject kind.\n\
# TYPE dd_contract_service_nats_published_total counter\n\
dd_contract_service_nats_published_total{{subject_kind=\"result\"}} {}\n\
dd_contract_service_nats_published_total{{subject_kind=\"event\"}} {}\n\
dd_contract_service_nats_published_total{{subject_kind=\"critical\"}} {}\n\
# HELP dd_contract_service_nats_publish_errors_total NATS publish failures observed.\n\
# TYPE dd_contract_service_nats_publish_errors_total counter\n\
dd_contract_service_nats_publish_errors_total {}\n\
# HELP dd_contract_service_send_blocked_total Raw transaction sends blocked by policy.\n\
# TYPE dd_contract_service_send_blocked_total counter\n\
dd_contract_service_send_blocked_total {}\n\
# HELP dd_contract_service_send_auth_failures_total Raw transaction send attempts rejected by the send auth header check.\n\
# TYPE dd_contract_service_send_auth_failures_total counter\n\
dd_contract_service_send_auth_failures_total {}\n\
# HELP dd_contract_service_errors_total Contract service errors observed.\n\
# TYPE dd_contract_service_errors_total counter\n\
dd_contract_service_errors_total {}\n",
        state.default_cluster,
        bool_label(state.send_enabled),
        bool_label(state.allow_skip_preflight),
        state.metrics.http_requests_total.load(Ordering::Relaxed),
        state.metrics.validations_total.load(Ordering::Relaxed),
        state
            .metrics
            .validation_errors_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .policy_rejections_total
            .load(Ordering::Relaxed),
        state.metrics.rpc_requests_total.load(Ordering::Relaxed),
        state.metrics.rpc_errors_total.load(Ordering::Relaxed),
        state
            .metrics
            .rpc_get_health_requests_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .rpc_get_version_requests_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .rpc_simulate_transaction_requests_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .rpc_send_transaction_requests_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .rpc_get_health_errors_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .rpc_get_version_errors_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .rpc_simulate_transaction_errors_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .rpc_send_transaction_errors_total
            .load(Ordering::Relaxed),
        state.metrics.nats_messages_total.load(Ordering::Relaxed),
        state
            .metrics
            .nats_payload_rejected_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .nats_results_published_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .nats_events_published_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .nats_critical_events_published_total
            .load(Ordering::Relaxed),
        state
            .metrics
            .nats_publish_errors_total
            .load(Ordering::Relaxed),
        state.metrics.send_blocked_total.load(Ordering::Relaxed),
        state
            .metrics
            .send_auth_failures_total
            .load(Ordering::Relaxed),
        state.metrics.errors_total.load(Ordering::Relaxed),
    )
}

async fn metrics(State(state): State<AppState>) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let body = metrics_body(&state);
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

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;

    const SYSTEM_PROGRAM: &str = "11111111111111111111111111111111";

    fn sample_contract_request() -> ContractRequest {
        ContractRequest {
            schema_version: SCHEMA_VERSION.to_string(),
            request_id: Some("contract-demo".to_string()),
            cluster: Some("devnet".to_string()),
            program_id: SYSTEM_PROGRAM.to_string(),
            payer: Some(SYSTEM_PROGRAM.to_string()),
            recent_blockhash: Some(SYSTEM_PROGRAM.to_string()),
            commitment: Some("confirmed".to_string()),
            memo: Some("example".to_string()),
            instructions: vec![ContractInstructionInput {
                name: "system-transfer-shape".to_string(),
                program_id: None,
                accounts: vec![AccountMetaInput {
                    pubkey: SYSTEM_PROGRAM.to_string(),
                    is_signer: Some(true),
                    is_writable: Some(true),
                    label: Some("from".to_string()),
                }],
                data_base64: Some("AQID".to_string()),
                data_base58: None,
                compute_units: Some(DEFAULT_COMPUTE_UNITS),
            }],
        }
    }

    fn sample_state() -> AppState {
        AppState {
            rpc_client: reqwest::Client::new(),
            solana_rpc_url: "https://api.devnet.solana.com".to_string(),
            default_cluster: "devnet".to_string(),
            send_enabled: true,
            send_auth_secret: Some("secret".to_string()),
            allow_skip_preflight: false,
            nats: None,
            result_subject: "results".to_string(),
            event_subject: "events".to_string(),
            critical_event_subject: "events.critical".to_string(),
            metrics: Arc::new(Metrics::default()),
        }
    }

    #[test]
    fn contract_validation_rejects_cluster_drift() {
        let mut request = sample_contract_request();
        request.cluster = Some("mainnet-beta".to_string());

        let errors = validate_contract_request(&request, "devnet").expect_err("must reject drift");

        assert!(errors
            .iter()
            .any(|error| error.contains("cluster must match configured SOLANA_CLUSTER")));
    }

    #[test]
    fn request_ids_are_restricted() {
        let mut request = sample_contract_request();
        request.request_id = Some("bad id\n".to_string());

        let errors = validate_contract_request(&request, "devnet").expect_err("must reject id");

        assert!(errors
            .iter()
            .any(|error| error.contains("requestId may contain only ASCII")));
    }

    #[test]
    fn rpc_url_policy_blocks_private_http_by_default() {
        assert!(validate_solana_rpc_url("https://api.devnet.solana.com", false).is_ok());
        assert!(validate_solana_rpc_url("http://127.0.0.1:8899", false).is_err());
        assert!(validate_solana_rpc_url("http://127.0.0.1:8899", true).is_ok());
        assert!(validate_solana_rpc_url("https://user:pass@example.com", false).is_err());
        assert!(validate_solana_rpc_url("https://10.0.0.10:8899", false).is_err());
        assert!(validate_solana_rpc_url("https://169.254.169.254/latest", false).is_err());
        assert!(
            validate_solana_rpc_url("https://solana-rpc.default.svc.cluster.local", false).is_err()
        );
    }

    #[test]
    fn simulate_rejects_signature_verify_with_blockhash_replacement() {
        let request = TransactionRpcRequest {
            request_id: Some("simulate-demo".to_string()),
            cluster: Some("devnet".to_string()),
            transaction: general_purpose::STANDARD.encode([1_u8, 2, 3]),
            encoding: Some("base64".to_string()),
            commitment: None,
            sig_verify: Some(true),
            replace_recent_blockhash: Some(true),
            skip_preflight: None,
            max_retries: None,
            min_context_slot: None,
        };

        let error = simulate_params(&request, "base64").expect_err("must reject invalid combo");

        assert!(error.contains("sigVerify and replaceRecentBlockhash cannot both be true"));
    }

    #[test]
    fn send_params_blocks_skip_preflight_by_default() {
        let request = TransactionRpcRequest {
            request_id: Some("send-demo".to_string()),
            cluster: Some("devnet".to_string()),
            transaction: general_purpose::STANDARD.encode([1_u8, 2, 3]),
            encoding: Some("base64".to_string()),
            commitment: None,
            sig_verify: None,
            replace_recent_blockhash: None,
            skip_preflight: Some(true),
            max_retries: Some(3),
            min_context_slot: None,
        };

        let error = send_params(&request, "base64", false).expect_err("must block skip");

        assert!(error.contains("skipPreflight is disabled by policy"));
        assert!(send_params(&request, "base64", true).is_ok());
    }

    #[test]
    fn send_params_rejects_excessive_retries() {
        let request = TransactionRpcRequest {
            request_id: Some("send-demo".to_string()),
            cluster: Some("devnet".to_string()),
            transaction: general_purpose::STANDARD.encode([1_u8, 2, 3]),
            encoding: Some("base64".to_string()),
            commitment: None,
            sig_verify: None,
            replace_recent_blockhash: None,
            skip_preflight: None,
            max_retries: Some(MAX_SEND_RETRIES + 1),
            min_context_slot: None,
        };

        let error = send_params(&request, "base64", false).expect_err("must reject retries");

        assert!(error.contains("maxRetries must be at most"));
    }

    #[test]
    fn signed_transaction_rejects_oversized_payload() {
        let request = TransactionRpcRequest {
            request_id: Some("simulate-demo".to_string()),
            cluster: Some("devnet".to_string()),
            transaction: general_purpose::STANDARD
                .encode(vec![7_u8; MAX_SIGNED_TRANSACTION_BYTES + 1]),
            encoding: Some("base64".to_string()),
            commitment: None,
            sig_verify: None,
            replace_recent_blockhash: None,
            skip_preflight: None,
            max_retries: None,
            min_context_slot: None,
        };

        let error = validate_signed_transaction(&request).expect_err("must reject oversize tx");

        assert!(error.contains("transaction must be at most"));
    }

    #[test]
    fn contract_validation_rejects_dual_instruction_data_encodings() {
        let mut request = sample_contract_request();
        request.instructions[0].data_base58 = Some("111".to_string());

        let errors =
            validate_contract_request(&request, "devnet").expect_err("must reject dual encoding");

        assert!(errors
            .iter()
            .any(|error| error.contains("dataBase64 or dataBase58, not both")));
    }

    #[test]
    fn send_auth_requires_matching_header() {
        let state = sample_state();
        let mut headers = HeaderMap::new();

        assert!(authorize_send(&headers, &state).is_err());
        headers.insert(SEND_AUTH_HEADER, "secret".parse().unwrap());
        assert!(authorize_send(&headers, &state).is_ok());
        headers.insert(SEND_AUTH_HEADER, "wrong".parse().unwrap());
        assert!(authorize_send(&headers, &state).is_err());
    }

    #[test]
    fn structured_log_record_matches_shared_contract() {
        let record = structured_log_record(
            "WARN",
            "contract-test-event",
            "contract test body",
            json!({ "rpcMethod": "simulateTransaction" }),
        );

        assert_eq!(record["schema"], LOG_SCHEMA);
        assert_eq!(record["severity_text"], "WARN");
        assert_eq!(record["severity_number"], 13);
        assert_eq!(record["resource_service_name"], SERVICE_NAME);
        assert_eq!(record["resource_service_namespace"], SERVICE_NAMESPACE);
        assert_eq!(record["scope_name"], LOG_SCOPE);
        assert_eq!(record["event_name"], "contract-test-event");
        assert_eq!(record["attributes"]["rpcMethod"], "simulateTransaction");
        assert!(record["time_unix_nano"].as_str().is_some());
    }

    #[test]
    fn metrics_body_includes_rpc_and_nats_breakdowns() {
        let state = sample_state();
        record_rpc_request(&state.metrics, "simulateTransaction");
        record_rpc_error(&state.metrics, "simulateTransaction");
        state
            .metrics
            .nats_results_published_total
            .fetch_add(2, Ordering::Relaxed);
        state
            .metrics
            .nats_critical_events_published_total
            .fetch_add(1, Ordering::Relaxed);

        let body = metrics_body(&state);

        assert!(body.contains("dd_contract_service_info{cluster=\"devnet\""));
        assert!(body.contains(
            "dd_contract_service_rpc_requests_by_method_total{rpc_method=\"simulateTransaction\"} 1"
        ));
        assert!(body.contains(
            "dd_contract_service_rpc_errors_by_method_total{rpc_method=\"simulateTransaction\"} 1"
        ));
        assert!(
            body.contains("dd_contract_service_nats_published_total{subject_kind=\"result\"} 2")
        );
        assert!(
            body.contains("dd_contract_service_nats_published_total{subject_kind=\"critical\"} 1")
        );
    }
}

async fn publish_contract_result(state: &AppState, payload: Value) {
    let Some(nats) = &state.nats else {
        return;
    };
    let Ok(encoded) = serde_json::to_vec(&payload) else {
        state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
        log_error(
            "contract-result-serialize-failed",
            "Contract validation result could not be serialized for NATS.",
            json!({}),
        );
        return;
    };
    if let Err(error) = nats
        .publish(state.result_subject.clone(), encoded.into())
        .await
    {
        state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
        state
            .metrics
            .nats_publish_errors_total
            .fetch_add(1, Ordering::Relaxed);
        publish_runtime_critical_event(
            state,
            "contract-result-publish-failed",
            "Contract validation result NATS publish failed.",
            json!({
                "subject": &state.result_subject,
                "error": error.to_string(),
            }),
        )
        .await;
    } else {
        state
            .metrics
            .nats_results_published_total
            .fetch_add(1, Ordering::Relaxed);
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
        state
            .metrics
            .nats_publish_errors_total
            .fetch_add(1, Ordering::Relaxed);
        log_warn(
            "contract-event-publish-failed",
            "Contract lifecycle event NATS publish failed.",
            json!({
                "subject": &state.event_subject,
                "eventType": event_type,
                "requestId": request_id,
                "error": error.to_string(),
            }),
        );
    } else {
        state
            .metrics
            .nats_events_published_total
            .fetch_add(1, Ordering::Relaxed);
    }
}

async fn publish_runtime_critical_event(
    state: &AppState,
    event_name: &str,
    body: &str,
    attributes: Value,
) {
    log_error(event_name, body, attributes.clone());
    let Some(nats) = &state.nats else {
        return;
    };
    let log = structured_log_record("ERROR", event_name, body, attributes);
    let payload = json!({
        "type": "runtime-critical-event",
        "schema": "dd.runtime_critical_event.v1",
        "source": SERVICE_NAME,
        "eventName": event_name,
        "severity": "ERROR",
        "log": log,
        "emittedAtMs": now_ms(),
    });
    match serde_json::to_vec(&payload) {
        Ok(encoded) => match nats
            .publish(state.critical_event_subject.clone(), encoded.into())
            .await
        {
            Ok(()) => {
                state
                    .metrics
                    .nats_critical_events_published_total
                    .fetch_add(1, Ordering::Relaxed);
            }
            Err(error) => {
                state
                    .metrics
                    .nats_publish_errors_total
                    .fetch_add(1, Ordering::Relaxed);
                log_error(
                    "contract-critical-event-publish-failed",
                    "Contract service critical event NATS publish failed.",
                    json!({
                        "subject": &state.critical_event_subject,
                        "eventName": event_name,
                        "error": error.to_string(),
                    }),
                );
            }
        },
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            log_error(
                "contract-critical-event-serialize-failed",
                "Contract service critical event payload serialization failed.",
                json!({
                    "eventName": event_name,
                    "error": error.to_string(),
                }),
            );
        }
    }
}

async fn run_nats_loop(state: AppState, subject: String, queue_group: String) {
    let Some(nats) = state.nats.clone() else {
        log_info(
            "contract-nats-loop-disabled",
            "Contract service NATS loop is disabled because NATS_URL is not configured.",
            json!({}),
        );
        return;
    };
    log_info(
        "contract-nats-loop-starting",
        "Contract service NATS validation loop is starting.",
        json!({
            "subject": &subject,
            "queueGroup": &queue_group,
            "resultSubject": &state.result_subject,
            "eventSubject": &state.event_subject,
            "criticalEventSubject": &state.critical_event_subject,
        }),
    );
    let mut subscription = match nats.queue_subscribe(subject, queue_group).await {
        Ok(subscription) => subscription,
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            publish_runtime_critical_event(
                &state,
                "contract-nats-subscribe-failed",
                "Contract service could not subscribe to validation requests.",
                json!({
                    "error": error.to_string(),
                }),
            )
            .await;
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
            state
                .metrics
                .nats_payload_rejected_total
                .fetch_add(1, Ordering::Relaxed);
            publish_runtime_critical_event(
                &state,
                "contract-nats-payload-too-large",
                "Contract service rejected an oversized NATS validation request.",
                json!({
                    "payloadBytes": payload.len(),
                    "maxPayloadBytes": MAX_NATS_PAYLOAD_BYTES,
                }),
            )
            .await;
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
                state
                    .metrics
                    .nats_payload_rejected_total
                    .fetch_add(1, Ordering::Relaxed);
                publish_runtime_critical_event(
                    &state,
                    "contract-nats-payload-invalid",
                    "Contract service rejected an invalid NATS validation request.",
                    json!({
                        "error": error.to_string(),
                    }),
                )
                .await;
            }
        }
    }
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        log_error(
            "contract-shutdown-signal-failed",
            "Contract service failed while waiting for Ctrl-C.",
            json!({ "error": error.to_string() }),
        );
    }
}

async fn api_docs_html() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("../generated/api-docs.html"))
}

async fn api_docs_json() -> impl axum::response::IntoResponse {
    (
        [("content-type", "application/json; charset=utf-8")],
        include_str!("../generated/api-docs.json"),
    )
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let host = env_value("HOST", "0.0.0.0");
    let port = env_value("PORT", "8101");
    let configured_cluster = env_value("SOLANA_CLUSTER", "devnet");
    let default_cluster =
        normalize_cluster(Some(&configured_cluster), "devnet").map_err(config_error)?;
    let allow_private_rpc = env_bool("SOLANA_ALLOW_PRIVATE_RPC", false);
    let solana_rpc_url = validate_solana_rpc_url(
        &env_value("SOLANA_RPC_URL", "https://api.devnet.solana.com"),
        allow_private_rpc,
    )
    .map_err(config_error)?;
    let send_enabled = env_bool("SOLANA_SEND_ENABLED", false);
    let send_auth_secret = env_secret("CONTRACT_SEND_AUTH_SECRET");
    if send_enabled && send_auth_secret.is_none() {
        return Err(
            config_error("SOLANA_SEND_ENABLED=true requires CONTRACT_SEND_AUTH_SECRET").into(),
        );
    }
    let allow_skip_preflight = env_bool("SOLANA_ALLOW_SKIP_PREFLIGHT", false);
    let rpc_timeout_seconds = env_u64("SOLANA_RPC_TIMEOUT_SECONDS", 20);
    let result_subject = env_value("CONTRACT_RESULT_SUBJECT", CONTRACTS_SOLANA_RESULTS_SUBJECT);
    let event_subject = env_value("CONTRACT_EVENT_SUBJECT", RUNTIME_EVENTS_SUBJECT);
    let critical_event_subject = env_value(
        "NATS_CRITICAL_EVENT_SUBJECT",
        RUNTIME_CRITICAL_EVENTS_SUBJECT,
    );
    let validate_subject = env_value(
        "CONTRACT_VALIDATE_SUBJECT",
        CONTRACTS_SOLANA_VALIDATE_SUBJECT,
    );
    let queue_group = env_value(
        "CONTRACT_QUEUE_GROUP",
        CONTRACTS_SOLANA_VALIDATE_QUEUE_GROUP,
    );

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
                log_error(
                    "contract-nats-connect-failed",
                    "Contract service failed to connect to NATS.",
                    json!({
                        "url": url,
                        "error": error.to_string(),
                    }),
                );
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
        send_auth_secret,
        allow_skip_preflight,
        nats,
        result_subject,
        event_subject,
        critical_event_subject,
        metrics: Arc::new(Metrics::default()),
    };

    log_info(
        "contract-service-starting",
        "Contract service runtime configuration loaded.",
        json!({
            "cluster": &state.default_cluster,
            "sendEnabled": state.send_enabled,
            "skipPreflightAllowed": state.allow_skip_preflight,
            "resultSubject": &state.result_subject,
            "eventSubject": &state.event_subject,
            "criticalEventSubject": &state.critical_event_subject,
            "natsEnabled": state.nats.is_some(),
        }),
    );

    if state.nats.is_some() {
        tokio::spawn(run_nats_loop(state.clone(), validate_subject, queue_group));
    }

    let app = Router::new()
        .route("/", get(home))
        .route("/healthz", get(healthz))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .route("/metrics", get(metrics))
        .route("/status", get(status_http))
        .route("/schema", get(schema_http))
        .route("/example", get(example_http))
        .route("/validate", post(validate_http))
        .route("/simulate", post(simulate_http))
        .route("/send", post(send_http))
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .with_state(state)
        .merge(dd_runtime_config_client::router());

    tokio::spawn(dd_runtime_config_client::register_with_control_plane());

    let address: SocketAddr = format!("{host}:{port}").parse()?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    log_info(
        "contract-service-listening",
        "Contract service HTTP listener is ready.",
        json!({
            "address": address.to_string(),
        }),
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}
