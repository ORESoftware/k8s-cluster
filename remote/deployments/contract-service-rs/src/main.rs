#![recursion_limit = "256"]

use std::{
    collections::HashMap,
    env,
    error::Error,
    net::{IpAddr, SocketAddr},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{DefaultBodyLimit, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose, Engine as _};
use dd_nats_subject_defs::{
    CONTRACTS_SOLANA_RESOLVE_QUEUE_GROUP, CONTRACTS_SOLANA_RESOLVE_SUBJECT,
    CONTRACTS_SOLANA_RESULTS_SUBJECT, CONTRACTS_SOLANA_SETTLEMENT_RESULTS_SUBJECT,
    CONTRACTS_SOLANA_SETTLE_QUEUE_GROUP, CONTRACTS_SOLANA_SETTLE_SUBJECT,
    CONTRACTS_SOLANA_VALIDATE_QUEUE_GROUP, CONTRACTS_SOLANA_VALIDATE_SUBJECT,
    ESCROW_SOLANA_RESULTS_SUBJECT, RUNTIME_CRITICAL_EVENTS_SUBJECT, RUNTIME_EVENTS_SUBJECT,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

mod blockchain;
mod coordination;
mod solana_features;

const SCHEMA_VERSION: &str = "solana.contract.v1";
const MAX_HTTP_BODY_BYTES: usize = 512 * 1024;
const MAX_NATS_PAYLOAD_BYTES: usize = 512 * 1024;
const MAX_SIGNED_TRANSACTION_BYTES: usize = 256 * 1024;
const MAX_RPC_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_RPC_IN_FLIGHT: usize = 64;
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
const SETTLEMENT_AUTH_HEADER: &str = "x-contract-settlement-auth";
const SETTLEMENT_SCHEMA_VERSION: &str = "solana.settlement.v1";
const RESOLUTION_SCHEMA_VERSION: &str = "solana.resolution.v1";
const MAX_SIGNATURE_LEN: usize = 96;
const MAX_RATIONALE_BYTES: usize = 2048;
const MAX_RENT_EXEMPTION_BYTES: u64 = 10 * 1024 * 1024;
const MAX_CONFIRM_SIGNATURES: usize = 8;
const DEFAULT_CONFIRM_TIMEOUT_MS: u64 = 30_000;
const MAX_CONFIRM_TIMEOUT_MS: u64 = 120_000;
const MIN_CONFIRM_POLL_INTERVAL_MS: u64 = 250;
const MAX_CONFIRM_POLL_INTERVAL_MS: u64 = 10_000;
const DEFAULT_CONFIRM_POLL_INTERVAL_MS: u64 = 1_500;
const MAX_CONFIRM_POLLS: u32 = 240;
const IDEMPOTENCY_TTL_MS: u128 = 10 * 60 * 1000;
const MAX_IDEMPOTENCY_ENTRIES: usize = 8_192;
// Service-wide cap on concurrent confirmation pollers across /confirm, /settle,
// /resolve, and the escrow-results verifier. Bounds sustained outbound Solana
// RPC fan-out so no set of requests (nor a flood of escrow result messages on
// the currently unauthenticated NATS bus) can amplify load on the upstream RPC
// endpoint; excess confirmations are shed and reported as "deferred".
const MAX_CONFIRM_POLLERS_IN_FLIGHT: u64 = 64;
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
    settlement_enabled: bool,
    resolution_enabled: bool,
    nats_settlement_enabled: bool,
    mainnet_settlement_enabled: bool,
    settlement_auth_secret: Option<String>,
    nats: Option<async_nats::Client>,
    result_subject: String,
    settlement_result_subject: String,
    event_subject: String,
    critical_event_subject: String,
    metrics: Arc<Metrics>,
    idempotency: Arc<Mutex<HashMap<String, u128>>>,
    confirm_in_flight: Arc<AtomicU64>,
    rpc_slots: Arc<tokio::sync::Semaphore>,
    coordination: coordination::CoordinationState,
    solana_features: solana_features::SolanaFeatureState,
    /// Keyless, off-by-default blockchain feature suite (wallets, executor,
    /// relayer, multisig, indexing, MEV monitoring, NFT storage, staking, bridge).
    blockchain: blockchain::BlockchainState,
}

/// RAII slot for one in-flight confirmation poller. Decrements the service-wide
/// counter on drop so a panicking or early-returning task can't leak a slot.
struct ConfirmSlot(Arc<AtomicU64>);

impl ConfirmSlot {
    /// Reserves a slot if the in-flight count is under the cap, else `None`.
    fn try_acquire(counter: &Arc<AtomicU64>) -> Option<Self> {
        let prior = counter.fetch_add(1, Ordering::AcqRel);
        if prior >= MAX_CONFIRM_POLLERS_IN_FLIGHT {
            counter.fetch_sub(1, Ordering::AcqRel);
            None
        } else {
            Some(ConfirmSlot(counter.clone()))
        }
    }
}

impl Drop for ConfirmSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl AppState {
    /// Records a settlement/resolution request id for at-most-once broadcast.
    /// Returns `false` when the id was already seen within the TTL window (a
    /// replay), so callers can skip a duplicate on-chain broadcast.
    fn claim_idempotency_key(&self, key: &str) -> bool {
        let now = now_ms();
        let mut guard = match self.idempotency.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.retain(|_, recorded| now.saturating_sub(*recorded) < IDEMPOTENCY_TTL_MS);
        if guard.contains_key(key) {
            return false;
        }
        if guard.len() >= MAX_IDEMPOTENCY_ENTRIES {
            // Bounded memory: drop the oldest entry before inserting a new one.
            if let Some(oldest) = guard
                .iter()
                .min_by_key(|(_, recorded)| **recorded)
                .map(|(stored_key, _)| stored_key.clone())
            {
                guard.remove(&oldest);
            }
        }
        guard.insert(key.to_string(), now);
        true
    }

    /// Releases a previously claimed idempotency key so a legitimately failed
    /// broadcast can be retried with the same request id. Safe because Solana
    /// dedupes resubmissions of the same signed transaction by signature.
    fn release_idempotency_key(&self, key: &str) {
        let mut guard = match self.idempotency.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.remove(key);
    }
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
    settlements_total: AtomicU64,
    settlement_errors_total: AtomicU64,
    resolutions_total: AtomicU64,
    resolution_errors_total: AtomicU64,
    settlement_idempotent_hits_total: AtomicU64,
    confirmations_confirmed_total: AtomicU64,
    confirmations_finalized_total: AtomicU64,
    confirmations_failed_total: AtomicU64,
    confirmations_pending_total: AtomicU64,
    confirmations_deferred_total: AtomicU64,
    rpc_get_health_requests_total: AtomicU64,
    rpc_get_health_errors_total: AtomicU64,
    rpc_get_version_requests_total: AtomicU64,
    rpc_get_version_errors_total: AtomicU64,
    rpc_simulate_transaction_requests_total: AtomicU64,
    rpc_simulate_transaction_errors_total: AtomicU64,
    rpc_send_transaction_requests_total: AtomicU64,
    rpc_send_transaction_errors_total: AtomicU64,
    rpc_get_latest_blockhash_requests_total: AtomicU64,
    rpc_get_latest_blockhash_errors_total: AtomicU64,
    rpc_get_signature_statuses_requests_total: AtomicU64,
    rpc_get_signature_statuses_errors_total: AtomicU64,
    rpc_get_transaction_requests_total: AtomicU64,
    rpc_get_transaction_errors_total: AtomicU64,
    rpc_get_account_info_requests_total: AtomicU64,
    rpc_get_account_info_errors_total: AtomicU64,
    rpc_get_balance_requests_total: AtomicU64,
    rpc_get_balance_errors_total: AtomicU64,
    rpc_get_token_account_balance_requests_total: AtomicU64,
    rpc_get_token_account_balance_errors_total: AtomicU64,
    rpc_get_fee_for_message_requests_total: AtomicU64,
    rpc_get_fee_for_message_errors_total: AtomicU64,
    rpc_get_minimum_balance_for_rent_exemption_requests_total: AtomicU64,
    rpc_get_minimum_balance_for_rent_exemption_errors_total: AtomicU64,
    rpc_get_signatures_for_address_requests_total: AtomicU64,
    rpc_get_signatures_for_address_errors_total: AtomicU64,
    rpc_get_recent_prioritization_fees_requests_total: AtomicU64,
    rpc_get_recent_prioritization_fees_errors_total: AtomicU64,
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
        Ok(line) => tracing::info!("{line}"),
        Err(error) => tracing::info!(
            "{{\"schema\":\"{LOG_SCHEMA}\",\"severity_text\":\"ERROR\",\"body\":\"structured log serialization failed\",\"resource_service_name\":\"{SERVICE_NAME}\",\"event_name\":\"structured-log-serialize-failed\",\"attributes\":{{\"error\":\"{error}\"}}}}"
        ),
    }
}

fn write_structured_log_to_stderr(severity: &str, event_name: &str, body: &str, attributes: Value) {
    let record = structured_log_record(severity, event_name, body, attributes);
    match serde_json::to_string(&record) {
        Ok(line) => tracing::error!("{line}"),
        Err(error) => tracing::error!(
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
        "getLatestBlockhash" => (
            &metrics.rpc_get_latest_blockhash_requests_total,
            &metrics.rpc_get_latest_blockhash_errors_total,
        ),
        "getSignatureStatuses" => (
            &metrics.rpc_get_signature_statuses_requests_total,
            &metrics.rpc_get_signature_statuses_errors_total,
        ),
        "getTransaction" => (
            &metrics.rpc_get_transaction_requests_total,
            &metrics.rpc_get_transaction_errors_total,
        ),
        "getAccountInfo" => (
            &metrics.rpc_get_account_info_requests_total,
            &metrics.rpc_get_account_info_errors_total,
        ),
        "getBalance" => (
            &metrics.rpc_get_balance_requests_total,
            &metrics.rpc_get_balance_errors_total,
        ),
        "getTokenAccountBalance" => (
            &metrics.rpc_get_token_account_balance_requests_total,
            &metrics.rpc_get_token_account_balance_errors_total,
        ),
        "getFeeForMessage" => (
            &metrics.rpc_get_fee_for_message_requests_total,
            &metrics.rpc_get_fee_for_message_errors_total,
        ),
        "getMinimumBalanceForRentExemption" => (
            &metrics.rpc_get_minimum_balance_for_rent_exemption_requests_total,
            &metrics.rpc_get_minimum_balance_for_rent_exemption_errors_total,
        ),
        "getSignaturesForAddress" => (
            &metrics.rpc_get_signatures_for_address_requests_total,
            &metrics.rpc_get_signatures_for_address_errors_total,
        ),
        "getRecentPrioritizationFees" => (
            &metrics.rpc_get_recent_prioritization_fees_requests_total,
            &metrics.rpc_get_recent_prioritization_fees_errors_total,
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

fn authorize_settlement(
    headers: &HeaderMap,
    state: &AppState,
) -> Result<(), (StatusCode, &'static str)> {
    let Some(secret) = &state.settlement_auth_secret else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "settlement/resolution is not configured with CONTRACT_SETTLEMENT_AUTH_SECRET",
        ));
    };
    let Some(value) = headers
        .get(SETTLEMENT_AUTH_HEADER)
        .and_then(|value| value.to_str().ok())
    else {
        return Err((
            StatusCode::UNAUTHORIZED,
            "missing x-contract-settlement-auth header",
        ));
    };
    if !sensitive_eq(value.trim(), secret) {
        return Err((
            StatusCode::UNAUTHORIZED,
            "invalid x-contract-settlement-auth header",
        ));
    }
    Ok(())
}

/// Validates a base58 transaction signature (64-byte ed25519 sig).
fn validate_signature(value: &str, label: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{label} must not be empty"));
    }
    if trimmed.len() > MAX_SIGNATURE_LEN {
        return Err(format!(
            "{label} must be at most {MAX_SIGNATURE_LEN} characters"
        ));
    }
    let decoded = bs58::decode(trimmed)
        .into_vec()
        .map_err(|error| format!("{label} must be valid base58: {error}"))?;
    if decoded.len() != 64 {
        return Err(format!(
            "{label} must decode to a 64 byte signature, got {} bytes",
            decoded.len()
        ));
    }
    Ok(trimmed.to_string())
}

fn config_error(message: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, message.into())
}

/// NATS-initiated settlement messages carry no auth header, and the NATS bus has
/// no per-subject authorization, so enabling NATS-triggered broadcast lets any
/// publisher to the settle/resolve subjects trigger an on-chain send. Require an
/// explicit acknowledgment so this cannot be turned on by flipping a single
/// boolean; lock down NATS (authz / NetworkPolicy) before setting the ack.
fn enforce_nats_broadcast_ack(
    nats_settlement_enabled: bool,
    ack_unauthenticated_bus: bool,
) -> Result<(), String> {
    if nats_settlement_enabled && !ack_unauthenticated_bus {
        return Err(
            "CONTRACT_NATS_SETTLEMENT_ENABLED=true requires CONTRACT_NATS_SETTLEMENT_ACK_UNAUTHENTICATED_BUS=true because the NATS bus has no per-subject auth: any publisher to the settle/resolve subjects could trigger an on-chain broadcast"
                .to_string(),
        );
    }
    Ok(())
}

/// Second gate for mainnet-beta: any capability that can broadcast a transaction
/// on-chain (`/send`, `/settle`, `/resolve`, or NATS-initiated settlement) must
/// not be enabled against mainnet without an explicit
/// `SOLANA_MAINNET_SETTLEMENT_ENABLED=true`. Mirrors the dd-escrow-rs mainnet
/// gate so a single misconfigured flag cannot move real funds.
fn enforce_mainnet_settlement_gate(
    cluster: &str,
    send_enabled: bool,
    settlement_enabled: bool,
    resolution_enabled: bool,
    mainnet_settlement_enabled: bool,
) -> Result<(), String> {
    let broadcast_capable = send_enabled || settlement_enabled || resolution_enabled;
    if cluster == "mainnet-beta" && broadcast_capable && !mainnet_settlement_enabled {
        return Err(
            "mainnet broadcast (SOLANA_SEND_ENABLED/SOLANA_SETTLEMENT_ENABLED/SOLANA_RESOLUTION_ENABLED) requires SOLANA_MAINNET_SETTLEMENT_ENABLED=true"
                .to_string(),
        );
    }
    Ok(())
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

// ---------------------------------------------------------------------------
// Read-only Solana RPC surface
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BlockhashQuery {
    cluster: Option<String>,
    commitment: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountInfoRequest {
    request_id: Option<String>,
    cluster: Option<String>,
    pubkey: String,
    encoding: Option<String>,
    commitment: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BalanceRequest {
    request_id: Option<String>,
    cluster: Option<String>,
    pubkey: String,
    /// "sol" (default) reads the lamport balance; "token" reads an SPL token
    /// account balance via getTokenAccountBalance.
    kind: Option<String>,
    commitment: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeeForMessageRequest {
    request_id: Option<String>,
    cluster: Option<String>,
    /// Base64-encoded compiled message (not a full signed transaction).
    message: String,
    commitment: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RentExemptionQuery {
    cluster: Option<String>,
    bytes: u64,
    commitment: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TransactionLookupRequest {
    request_id: Option<String>,
    cluster: Option<String>,
    signature: String,
    commitment: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfirmRequest {
    request_id: Option<String>,
    cluster: Option<String>,
    signatures: Vec<String>,
    target_commitment: Option<String>,
    timeout_ms: Option<u64>,
    poll_interval_ms: Option<u64>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ConfirmOutcome {
    signature: String,
    status: &'static str,
    target_commitment: String,
    reached: bool,
    polls: u32,
    elapsed_ms: u128,
    slot: Option<u64>,
    confirmation_status: Option<String>,
    error: Option<Value>,
}

/// Validates a confirmation commitment target. Only `confirmed` and
/// `finalized` are valid landing targets; `processed` is not durable.
fn normalize_confirm_commitment(input: Option<&str>) -> Result<String, String> {
    let value = input
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("confirmed");
    match value.to_ascii_lowercase().as_str() {
        "confirmed" => Ok("confirmed".to_string()),
        "finalized" => Ok("finalized".to_string()),
        _ => Err(format!(
            "targetCommitment must be confirmed or finalized: {value}"
        )),
    }
}

fn commitment_rank(status: &str) -> u8 {
    match status {
        "processed" => 1,
        "confirmed" => 2,
        "finalized" => 3,
        _ => 0,
    }
}

fn record_confirm_outcome(metrics: &Metrics, status: &str) {
    let counter = match status {
        "confirmed" => &metrics.confirmations_confirmed_total,
        "finalized" => &metrics.confirmations_finalized_total,
        "failed" => &metrics.confirmations_failed_total,
        _ => &metrics.confirmations_pending_total,
    };
    counter.fetch_add(1, Ordering::Relaxed);
}

/// Polls `getSignatureStatuses` until the signature reaches the target
/// commitment, fails on-chain, or the bounded timeout elapses.
async fn confirm_signature(
    state: &AppState,
    signature: &str,
    target_commitment: &str,
    timeout_ms: u64,
    poll_interval_ms: u64,
) -> ConfirmOutcome {
    let interval = poll_interval_ms
        .clamp(MIN_CONFIRM_POLL_INTERVAL_MS, MAX_CONFIRM_POLL_INTERVAL_MS)
        .max(1);
    let timeout = timeout_ms.clamp(interval, MAX_CONFIRM_TIMEOUT_MS);
    let max_polls = ((timeout / interval) as u32 + 1).min(MAX_CONFIRM_POLLS);
    let target_rank = commitment_rank(target_commitment);
    let started = Instant::now();

    let mut polls = 0u32;
    let mut last_confirmation_status: Option<String> = None;
    let mut last_slot: Option<u64> = None;

    while polls < max_polls {
        polls += 1;
        let params = json!([[signature], { "searchTransactionHistory": true }]);
        match solana_rpc(state, "getSignatureStatuses", params).await {
            Ok(result) => {
                let entry = result.pointer("/value/0").cloned().unwrap_or(Value::Null);
                if entry.is_object() {
                    last_slot = entry.get("slot").and_then(Value::as_u64).or(last_slot);
                    if let Some(error) = entry.get("err") {
                        if !error.is_null() {
                            let outcome = ConfirmOutcome {
                                signature: signature.to_string(),
                                status: "failed",
                                target_commitment: target_commitment.to_string(),
                                reached: false,
                                polls,
                                elapsed_ms: started.elapsed().as_millis(),
                                slot: last_slot,
                                confirmation_status: entry
                                    .get("confirmationStatus")
                                    .and_then(Value::as_str)
                                    .map(str::to_string),
                                error: Some(error.clone()),
                            };
                            record_confirm_outcome(&state.metrics, "failed");
                            return outcome;
                        }
                    }
                    let confirmation_status = entry
                        .get("confirmationStatus")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    if let Some(status) = &confirmation_status {
                        last_confirmation_status = Some(status.clone());
                        if commitment_rank(status) >= target_rank {
                            let status_label: &'static str = if target_commitment == "finalized" {
                                "finalized"
                            } else {
                                "confirmed"
                            };
                            record_confirm_outcome(&state.metrics, status_label);
                            return ConfirmOutcome {
                                signature: signature.to_string(),
                                status: status_label,
                                target_commitment: target_commitment.to_string(),
                                reached: true,
                                polls,
                                elapsed_ms: started.elapsed().as_millis(),
                                slot: last_slot,
                                confirmation_status,
                                error: None,
                            };
                        }
                    }
                }
            }
            Err(_) => {
                // Transient RPC error is already counted/logged in solana_rpc;
                // keep polling until the bounded budget is exhausted.
            }
        }
        if polls < max_polls {
            tokio::time::sleep(Duration::from_millis(interval)).await;
        }
    }

    record_confirm_outcome(&state.metrics, "pending");
    ConfirmOutcome {
        signature: signature.to_string(),
        status: "pending",
        target_commitment: target_commitment.to_string(),
        reached: false,
        polls,
        elapsed_ms: started.elapsed().as_millis(),
        slot: last_slot,
        confirmation_status: last_confirmation_status,
        error: None,
    }
}

/// Synthetic outcome returned when the service-wide confirmation-poller cap is
/// reached. No RPC is performed; the caller can re-check via `/confirm`.
fn deferred_confirm_outcome(signature: &str, target_commitment: &str) -> ConfirmOutcome {
    ConfirmOutcome {
        signature: signature.to_string(),
        status: "deferred",
        target_commitment: target_commitment.to_string(),
        reached: false,
        polls: 0,
        elapsed_ms: 0,
        slot: None,
        confirmation_status: None,
        error: Some(json!(
            "confirmation deferred: service confirmation capacity reached; re-check via POST /confirm"
        )),
    }
}

/// Runs `confirm_signature` under a service-wide poller slot. When the cap is
/// reached it sheds gracefully (no RPC) rather than amplifying upstream load.
async fn bounded_confirm(
    state: &AppState,
    signature: &str,
    target_commitment: &str,
    timeout_ms: u64,
    poll_interval_ms: u64,
) -> ConfirmOutcome {
    match ConfirmSlot::try_acquire(&state.confirm_in_flight) {
        Some(_slot) => {
            confirm_signature(state, signature, target_commitment, timeout_ms, poll_interval_ms).await
        }
        None => {
            state
                .metrics
                .confirmations_deferred_total
                .fetch_add(1, Ordering::Relaxed);
            deferred_confirm_outcome(signature, target_commitment)
        }
    }
}

// ---------------------------------------------------------------------------
// Settlement and resolution vocabulary (shared with dd-escrow-rs)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum SettlementAction {
    Fund,
    Release,
    Refund,
    PartialRelease,
    SplitRelease,
    DisputeAward,
    Expire,
    Cancel,
}

impl SettlementAction {
    fn as_str(self) -> &'static str {
        match self {
            SettlementAction::Fund => "fund",
            SettlementAction::Release => "release",
            SettlementAction::Refund => "refund",
            SettlementAction::PartialRelease => "partial-release",
            SettlementAction::SplitRelease => "split-release",
            SettlementAction::DisputeAward => "dispute-award",
            SettlementAction::Expire => "expire",
            SettlementAction::Cancel => "cancel",
        }
    }
}

const SETTLEMENT_ACTIONS: [&str; 8] = [
    "fund",
    "release",
    "refund",
    "partial-release",
    "split-release",
    "dispute-award",
    "expire",
    "cancel",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum ResolutionDecision {
    ReleaseToPayee,
    RefundToPayer,
    Split,
    AwardToClaimant,
    Uphold,
    Overturn,
}

impl ResolutionDecision {
    fn as_str(self) -> &'static str {
        match self {
            ResolutionDecision::ReleaseToPayee => "release-to-payee",
            ResolutionDecision::RefundToPayer => "refund-to-payer",
            ResolutionDecision::Split => "split",
            ResolutionDecision::AwardToClaimant => "award-to-claimant",
            ResolutionDecision::Uphold => "uphold",
            ResolutionDecision::Overturn => "overturn",
        }
    }

    /// Settlement actions that may legitimately enact a given dispute decision.
    fn allowed_actions(self) -> &'static [SettlementAction] {
        match self {
            ResolutionDecision::ReleaseToPayee => {
                &[SettlementAction::Release, SettlementAction::PartialRelease]
            }
            ResolutionDecision::RefundToPayer => &[SettlementAction::Refund],
            ResolutionDecision::Split => &[SettlementAction::SplitRelease],
            ResolutionDecision::AwardToClaimant => &[SettlementAction::DisputeAward],
            ResolutionDecision::Uphold => &[
                SettlementAction::Release,
                SettlementAction::PartialRelease,
                SettlementAction::SplitRelease,
                SettlementAction::DisputeAward,
            ],
            ResolutionDecision::Overturn => &[
                SettlementAction::Refund,
                SettlementAction::SplitRelease,
                SettlementAction::DisputeAward,
            ],
        }
    }
}

const RESOLUTION_DECISIONS: [&str; 6] = [
    "release-to-payee",
    "refund-to-payer",
    "split",
    "award-to-claimant",
    "uphold",
    "overturn",
];

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ConfirmOptions {
    target_commitment: Option<String>,
    timeout_ms: Option<u64>,
    poll_interval_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SettlementRequest {
    schema_version: String,
    request_id: Option<String>,
    cluster: Option<String>,
    contract_id: Option<String>,
    escrow_id: Option<String>,
    action: SettlementAction,
    transaction: String,
    encoding: Option<String>,
    commitment: Option<String>,
    skip_preflight: Option<bool>,
    max_retries: Option<usize>,
    min_context_slot: Option<u64>,
    confirm: Option<ConfirmOptions>,
    intent_digest: Option<String>,
    memo: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResolutionRequest {
    schema_version: String,
    request_id: Option<String>,
    cluster: Option<String>,
    dispute_id: Option<String>,
    escrow_id: Option<String>,
    decision: ResolutionDecision,
    action: SettlementAction,
    arbiter: Option<String>,
    arbiter_required_signer: Option<bool>,
    transaction: String,
    encoding: Option<String>,
    commitment: Option<String>,
    skip_preflight: Option<bool>,
    max_retries: Option<usize>,
    min_context_slot: Option<u64>,
    confirm: Option<ConfirmOptions>,
    rationale: Option<String>,
}

/// Common fields needed to drive simulate/send for a settlement-style request.
struct SettlementCore {
    request_id: Option<String>,
    cluster: Option<String>,
    transaction: String,
    encoding: Option<String>,
    commitment: Option<String>,
    skip_preflight: Option<bool>,
    max_retries: Option<usize>,
    min_context_slot: Option<u64>,
}

impl SettlementCore {
    /// Builds a TransactionRpcRequest so settlement paths reuse the audited
    /// validate/simulate/send helpers. `for_simulate` flips on
    /// replaceRecentBlockhash so dry-runs don't need a fresh blockhash.
    fn tx_request(&self, for_simulate: bool) -> TransactionRpcRequest {
        TransactionRpcRequest {
            request_id: self.request_id.clone(),
            cluster: self.cluster.clone(),
            transaction: self.transaction.clone(),
            encoding: self.encoding.clone(),
            commitment: self.commitment.clone(),
            sig_verify: Some(false),
            replace_recent_blockhash: Some(for_simulate),
            skip_preflight: self.skip_preflight,
            max_retries: self.max_retries,
            min_context_slot: self.min_context_slot,
        }
    }
}

impl SettlementRequest {
    fn core(&self) -> SettlementCore {
        SettlementCore {
            request_id: self.request_id.clone(),
            cluster: self.cluster.clone(),
            transaction: self.transaction.clone(),
            encoding: self.encoding.clone(),
            commitment: self.commitment.clone(),
            skip_preflight: self.skip_preflight,
            max_retries: self.max_retries,
            min_context_slot: self.min_context_slot,
        }
    }
}

impl ResolutionRequest {
    fn core(&self) -> SettlementCore {
        SettlementCore {
            request_id: self.request_id.clone(),
            cluster: self.cluster.clone(),
            transaction: self.transaction.clone(),
            encoding: self.encoding.clone(),
            commitment: self.commitment.clone(),
            skip_preflight: self.skip_preflight,
            max_retries: self.max_retries,
            min_context_slot: self.min_context_slot,
        }
    }
}

fn resolve_confirm_target(options: &Option<ConfirmOptions>) -> Result<(String, u64, u64), String> {
    let (target, timeout_ms, poll_interval_ms) = match options {
        Some(options) => (
            normalize_confirm_commitment(options.target_commitment.as_deref())?,
            options.timeout_ms.unwrap_or(DEFAULT_CONFIRM_TIMEOUT_MS),
            options
                .poll_interval_ms
                .unwrap_or(DEFAULT_CONFIRM_POLL_INTERVAL_MS),
        ),
        None => (
            "confirmed".to_string(),
            DEFAULT_CONFIRM_TIMEOUT_MS,
            DEFAULT_CONFIRM_POLL_INTERVAL_MS,
        ),
    };
    Ok((target, timeout_ms, poll_interval_ms))
}

/// Shared validation for the settlement-style transaction core. Returns the
/// validated encoding plus decoded byte length, or a list of errors.
fn validate_settlement_core(
    core: &SettlementCore,
    default_cluster: &str,
) -> Result<(String, &'static str, usize), Vec<String>> {
    let mut errors = Vec::new();
    let tx = core.tx_request(false);
    let cluster = match normalize_request_cluster(core.cluster.as_deref(), default_cluster) {
        Ok(cluster) => cluster,
        Err(error) => {
            errors.push(error);
            default_cluster.to_string()
        }
    };
    if let Err(error) = normalize_commitment(core.commitment.as_deref()) {
        errors.push(error);
    }
    match validate_signed_transaction(&tx) {
        Ok((encoding, decoded_len)) => {
            if errors.is_empty() {
                Ok((cluster, encoding, decoded_len))
            } else {
                Err(errors)
            }
        }
        Err(error) => {
            errors.push(error);
            Err(errors)
        }
    }
}

fn signed_transaction_bytes_from_rpc_params(params: &Value) -> Result<Vec<u8>, String> {
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

async fn solana_rpc(state: &AppState, method: &str, params: Value) -> Result<Value, String> {
    record_rpc_request(&state.metrics, method);
    let _rpc_permit = state
        .rpc_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
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
        "settlementSchemaVersion": SETTLEMENT_SCHEMA_VERSION,
        "resolutionSchemaVersion": RESOLUTION_SCHEMA_VERSION,
        "cluster": state.default_cluster,
        "sendEnabled": state.send_enabled,
        "skipPreflightAllowed": state.allow_skip_preflight,
        "settlementEnabled": state.settlement_enabled,
        "resolutionEnabled": state.resolution_enabled,
        "natsSettlementEnabled": state.nats_settlement_enabled,
        "mainnetSettlementEnabled": state.mainnet_settlement_enabled,
        "routes": {
            "health": "/healthz",
            "readiness": "/readyz",
            "capabilities": "/capabilities",
            "metrics": "/metrics",
            "status": "/status",
            "schema": "/schema",
            "settlementSchema": "/schema/settlement",
            "resolutionSchema": "/schema/resolution",
            "example": "/example",
            "settlementExample": "/example/settlement",
            "validate": "POST /validate",
            "simulate": "POST /simulate",
            "send": "POST /send",
            "blockhash": "GET /blockhash",
            "account": "POST /account",
            "balance": "POST /balance",
            "fee": "POST /fee",
            "rentExemption": "GET /rent-exemption",
            "transaction": "POST /transaction",
            "confirm": "POST /confirm",
            "simulateSettlement": "POST /simulate-settlement",
            "settle": "POST /settle",
            "resolve": "POST /resolve",
            "inspectProgram": "POST /program/inspect",
            "verifyProgram": "POST /program/verify",
            "inspectEscrow": "POST /escrow/inspect",
            "signatureHistory": "POST /chain/signatures",
            "priorityFees": "POST /chain/priority-fees"
        },
        "nats": {
            "resultSubject": state.result_subject,
            "settlementResultSubject": state.settlement_result_subject,
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

async fn readyz(State(state): State<AppState>) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let (chain, coordination, formal_methods) = tokio::join!(
        solana_rpc(&state, "getHealth", json!([])),
        state.coordination.readiness(),
        state.solana_features.readiness(),
    );
    let ok = chain.is_ok() && coordination.is_ok() && formal_methods.is_ok();
    if !ok {
        log_warn(
            "contract-service-not-ready",
            "Contract service dependency readiness check failed.",
            json!({
                "solana": chain.as_ref().err(),
                "coordination": coordination.as_ref().err(),
                "formalMethods": formal_methods.as_ref().err(),
            }),
        );
    }
    json_response(
        if ok {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        json!({
            "ok": ok,
            "service": SERVICE_NAME,
            "dependencies": {
                "solanaRpc": chain.is_ok(),
                "postgresAndFiducia": coordination.is_ok(),
                "formalMethods": formal_methods.is_ok(),
            }
        }),
    )
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
            "sendEnabled": state.send_enabled,
            "settlementEnabled": state.settlement_enabled,
            "resolutionEnabled": state.resolution_enabled,
            "natsSettlementEnabled": state.nats_settlement_enabled,
            "mainnetSettlementEnabled": state.mainnet_settlement_enabled,
            "skipPreflightAllowed": state.allow_skip_preflight,
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

async fn settlement_schema_http(State(state): State<AppState>) -> impl IntoResponse {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(settlement_schema())
}

async fn resolution_schema_http(State(state): State<AppState>) -> impl IntoResponse {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(resolution_schema())
}

async fn settlement_example_http(State(state): State<AppState>) -> impl IntoResponse {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    Json(settlement_example())
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

/// Returns the caller-supplied request id only when it is explicitly set and
/// non-empty, so idempotency keys never collapse onto the default prefix.
fn explicit_request_id(input: Option<&String>) -> Option<String> {
    input
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[allow(clippy::result_large_err)]
fn enforce_cluster(
    state: &AppState,
    cluster: Option<&str>,
    metrics_prefix: &str,
) -> Result<String, Response> {
    normalize_request_cluster(cluster, &state.default_cluster).map_err(|error| {
        state
            .metrics
            .policy_rejections_total
            .fetch_add(1, Ordering::Relaxed);
        log_warn(
            "contract-read-policy-rejected",
            "Read RPC request was rejected by policy.",
            json!({ "reason": "cluster_mismatch", "scope": metrics_prefix }),
        );
        json_response(StatusCode::BAD_REQUEST, json!({ "ok": false, "error": error }))
    })
}

async fn read_rpc_response(
    state: &AppState,
    method: &str,
    params: Value,
    request_id: String,
    cluster: String,
) -> Response {
    match solana_rpc(state, method, params).await {
        Ok(result) => json_response(
            StatusCode::OK,
            json!({
                "ok": true,
                "requestId": request_id,
                "cluster": cluster,
                "rpcMethod": method,
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
                    "requestId": request_id,
                    "rpcMethod": method,
                    "error": error,
                    "generatedAtMs": now_ms()
                }),
            )
        }
    }
}

async fn blockhash_http(
    State(state): State<AppState>,
    Query(query): Query<BlockhashQuery>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let cluster = match enforce_cluster(&state, query.cluster.as_deref(), "blockhash") {
        Ok(cluster) => cluster,
        Err(response) => return response,
    };
    let commitment = match normalize_commitment_or_default(query.commitment.as_deref()) {
        Ok(commitment) => commitment,
        Err(error) => {
            return json_response(StatusCode::BAD_REQUEST, json!({ "ok": false, "error": error }))
        }
    };
    let params = json!([{ "commitment": commitment }]);
    read_rpc_response(
        &state,
        "getLatestBlockhash",
        params,
        "contract-blockhash".to_string(),
        cluster,
    )
    .await
}

async fn account_http(
    State(state): State<AppState>,
    Json(request): Json<AccountInfoRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let cluster = match enforce_cluster(&state, request.cluster.as_deref(), "account") {
        Ok(cluster) => cluster,
        Err(response) => return response,
    };
    if let Err(error) = validate_pubkey(&request.pubkey, "pubkey") {
        return json_response(StatusCode::BAD_REQUEST, json!({ "ok": false, "error": error }));
    }
    let encoding = match request.encoding.as_deref().map(str::trim) {
        Some("base64") | None => "base64",
        Some("base58") => "base58",
        Some("jsonParsed") => "jsonParsed",
        Some(other) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "ok": false, "error": format!("encoding must be base64, base58, or jsonParsed: {other}") }),
            )
        }
    };
    let commitment = match normalize_commitment_or_default(request.commitment.as_deref()) {
        Ok(commitment) => commitment,
        Err(error) => {
            return json_response(StatusCode::BAD_REQUEST, json!({ "ok": false, "error": error }))
        }
    };
    let params = json!([
        request.pubkey.trim(),
        { "encoding": encoding, "commitment": commitment }
    ]);
    read_rpc_response(
        &state,
        "getAccountInfo",
        params,
        request_id(request.request_id.as_ref(), "contract-account"),
        cluster,
    )
    .await
}

async fn balance_http(
    State(state): State<AppState>,
    Json(request): Json<BalanceRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let cluster = match enforce_cluster(&state, request.cluster.as_deref(), "balance") {
        Ok(cluster) => cluster,
        Err(response) => return response,
    };
    if let Err(error) = validate_pubkey(&request.pubkey, "pubkey") {
        return json_response(StatusCode::BAD_REQUEST, json!({ "ok": false, "error": error }));
    }
    let commitment = match normalize_commitment_or_default(request.commitment.as_deref()) {
        Ok(commitment) => commitment,
        Err(error) => {
            return json_response(StatusCode::BAD_REQUEST, json!({ "ok": false, "error": error }))
        }
    };
    let (method, params) = match request.kind.as_deref().map(str::trim).unwrap_or("sol") {
        "sol" => (
            "getBalance",
            json!([request.pubkey.trim(), { "commitment": commitment }]),
        ),
        "token" => (
            "getTokenAccountBalance",
            json!([request.pubkey.trim(), { "commitment": commitment }]),
        ),
        other => {
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "ok": false, "error": format!("kind must be sol or token: {other}") }),
            )
        }
    };
    read_rpc_response(
        &state,
        method,
        params,
        request_id(request.request_id.as_ref(), "contract-balance"),
        cluster,
    )
    .await
}

async fn fee_http(
    State(state): State<AppState>,
    Json(request): Json<FeeForMessageRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let cluster = match enforce_cluster(&state, request.cluster.as_deref(), "fee") {
        Ok(cluster) => cluster,
        Err(response) => return response,
    };
    let message = request.message.trim();
    if message.is_empty() {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "ok": false, "error": "message must not be empty" }),
        );
    }
    match general_purpose::STANDARD.decode(message) {
        Ok(bytes) if bytes.len() <= MAX_SIGNED_TRANSACTION_BYTES => {}
        Ok(_) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "ok": false, "error": "message exceeds maximum size" }),
            )
        }
        Err(error) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "ok": false, "error": format!("message must be valid base64: {error}") }),
            )
        }
    }
    let commitment = match normalize_commitment_or_default(request.commitment.as_deref()) {
        Ok(commitment) => commitment,
        Err(error) => {
            return json_response(StatusCode::BAD_REQUEST, json!({ "ok": false, "error": error }))
        }
    };
    let params = json!([message, { "commitment": commitment }]);
    read_rpc_response(
        &state,
        "getFeeForMessage",
        params,
        request_id(request.request_id.as_ref(), "contract-fee"),
        cluster,
    )
    .await
}

async fn rent_exemption_http(
    State(state): State<AppState>,
    Query(query): Query<RentExemptionQuery>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let cluster = match enforce_cluster(&state, query.cluster.as_deref(), "rent-exemption") {
        Ok(cluster) => cluster,
        Err(response) => return response,
    };
    if query.bytes > MAX_RENT_EXEMPTION_BYTES {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "ok": false, "error": format!("bytes must be at most {MAX_RENT_EXEMPTION_BYTES}") }),
        );
    }
    let commitment = match normalize_commitment_or_default(query.commitment.as_deref()) {
        Ok(commitment) => commitment,
        Err(error) => {
            return json_response(StatusCode::BAD_REQUEST, json!({ "ok": false, "error": error }))
        }
    };
    let params = json!([query.bytes, { "commitment": commitment }]);
    read_rpc_response(
        &state,
        "getMinimumBalanceForRentExemption",
        params,
        "contract-rent-exemption".to_string(),
        cluster,
    )
    .await
}

async fn transaction_http(
    State(state): State<AppState>,
    Json(request): Json<TransactionLookupRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let cluster = match enforce_cluster(&state, request.cluster.as_deref(), "transaction") {
        Ok(cluster) => cluster,
        Err(response) => return response,
    };
    let signature = match validate_signature(&request.signature, "signature") {
        Ok(signature) => signature,
        Err(error) => {
            return json_response(StatusCode::BAD_REQUEST, json!({ "ok": false, "error": error }))
        }
    };
    let commitment = match normalize_confirm_commitment(request.commitment.as_deref()) {
        Ok(commitment) => commitment,
        Err(error) => {
            return json_response(StatusCode::BAD_REQUEST, json!({ "ok": false, "error": error }))
        }
    };
    let params = json!([
        signature,
        { "commitment": commitment, "maxSupportedTransactionVersion": 0, "encoding": "json" }
    ]);
    read_rpc_response(
        &state,
        "getTransaction",
        params,
        request_id(request.request_id.as_ref(), "contract-transaction"),
        cluster,
    )
    .await
}

async fn confirm_http(
    State(state): State<AppState>,
    Json(request): Json<ConfirmRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let cluster = match enforce_cluster(&state, request.cluster.as_deref(), "confirm") {
        Ok(cluster) => cluster,
        Err(response) => return response,
    };
    if request.signatures.is_empty() {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "ok": false, "error": "signatures must contain at least one signature" }),
        );
    }
    if request.signatures.len() > MAX_CONFIRM_SIGNATURES {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "ok": false, "error": format!("signatures must contain at most {MAX_CONFIRM_SIGNATURES} signatures") }),
        );
    }
    let mut signatures = Vec::with_capacity(request.signatures.len());
    for (index, signature) in request.signatures.iter().enumerate() {
        match validate_signature(signature, &format!("signatures[{index}]")) {
            Ok(signature) => signatures.push(signature),
            Err(error) => {
                return json_response(
                    StatusCode::BAD_REQUEST,
                    json!({ "ok": false, "error": error }),
                )
            }
        }
    }
    let target = match normalize_confirm_commitment(request.target_commitment.as_deref()) {
        Ok(target) => target,
        Err(error) => {
            return json_response(StatusCode::BAD_REQUEST, json!({ "ok": false, "error": error }))
        }
    };
    let timeout_ms = request.timeout_ms.unwrap_or(DEFAULT_CONFIRM_TIMEOUT_MS);
    let poll_interval_ms = request
        .poll_interval_ms
        .unwrap_or(DEFAULT_CONFIRM_POLL_INTERVAL_MS);

    // Confirm the batch concurrently so wall-clock is bounded by a single
    // timeout window, not the sum across signatures.
    let outcomes = futures_util::future::join_all(
        signatures
            .iter()
            .map(|signature| bounded_confirm(&state, signature, &target, timeout_ms, poll_interval_ms)),
    )
    .await;
    let all_reached = outcomes.iter().all(|outcome| outcome.reached);
    json_response(
        StatusCode::OK,
        json!({
            "ok": all_reached,
            "requestId": request_id(request.request_id.as_ref(), "contract-confirm"),
            "cluster": cluster,
            "targetCommitment": target,
            "outcomes": outcomes,
            "generatedAtMs": now_ms()
        }),
    )
}

async fn simulate_settlement_http(
    State(state): State<AppState>,
    Json(request): Json<SettlementRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if request.schema_version != SETTLEMENT_SCHEMA_VERSION {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "ok": false, "error": format!("schemaVersion must be {SETTLEMENT_SCHEMA_VERSION}") }),
        );
    }
    let core = request.core();
    let (cluster, encoding, decoded_bytes) =
        match validate_settlement_core(&core, &state.default_cluster) {
            Ok(validated) => validated,
            Err(errors) => {
                state
                    .metrics
                    .policy_rejections_total
                    .fetch_add(1, Ordering::Relaxed);
                return json_response(
                    StatusCode::BAD_REQUEST,
                    json!({ "ok": false, "errors": errors }),
                );
            }
        };
    let tx = core.tx_request(true);
    let params = match simulate_params(&tx, encoding) {
        Ok(params) => params,
        Err(error) => {
            return json_response(StatusCode::BAD_REQUEST, json!({ "ok": false, "error": error }))
        }
    };
    match solana_rpc(&state, "simulateTransaction", params).await {
        Ok(result) => json_response(
            StatusCode::OK,
            json!({
                "ok": true,
                "requestId": request_id(request.request_id.as_ref(), "contract-settlement-simulate"),
                "schemaVersion": SETTLEMENT_SCHEMA_VERSION,
                "cluster": cluster,
                "action": request.action.as_str(),
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
                json!({ "ok": false, "error": error }),
            )
        }
    }
}

async fn settle_http(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(request): Json<SettlementRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .settlements_total
        .fetch_add(1, Ordering::Relaxed);

    let req_id = request_id(request.request_id.as_ref(), "contract-settlement");

    if !state.settlement_enabled {
        state
            .metrics
            .policy_rejections_total
            .fetch_add(1, Ordering::Relaxed);
        return json_response(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({
                "ok": false,
                "requestId": req_id,
                "error": "settlement is disabled; set SOLANA_SETTLEMENT_ENABLED=true to permit /settle",
                "generatedAtMs": now_ms()
            }),
        );
    }
    if let Err((status, error)) = authorize_settlement(&headers, &state) {
        state
            .metrics
            .send_auth_failures_total
            .fetch_add(1, Ordering::Relaxed);
        state
            .metrics
            .policy_rejections_total
            .fetch_add(1, Ordering::Relaxed);
        return json_response(
            status,
            json!({ "ok": false, "requestId": req_id, "error": error }),
        );
    }
    if request.schema_version != SETTLEMENT_SCHEMA_VERSION {
        state
            .metrics
            .settlement_errors_total
            .fetch_add(1, Ordering::Relaxed);
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "ok": false, "requestId": req_id, "error": format!("schemaVersion must be {SETTLEMENT_SCHEMA_VERSION}") }),
        );
    }
    let core = request.core();
    let (cluster, encoding, decoded_bytes) =
        match validate_settlement_core(&core, &state.default_cluster) {
            Ok(validated) => validated,
            Err(errors) => {
                state
                    .metrics
                    .settlement_errors_total
                    .fetch_add(1, Ordering::Relaxed);
                state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                return json_response(
                    StatusCode::BAD_REQUEST,
                    json!({ "ok": false, "requestId": req_id, "errors": errors }),
                );
            }
        };
    let (confirm_target, confirm_timeout, confirm_interval) =
        match resolve_confirm_target(&request.confirm) {
            Ok(values) => values,
            Err(error) => {
                return json_response(
                    StatusCode::BAD_REQUEST,
                    json!({ "ok": false, "requestId": req_id, "error": error }),
                )
            }
        };

    // Idempotency: only an explicitly provided request id guards a broadcast.
    let idem_key = explicit_request_id(request.request_id.as_ref()).map(|key| format!("settle:{key}"));
    if let Some(key) = &idem_key {
        if !state.claim_idempotency_key(key) {
            state
                .metrics
                .settlement_idempotent_hits_total
                .fetch_add(1, Ordering::Relaxed);
            return json_response(
                StatusCode::CONFLICT,
                json!({
                    "ok": false,
                    "requestId": req_id,
                    "error": "duplicate settlement requestId within the idempotency window; broadcast suppressed",
                    "idempotent": true,
                    "generatedAtMs": now_ms()
                }),
            );
        }
    }
    let release = |state: &AppState| {
        if let Some(key) = &idem_key {
            state.release_idempotency_key(key);
        }
    };

    let tx = core.tx_request(false);
    let send = match send_params(&tx, encoding, state.allow_skip_preflight) {
        Ok(params) => params,
        Err(error) => {
            release(&state);
            state
                .metrics
                .settlement_errors_total
                .fetch_add(1, Ordering::Relaxed);
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "ok": false, "requestId": req_id, "error": error }),
            );
        }
    };
    let signature_value = match solana_rpc(&state, "sendTransaction", send).await {
        Ok(value) => value,
        Err(error) => {
            release(&state);
            state
                .metrics
                .settlement_errors_total
                .fetch_add(1, Ordering::Relaxed);
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            publish_runtime_critical_event(
                &state,
                "contract-settlement-send-failed",
                "Settlement sendTransaction failed.",
                json!({ "requestId": req_id, "action": request.action.as_str(), "error": error }),
            )
            .await;
            return json_response(
                StatusCode::BAD_GATEWAY,
                json!({ "ok": false, "requestId": req_id, "error": error }),
            );
        }
    };
    let signature = signature_value.as_str().unwrap_or_default().to_string();
    if signature.is_empty() {
        release(&state);
        state
            .metrics
            .settlement_errors_total
            .fetch_add(1, Ordering::Relaxed);
        return json_response(
            StatusCode::BAD_GATEWAY,
            json!({ "ok": false, "requestId": req_id, "error": "sendTransaction did not return a signature" }),
        );
    }
    let confirmation = bounded_confirm(
        &state,
        &signature,
        &confirm_target,
        confirm_timeout,
        confirm_interval,
    )
    .await;

    let outcome = json!({
        "messageKind": "solana.settlement.outcome",
        "source": SERVICE_NAME,
        "ok": confirmation.reached,
        "requestId": req_id,
        "schemaVersion": SETTLEMENT_SCHEMA_VERSION,
        "cluster": cluster,
        "kind": "settlement",
        "action": request.action.as_str(),
        "contractId": request.contract_id,
        "escrowId": request.escrow_id,
        "intentDigest": request.intent_digest,
        "memo": request.memo,
        "encoding": encoding,
        "transactionBytes": decoded_bytes,
        "signature": signature,
        "confirmation": confirmation,
        "generatedAtMs": now_ms()
    });
    publish_settlement_outcome(&state, outcome.clone()).await;
    publish_contract_event(&state, "solana.contract.settlement", &req_id, confirmation.reached)
        .await;
    json_response(StatusCode::OK, outcome)
}

async fn resolve_http(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(request): Json<ResolutionRequest>,
) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .resolutions_total
        .fetch_add(1, Ordering::Relaxed);

    let req_id = request_id(request.request_id.as_ref(), "contract-resolution");

    if !state.resolution_enabled {
        state
            .metrics
            .policy_rejections_total
            .fetch_add(1, Ordering::Relaxed);
        return json_response(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({
                "ok": false,
                "requestId": req_id,
                "error": "resolution is disabled; set SOLANA_RESOLUTION_ENABLED=true to permit /resolve",
                "generatedAtMs": now_ms()
            }),
        );
    }
    if let Err((status, error)) = authorize_settlement(&headers, &state) {
        state
            .metrics
            .send_auth_failures_total
            .fetch_add(1, Ordering::Relaxed);
        state
            .metrics
            .policy_rejections_total
            .fetch_add(1, Ordering::Relaxed);
        return json_response(
            status,
            json!({ "ok": false, "requestId": req_id, "error": error }),
        );
    }

    let mut errors = Vec::new();
    if request.schema_version != RESOLUTION_SCHEMA_VERSION {
        errors.push(format!("schemaVersion must be {RESOLUTION_SCHEMA_VERSION}"));
    }
    if !request.decision.allowed_actions().contains(&request.action) {
        errors.push(format!(
            "decision {} does not permit settlement action {}",
            request.decision.as_str(),
            request.action.as_str()
        ));
    }
    if let Some(arbiter) = &request.arbiter {
        if let Err(error) = validate_pubkey(arbiter, "arbiter") {
            errors.push(error);
        }
    } else if request.arbiter_required_signer == Some(true) {
        errors.push("arbiter pubkey is required when arbiterRequiredSigner is true".to_string());
    }
    if let Some(rationale) = &request.rationale {
        if rationale.len() > MAX_RATIONALE_BYTES {
            errors.push(format!(
                "rationale must be at most {MAX_RATIONALE_BYTES} bytes"
            ));
        }
    }
    let core = request.core();
    let (cluster, encoding, decoded_bytes) =
        match validate_settlement_core(&core, &state.default_cluster) {
            Ok(validated) => validated,
            Err(core_errors) => {
                errors.extend(core_errors);
                (state.default_cluster.clone(), "base64", 0)
            }
        };
    let (confirm_target, confirm_timeout, confirm_interval) =
        match resolve_confirm_target(&request.confirm) {
            Ok(values) => values,
            Err(error) => {
                errors.push(error);
                ("confirmed".to_string(), DEFAULT_CONFIRM_TIMEOUT_MS, DEFAULT_CONFIRM_POLL_INTERVAL_MS)
            }
        };
    if !errors.is_empty() {
        state
            .metrics
            .resolution_errors_total
            .fetch_add(1, Ordering::Relaxed);
        state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "ok": false, "requestId": req_id, "errors": errors }),
        );
    }

    let idem_key =
        explicit_request_id(request.request_id.as_ref()).map(|key| format!("resolve:{key}"));
    if let Some(key) = &idem_key {
        if !state.claim_idempotency_key(key) {
            state
                .metrics
                .settlement_idempotent_hits_total
                .fetch_add(1, Ordering::Relaxed);
            return json_response(
                StatusCode::CONFLICT,
                json!({
                    "ok": false,
                    "requestId": req_id,
                    "error": "duplicate resolution requestId within the idempotency window; broadcast suppressed",
                    "idempotent": true,
                    "generatedAtMs": now_ms()
                }),
            );
        }
    }
    let release = |state: &AppState| {
        if let Some(key) = &idem_key {
            state.release_idempotency_key(key);
        }
    };

    let tx = core.tx_request(false);
    let send = match send_params(&tx, encoding, state.allow_skip_preflight) {
        Ok(params) => params,
        Err(error) => {
            release(&state);
            state
                .metrics
                .resolution_errors_total
                .fetch_add(1, Ordering::Relaxed);
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "ok": false, "requestId": req_id, "error": error }),
            );
        }
    };
    let signature_value = match solana_rpc(&state, "sendTransaction", send).await {
        Ok(value) => value,
        Err(error) => {
            release(&state);
            state
                .metrics
                .resolution_errors_total
                .fetch_add(1, Ordering::Relaxed);
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            publish_runtime_critical_event(
                &state,
                "contract-resolution-send-failed",
                "Resolution sendTransaction failed.",
                json!({ "requestId": req_id, "decision": request.decision.as_str(), "action": request.action.as_str(), "error": error }),
            )
            .await;
            return json_response(
                StatusCode::BAD_GATEWAY,
                json!({ "ok": false, "requestId": req_id, "error": error }),
            );
        }
    };
    let signature = signature_value.as_str().unwrap_or_default().to_string();
    if signature.is_empty() {
        release(&state);
        state
            .metrics
            .resolution_errors_total
            .fetch_add(1, Ordering::Relaxed);
        return json_response(
            StatusCode::BAD_GATEWAY,
            json!({ "ok": false, "requestId": req_id, "error": "sendTransaction did not return a signature" }),
        );
    }
    let confirmation = bounded_confirm(
        &state,
        &signature,
        &confirm_target,
        confirm_timeout,
        confirm_interval,
    )
    .await;

    let outcome = json!({
        "messageKind": "solana.resolution.outcome",
        "source": SERVICE_NAME,
        "ok": confirmation.reached,
        "requestId": req_id,
        "schemaVersion": RESOLUTION_SCHEMA_VERSION,
        "cluster": cluster,
        "kind": "resolution",
        "decision": request.decision.as_str(),
        "action": request.action.as_str(),
        "disputeId": request.dispute_id,
        "escrowId": request.escrow_id,
        "arbiter": request.arbiter,
        "encoding": encoding,
        "transactionBytes": decoded_bytes,
        "signature": signature,
        "confirmation": confirmation,
        "generatedAtMs": now_ms()
    });
    publish_settlement_outcome(&state, outcome.clone()).await;
    publish_contract_event(&state, "solana.contract.resolution", &req_id, confirmation.reached)
        .await;
    json_response(StatusCode::OK, outcome)
}

fn bool_label(value: bool) -> &'static str {
    if value {
        "true"
    } else {
        "false"
    }
}

/// RPC methods exposed as low-cardinality `rpc_method` label values in the
/// per-method counter families.
const METRICS_RPC_METHODS: [&str; 14] = [
    "getHealth",
    "getVersion",
    "simulateTransaction",
    "sendTransaction",
    "getLatestBlockhash",
    "getSignatureStatuses",
    "getTransaction",
    "getAccountInfo",
    "getBalance",
    "getTokenAccountBalance",
    "getFeeForMessage",
    "getMinimumBalanceForRentExemption",
    "getSignaturesForAddress",
    "getRecentPrioritizationFees",
];

/// Appends a single-line Prometheus counter family (HELP/TYPE + one sample).
fn push_counter(out: &mut String, name: &str, help: &str, value: u64) {
    out.push_str(&format!(
        "# HELP {name} {help}\n# TYPE {name} counter\n{name} {value}\n"
    ));
}

fn metrics_body(state: &AppState) -> String {
    let m = &state.metrics;
    let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
    let mut out = String::with_capacity(4096);

    push_counter(
        &mut out,
        "dd_contract_service_http_requests_total",
        "HTTP requests handled by the Solana contract service.",
        load(&m.http_requests_total),
    );
    push_counter(
        &mut out,
        "dd_contract_service_validations_total",
        "Contract validation requests handled.",
        load(&m.validations_total),
    );
    push_counter(
        &mut out,
        "dd_contract_service_validation_errors_total",
        "Contract validation requests rejected.",
        load(&m.validation_errors_total),
    );
    push_counter(
        &mut out,
        "dd_contract_service_policy_rejections_total",
        "Requests rejected by contract service safety policy before upstream RPC.",
        load(&m.policy_rejections_total),
    );
    push_counter(
        &mut out,
        "dd_contract_service_settlements_total",
        "Settlement requests handled by /settle and the settle NATS subject.",
        load(&m.settlements_total),
    );
    push_counter(
        &mut out,
        "dd_contract_service_settlement_errors_total",
        "Settlement requests that failed validation or broadcast.",
        load(&m.settlement_errors_total),
    );
    push_counter(
        &mut out,
        "dd_contract_service_resolutions_total",
        "Dispute resolution requests handled by /resolve and the resolve NATS subject.",
        load(&m.resolutions_total),
    );
    push_counter(
        &mut out,
        "dd_contract_service_resolution_errors_total",
        "Dispute resolution requests that failed validation or broadcast.",
        load(&m.resolution_errors_total),
    );
    push_counter(
        &mut out,
        "dd_contract_service_settlement_idempotent_hits_total",
        "Settlement/resolution broadcasts suppressed by the idempotency guard.",
        load(&m.settlement_idempotent_hits_total),
    );
    push_counter(
        &mut out,
        "dd_contract_service_rpc_requests_total",
        "Solana JSON-RPC requests sent.",
        load(&m.rpc_requests_total),
    );
    push_counter(
        &mut out,
        "dd_contract_service_rpc_errors_total",
        "Solana JSON-RPC requests that failed.",
        load(&m.rpc_errors_total),
    );

    // Per-method request/error families (stable label set).
    out.push_str("# HELP dd_contract_service_rpc_requests_by_method_total Solana JSON-RPC requests sent by low-cardinality method.\n# TYPE dd_contract_service_rpc_requests_by_method_total counter\n");
    for method in METRICS_RPC_METHODS {
        let (requests, _) = rpc_method_counters(m, method);
        out.push_str(&format!(
            "dd_contract_service_rpc_requests_by_method_total{{rpc_method=\"{method}\"}} {}\n",
            load(requests)
        ));
    }
    out.push_str("# HELP dd_contract_service_rpc_errors_by_method_total Solana JSON-RPC failures by low-cardinality method.\n# TYPE dd_contract_service_rpc_errors_by_method_total counter\n");
    for method in METRICS_RPC_METHODS {
        let (_, errors) = rpc_method_counters(m, method);
        out.push_str(&format!(
            "dd_contract_service_rpc_errors_by_method_total{{rpc_method=\"{method}\"}} {}\n",
            load(errors)
        ));
    }

    // Confirmation outcomes by terminal status.
    out.push_str("# HELP dd_contract_service_confirmations_total Settlement/resolution signature confirmation outcomes by terminal status.\n# TYPE dd_contract_service_confirmations_total counter\n");
    for (outcome, value) in [
        ("confirmed", load(&m.confirmations_confirmed_total)),
        ("finalized", load(&m.confirmations_finalized_total)),
        ("failed", load(&m.confirmations_failed_total)),
        ("pending", load(&m.confirmations_pending_total)),
        ("deferred", load(&m.confirmations_deferred_total)),
    ] {
        out.push_str(&format!(
            "dd_contract_service_confirmations_total{{outcome=\"{outcome}\"}} {value}\n"
        ));
    }

    push_counter(
        &mut out,
        "dd_contract_service_nats_messages_total",
        "NATS messages received across subscribed subjects.",
        load(&m.nats_messages_total),
    );
    push_counter(
        &mut out,
        "dd_contract_service_nats_payload_rejected_total",
        "NATS messages rejected before processing.",
        load(&m.nats_payload_rejected_total),
    );
    out.push_str("# HELP dd_contract_service_nats_published_total NATS messages published by subject kind.\n# TYPE dd_contract_service_nats_published_total counter\n");
    out.push_str(&format!(
        "dd_contract_service_nats_published_total{{subject_kind=\"result\"}} {}\n",
        load(&m.nats_results_published_total)
    ));
    out.push_str(&format!(
        "dd_contract_service_nats_published_total{{subject_kind=\"event\"}} {}\n",
        load(&m.nats_events_published_total)
    ));
    out.push_str(&format!(
        "dd_contract_service_nats_published_total{{subject_kind=\"critical\"}} {}\n",
        load(&m.nats_critical_events_published_total)
    ));
    push_counter(
        &mut out,
        "dd_contract_service_nats_publish_errors_total",
        "NATS publish failures observed.",
        load(&m.nats_publish_errors_total),
    );
    push_counter(
        &mut out,
        "dd_contract_service_send_blocked_total",
        "Raw transaction sends blocked by policy.",
        load(&m.send_blocked_total),
    );
    push_counter(
        &mut out,
        "dd_contract_service_send_auth_failures_total",
        "Send/settlement attempts rejected by an auth header check.",
        load(&m.send_auth_failures_total),
    );
    push_counter(
        &mut out,
        "dd_contract_service_errors_total",
        "Contract service errors observed.",
        load(&m.errors_total),
    );
    // Blockchain feature-suite counters share the same exposition.
    state.blockchain.render_metrics(&mut out);
    state.coordination.render_metrics(&mut out);
    state.solana_features.render_metrics(&mut out);

    format!(
        "# HELP dd_contract_service_info Static service configuration labels for the Solana contract service.\n\
# TYPE dd_contract_service_info gauge\n\
dd_contract_service_info{{cluster=\"{}\",send_enabled=\"{}\",skip_preflight_allowed=\"{}\",settlement_enabled=\"{}\",resolution_enabled=\"{}\",mainnet_settlement_enabled=\"{}\",coordination_enabled=\"{}\",formal_methods_enabled=\"{}\"}} 1\n{out}",
        state.default_cluster,
        bool_label(state.send_enabled),
        bool_label(state.allow_skip_preflight),
        bool_label(state.settlement_enabled),
        bool_label(state.resolution_enabled),
        bool_label(state.mainnet_settlement_enabled),
        bool_label(state.coordination.enabled()),
        bool_label(state.solana_features.formal_enabled()),
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

fn settlement_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "dd-contract-service Solana settlement request",
        "type": "object",
        "required": ["schemaVersion", "action", "transaction"],
        "properties": {
            "schemaVersion": { "const": SETTLEMENT_SCHEMA_VERSION },
            "requestId": { "type": "string", "maxLength": MAX_REQUEST_ID_LEN, "description": "Explicit ids guard at-most-once broadcast within the idempotency window." },
            "cluster": { "enum": ["mainnet-beta", "devnet", "testnet", "localnet", "custom"] },
            "contractId": { "type": "string" },
            "escrowId": { "type": "string" },
            "action": { "enum": SETTLEMENT_ACTIONS },
            "transaction": { "type": "string", "description": "Signed transaction, base64 (default) or base58" },
            "encoding": { "enum": ["base64", "base58"] },
            "commitment": { "enum": ["processed", "confirmed", "finalized"] },
            "skipPreflight": { "type": "boolean" },
            "maxRetries": { "type": "integer", "minimum": 0, "maximum": MAX_SEND_RETRIES },
            "minContextSlot": { "type": "integer", "minimum": 0 },
            "intentDigest": { "type": "string" },
            "memo": { "type": "string", "maxLength": MAX_MEMO_BYTES },
            "confirm": {
                "type": "object",
                "properties": {
                    "targetCommitment": { "enum": ["confirmed", "finalized"] },
                    "timeoutMs": { "type": "integer", "minimum": 0, "maximum": MAX_CONFIRM_TIMEOUT_MS },
                    "pollIntervalMs": { "type": "integer", "minimum": MIN_CONFIRM_POLL_INTERVAL_MS }
                }
            }
        }
    })
}

fn resolution_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "dd-contract-service Solana dispute resolution request",
        "type": "object",
        "required": ["schemaVersion", "decision", "action", "transaction"],
        "properties": {
            "schemaVersion": { "const": RESOLUTION_SCHEMA_VERSION },
            "requestId": { "type": "string", "maxLength": MAX_REQUEST_ID_LEN },
            "cluster": { "enum": ["mainnet-beta", "devnet", "testnet", "localnet", "custom"] },
            "disputeId": { "type": "string" },
            "escrowId": { "type": "string" },
            "decision": { "enum": RESOLUTION_DECISIONS, "description": "Dispute outcome; constrains which settlement action may enact it." },
            "action": { "enum": SETTLEMENT_ACTIONS },
            "arbiter": { "type": "string", "description": "Base58 arbiter public key" },
            "arbiterRequiredSigner": { "type": "boolean" },
            "transaction": { "type": "string" },
            "encoding": { "enum": ["base64", "base58"] },
            "commitment": { "enum": ["processed", "confirmed", "finalized"] },
            "skipPreflight": { "type": "boolean" },
            "maxRetries": { "type": "integer", "minimum": 0, "maximum": MAX_SEND_RETRIES },
            "minContextSlot": { "type": "integer", "minimum": 0 },
            "rationale": { "type": "string", "maxLength": MAX_RATIONALE_BYTES },
            "confirm": {
                "type": "object",
                "properties": {
                    "targetCommitment": { "enum": ["confirmed", "finalized"] },
                    "timeoutMs": { "type": "integer", "minimum": 0, "maximum": MAX_CONFIRM_TIMEOUT_MS },
                    "pollIntervalMs": { "type": "integer", "minimum": MIN_CONFIRM_POLL_INTERVAL_MS }
                }
            }
        }
    })
}

fn settlement_example() -> Value {
    json!({
        "schemaVersion": SETTLEMENT_SCHEMA_VERSION,
        "requestId": "settlement-demo",
        "cluster": "devnet",
        "escrowId": "escrow-demo",
        "action": "release",
        "transaction": "<base64-encoded signed settlement transaction>",
        "encoding": "base64",
        "commitment": "confirmed",
        "intentDigest": "solana:0011223344556677",
        "confirm": { "targetCommitment": "finalized", "timeoutMs": 30000 }
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
            settlement_enabled: true,
            resolution_enabled: true,
            nats_settlement_enabled: false,
            mainnet_settlement_enabled: false,
            settlement_auth_secret: Some("settlement-secret".to_string()),
            nats: None,
            result_subject: "results".to_string(),
            settlement_result_subject: "settlement.results".to_string(),
            event_subject: "events".to_string(),
            critical_event_subject: "events.critical".to_string(),
            metrics: Arc::new(Metrics::default()),
            idempotency: Arc::new(Mutex::new(HashMap::new())),
            confirm_in_flight: Arc::new(AtomicU64::new(0)),
            rpc_slots: Arc::new(tokio::sync::Semaphore::new(MAX_RPC_IN_FLIGHT)),
            coordination: coordination::CoordinationState::disabled_for_tests(),
            solana_features: solana_features::SolanaFeatureState::disabled_for_tests(),
            blockchain: blockchain::BlockchainState::from_env(
                reqwest::Client::new(),
                "https://api.devnet.solana.com",
                "devnet",
            )
            .expect("blockchain state defaults are valid"),
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
    fn broadcast_coordination_uses_canonical_transaction_bytes() {
        let bytes = [1_u8, 2, 3, 4];
        let from_base64 = signed_transaction_bytes_from_rpc_params(&json!([
            general_purpose::STANDARD.encode(bytes),
            { "encoding": "base64" }
        ]))
        .expect("base64 transaction");
        let from_base58 = signed_transaction_bytes_from_rpc_params(&json!([
            bs58::encode(bytes).into_string(),
            { "encoding": "base58" }
        ]))
        .expect("base58 transaction");

        assert_eq!(from_base64, bytes);
        assert_eq!(from_base58, bytes);
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

    #[test]
    fn metrics_body_includes_new_rpc_methods_and_settlement_counters() {
        let state = sample_state();
        record_rpc_request(&state.metrics, "getSignatureStatuses");
        record_rpc_request(&state.metrics, "getLatestBlockhash");
        record_rpc_request(&state.metrics, "getSignaturesForAddress");
        record_rpc_request(&state.metrics, "getRecentPrioritizationFees");
        record_confirm_outcome(&state.metrics, "finalized");

        let body = metrics_body(&state);

        assert!(body.contains(
            "dd_contract_service_rpc_requests_by_method_total{rpc_method=\"getSignatureStatuses\"} 1"
        ));
        assert!(body.contains(
            "dd_contract_service_rpc_requests_by_method_total{rpc_method=\"getLatestBlockhash\"} 1"
        ));
        assert!(body.contains(
            "dd_contract_service_rpc_requests_by_method_total{rpc_method=\"getSignaturesForAddress\"} 1"
        ));
        assert!(body.contains(
            "dd_contract_service_rpc_requests_by_method_total{rpc_method=\"getRecentPrioritizationFees\"} 1"
        ));
        assert!(body.contains("dd_contract_service_confirmations_total{outcome=\"finalized\"} 1"));
        assert!(body.contains("dd_contract_service_settlements_total 0"));
        assert!(body.contains("settlement_enabled=\"true\""));
    }

    #[test]
    fn settlement_auth_requires_matching_header() {
        let state = sample_state();
        let mut headers = HeaderMap::new();

        assert!(authorize_settlement(&headers, &state).is_err());
        headers.insert(SETTLEMENT_AUTH_HEADER, "settlement-secret".parse().unwrap());
        assert!(authorize_settlement(&headers, &state).is_ok());
        headers.insert(SETTLEMENT_AUTH_HEADER, "nope".parse().unwrap());
        assert!(authorize_settlement(&headers, &state).is_err());
    }

    #[test]
    fn mainnet_gate_blocks_broadcast_without_explicit_flag() {
        // Devnet never requires the gate.
        assert!(enforce_mainnet_settlement_gate("devnet", true, true, true, false).is_ok());
        // Mainnet with any broadcast capability and no gate is refused.
        assert!(enforce_mainnet_settlement_gate("mainnet-beta", true, false, false, false).is_err());
        assert!(enforce_mainnet_settlement_gate("mainnet-beta", false, true, false, false).is_err());
        assert!(enforce_mainnet_settlement_gate("mainnet-beta", false, false, true, false).is_err());
        // Mainnet with the explicit gate is allowed.
        assert!(enforce_mainnet_settlement_gate("mainnet-beta", true, true, true, true).is_ok());
        // Mainnet with nothing broadcast-capable needs no gate.
        assert!(enforce_mainnet_settlement_gate("mainnet-beta", false, false, false, false).is_ok());
    }

    #[test]
    fn nats_broadcast_requires_explicit_unauthenticated_bus_ack() {
        // Off: no ack needed.
        assert!(enforce_nats_broadcast_ack(false, false).is_ok());
        // Enabling NATS broadcast without acknowledging the unauthenticated bus is refused.
        assert!(enforce_nats_broadcast_ack(true, false).is_err());
        // With the explicit acknowledgment it is allowed.
        assert!(enforce_nats_broadcast_ack(true, true).is_ok());
    }

    #[test]
    fn resolution_decision_constrains_actions() {
        assert!(ResolutionDecision::RefundToPayer
            .allowed_actions()
            .contains(&SettlementAction::Refund));
        assert!(!ResolutionDecision::RefundToPayer
            .allowed_actions()
            .contains(&SettlementAction::Release));
        assert!(ResolutionDecision::AwardToClaimant
            .allowed_actions()
            .contains(&SettlementAction::DisputeAward));
    }

    #[test]
    fn confirm_commitment_target_is_durable_only() {
        assert_eq!(normalize_confirm_commitment(None).unwrap(), "confirmed");
        assert_eq!(
            normalize_confirm_commitment(Some("finalized")).unwrap(),
            "finalized"
        );
        // processed is not a durable landing target.
        assert!(normalize_confirm_commitment(Some("processed")).is_err());
        assert!(commitment_rank("finalized") > commitment_rank("confirmed"));
        assert!(commitment_rank("confirmed") > commitment_rank("processed"));
    }

    #[test]
    fn signature_validation_requires_64_bytes() {
        let signature = bs58::encode([7_u8; 64]).into_string();
        assert!(validate_signature(&signature, "signature").is_ok());
        assert!(validate_signature("not-base58-!!!", "signature").is_err());
        let short = bs58::encode([7_u8; 32]).into_string();
        assert!(validate_signature(&short, "signature").is_err());
    }

    #[test]
    fn idempotency_key_is_claimed_once() {
        let state = sample_state();
        assert!(state.claim_idempotency_key("settle:abc"));
        // Second claim of the same key within the TTL window is suppressed.
        assert!(!state.claim_idempotency_key("settle:abc"));
        // A distinct key is independent.
        assert!(state.claim_idempotency_key("settle:def"));
    }

    #[test]
    fn confirm_slot_bounds_in_flight_and_releases_on_drop() {
        let counter = Arc::new(AtomicU64::new(0));
        let mut slots = Vec::new();
        for _ in 0..MAX_CONFIRM_POLLERS_IN_FLIGHT {
            slots.push(ConfirmSlot::try_acquire(&counter).expect("under cap"));
        }
        // At the cap, further acquisitions are shed (and do not leak a slot).
        assert!(ConfirmSlot::try_acquire(&counter).is_none());
        assert_eq!(counter.load(Ordering::Acquire), MAX_CONFIRM_POLLERS_IN_FLIGHT);
        // Dropping a slot frees capacity again.
        slots.pop();
        assert!(ConfirmSlot::try_acquire(&counter).is_some());
    }

    #[test]
    fn deferred_confirm_outcome_is_not_reached() {
        let outcome = deferred_confirm_outcome("sig", "finalized");
        assert_eq!(outcome.status, "deferred");
        assert!(!outcome.reached);
        assert_eq!(outcome.polls, 0);
        assert!(outcome.error.is_some());
    }

    #[test]
    fn mainnet_gate_blocks_unflagged_broadcast() {
        // Devnet is unaffected regardless of broadcast flags.
        assert!(enforce_mainnet_settlement_gate("devnet", true, true, true, false).is_ok());
        // Mainnet with any broadcast capability needs the explicit second flag.
        assert!(enforce_mainnet_settlement_gate("mainnet-beta", true, false, false, false).is_err());
        assert!(enforce_mainnet_settlement_gate("mainnet-beta", false, true, false, false).is_err());
        assert!(enforce_mainnet_settlement_gate("mainnet-beta", false, false, true, false).is_err());
        // With the second flag, mainnet broadcast is permitted.
        assert!(enforce_mainnet_settlement_gate("mainnet-beta", true, true, true, true).is_ok());
        // Mainnet with no broadcast capability is always fine.
        assert!(enforce_mainnet_settlement_gate("mainnet-beta", false, false, false, false).is_ok());
    }

    #[test]
    fn nats_broadcast_requires_unauthenticated_bus_ack() {
        // NATS broadcast off: ack irrelevant.
        assert!(enforce_nats_broadcast_ack(false, false).is_ok());
        // NATS broadcast on without ack is refused.
        assert!(enforce_nats_broadcast_ack(true, false).is_err());
        // NATS broadcast on with explicit ack is permitted.
        assert!(enforce_nats_broadcast_ack(true, true).is_ok());
    }

    #[test]
    fn idempotency_key_released_allows_retry() {
        let state = sample_state();
        assert!(state.claim_idempotency_key("settle:retry"));
        // A failed broadcast releases the key so the same request id can retry
        // (Solana dedupes resubmissions of the same signed tx by signature).
        state.release_idempotency_key("settle:retry");
        assert!(state.claim_idempotency_key("settle:retry"));
    }

    #[test]
    fn settlement_core_rejects_cluster_drift_and_bad_tx() {
        let core = SettlementCore {
            request_id: Some("settle-demo".to_string()),
            cluster: Some("mainnet-beta".to_string()),
            transaction: general_purpose::STANDARD.encode([1_u8, 2, 3]),
            encoding: Some("base64".to_string()),
            commitment: None,
            skip_preflight: None,
            max_retries: None,
            min_context_slot: None,
        };
        let errors = validate_settlement_core(&core, "devnet").expect_err("cluster drift");
        assert!(errors
            .iter()
            .any(|error| error.contains("cluster must match configured SOLANA_CLUSTER")));

        let valid = SettlementCore {
            cluster: Some("devnet".to_string()),
            ..core_with_tx(general_purpose::STANDARD.encode([9_u8; 64]))
        };
        let (cluster, encoding, bytes) =
            validate_settlement_core(&valid, "devnet").expect("valid core");
        assert_eq!(cluster, "devnet");
        assert_eq!(encoding, "base64");
        assert_eq!(bytes, 64);
    }

    fn core_with_tx(transaction: String) -> SettlementCore {
        SettlementCore {
            request_id: Some("settle-demo".to_string()),
            cluster: Some("devnet".to_string()),
            transaction,
            encoding: Some("base64".to_string()),
            commitment: None,
            skip_preflight: None,
            max_retries: None,
            min_context_slot: None,
        }
    }

    #[test]
    fn confirm_options_resolve_with_defaults() {
        let (target, timeout, interval) = resolve_confirm_target(&None).unwrap();
        assert_eq!(target, "confirmed");
        assert_eq!(timeout, DEFAULT_CONFIRM_TIMEOUT_MS);
        assert_eq!(interval, DEFAULT_CONFIRM_POLL_INTERVAL_MS);

        let options = Some(ConfirmOptions {
            target_commitment: Some("finalized".to_string()),
            timeout_ms: Some(5_000),
            poll_interval_ms: Some(500),
        });
        let (target, timeout, interval) = resolve_confirm_target(&options).unwrap();
        assert_eq!(target, "finalized");
        assert_eq!(timeout, 5_000);
        assert_eq!(interval, 500);
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

async fn publish_settlement_outcome(state: &AppState, payload: Value) {
    let Some(nats) = &state.nats else {
        return;
    };
    let Ok(encoded) = serde_json::to_vec(&payload) else {
        state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
        log_error(
            "contract-settlement-result-serialize-failed",
            "Settlement/resolution outcome could not be serialized for NATS.",
            json!({}),
        );
        return;
    };
    if let Err(error) = nats
        .publish(state.settlement_result_subject.clone(), encoded.into())
        .await
    {
        state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
        state
            .metrics
            .nats_publish_errors_total
            .fetch_add(1, Ordering::Relaxed);
        publish_runtime_critical_event(
            state,
            "contract-settlement-result-publish-failed",
            "Settlement/resolution outcome NATS publish failed.",
            json!({
                "subject": &state.settlement_result_subject,
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

/// Publish-only helper for the blockchain feature suite: fire-and-forget a JSON
/// payload to a fixed subject (index events, MEV alerts, bridge attestations),
/// counting the same NATS metrics as the contract publish paths. No-op when NATS
/// is not connected.
pub(crate) async fn publish_blockchain_event(state: &AppState, subject: &str, payload: Value) {
    let Some(nats) = &state.nats else {
        return;
    };
    let Ok(encoded) = serde_json::to_vec(&payload) else {
        state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
        return;
    };
    if nats.publish(subject.to_string(), encoded.into()).await.is_err() {
        state
            .metrics
            .nats_publish_errors_total
            .fetch_add(1, Ordering::Relaxed);
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum NatsKind {
    Validate,
    Settle,
    Resolve,
    EscrowResults,
}

impl NatsKind {
    fn label(self) -> &'static str {
        match self {
            NatsKind::Validate => "validate",
            NatsKind::Settle => "settle",
            NatsKind::Resolve => "resolve",
            NatsKind::EscrowResults => "escrow-results",
        }
    }
}

async fn run_nats_loop(
    state: AppState,
    subject: String,
    queue_group: Option<String>,
    kind: NatsKind,
) {
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
        "Contract service NATS subscription loop is starting.",
        json!({
            "subject": &subject,
            "queueGroup": &queue_group,
            "kind": kind.label(),
            "natsSettlementEnabled": state.nats_settlement_enabled,
        }),
    );
    loop {
        let subscribe = match &queue_group {
            Some(group) => nats.queue_subscribe(subject.clone(), group.clone()).await,
            None => nats.subscribe(subject.clone()).await,
        };
        let mut subscription = match subscribe {
            Ok(subscription) => subscription,
            Err(error) => {
                state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                publish_runtime_critical_event(
                    &state,
                    "contract-nats-subscribe-failed",
                    "Contract service could not subscribe to a NATS subject; retrying in 5s.",
                    json!({ "subject": &subject, "kind": kind.label(), "error": error.to_string() }),
                )
                .await;
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
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
                    "Contract service rejected an oversized NATS message.",
                    json!({
                        "kind": kind.label(),
                        "payloadBytes": payload.len(),
                        "maxPayloadBytes": MAX_NATS_PAYLOAD_BYTES,
                    }),
                )
                .await;
                continue;
            }
            match kind {
                NatsKind::Validate => process_nats_validate(&state, &payload).await,
                NatsKind::Settle => process_nats_settle(&state, &payload).await,
                NatsKind::Resolve => process_nats_resolve(&state, &payload).await,
                NatsKind::EscrowResults => process_nats_escrow_result(&state, &payload).await,
            }
        }

        // The stream only ends when the subscription is closed or the connection is
        // torn down without async-nats restoring it. That silently kills a consumer,
        // so alert and re-subscribe instead of dying quietly.
        publish_runtime_critical_event(
            &state,
            "contract-nats-loop-ended",
            "Contract service NATS subscription loop ended unexpectedly; re-subscribing in 5s.",
            json!({ "subject": &subject, "kind": kind.label() }),
        )
        .await;
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

fn nats_payload_invalid(state: &AppState, kind: NatsKind, error: &str) {
    state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .nats_payload_rejected_total
        .fetch_add(1, Ordering::Relaxed);
    log_warn(
        "contract-nats-payload-invalid",
        "Contract service rejected an invalid NATS message.",
        json!({ "kind": kind.label(), "error": error }),
    );
}

async fn process_nats_validate(state: &AppState, payload: &[u8]) {
    match serde_json::from_slice::<ContractRequest>(payload) {
        Ok(request) => {
            state
                .metrics
                .validations_total
                .fetch_add(1, Ordering::Relaxed);
            let request_id = request_id(request.request_id.as_ref(), "contract-validation");
            let result = match validate_contract_request(&request, &state.default_cluster) {
                Ok(response) => json!({
                    "messageKind": "solana.contract.validation.result",
                    "source": "dd-contract-service",
                    "result": response
                }),
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
            publish_contract_result(state, result).await;
            publish_contract_event(state, "solana.contract.validation", &request_id, ok).await;
        }
        Err(error) => nats_payload_invalid(state, NatsKind::Validate, &error.to_string()),
    }
}

/// Drives validate -> simulate -> (optional broadcast+confirm) -> publish for a
/// settlement-style NATS message. Broadcast only happens when
/// `CONTRACT_NATS_SETTLEMENT_ENABLED=true`, because NATS messages carry no auth
/// header; otherwise the service validates, simulates, and reports.
#[allow(clippy::too_many_arguments)]
async fn nats_settlement_flow(
    state: &AppState,
    req_id: &str,
    schema_version: &str,
    message_kind: &str,
    event_type: &str,
    core: &SettlementCore,
    confirm: &Option<ConfirmOptions>,
    idem_key: Option<String>,
    mut extra: Map<String, Value>,
) {
    let (cluster, encoding, decoded_bytes) =
        match validate_settlement_core(core, &state.default_cluster) {
            Ok(validated) => validated,
            Err(errors) => {
                let mut outcome = base_settlement_outcome(
                    message_kind,
                    schema_version,
                    req_id,
                    &state.default_cluster,
                    false,
                    "rejected",
                );
                outcome.append(&mut extra);
                outcome.insert("errors".to_string(), json!(errors));
                publish_settlement_outcome(state, Value::Object(outcome)).await;
                publish_contract_event(state, event_type, req_id, false).await;
                return;
            }
        };

    // Always simulate for visibility, regardless of broadcast policy.
    let sim_tx = core.tx_request(true);
    let simulation = match simulate_params(&sim_tx, encoding) {
        Ok(params) => solana_rpc(state, "simulateTransaction", params)
            .await
            .unwrap_or_else(|error| json!({ "error": error })),
        Err(error) => json!({ "error": error }),
    };

    let mut outcome = base_settlement_outcome(
        message_kind,
        schema_version,
        req_id,
        &cluster,
        false,
        "validated",
    );
    outcome.append(&mut extra);
    outcome.insert("encoding".to_string(), json!(encoding));
    outcome.insert("transactionBytes".to_string(), json!(decoded_bytes));
    outcome.insert("simulation".to_string(), simulation);

    if !state.nats_settlement_enabled {
        outcome.insert("broadcast".to_string(), json!(false));
        outcome.insert(
            "note".to_string(),
            json!("NATS-initiated broadcast is disabled; set CONTRACT_NATS_SETTLEMENT_ENABLED=true to broadcast"),
        );
        publish_settlement_outcome(state, Value::Object(outcome)).await;
        publish_contract_event(state, event_type, req_id, true).await;
        return;
    }

    // Broadcast path: guard double-broadcast only on explicit request ids
    // (an absent id must not collapse distinct messages onto one key).
    if let Some(key) = &idem_key {
        if !state.claim_idempotency_key(key) {
            state
                .metrics
                .settlement_idempotent_hits_total
                .fetch_add(1, Ordering::Relaxed);
            outcome.insert("idempotent".to_string(), json!(true));
            outcome.insert("broadcast".to_string(), json!(false));
            publish_settlement_outcome(state, Value::Object(outcome)).await;
            return;
        }
    }
    let release = |state: &AppState| {
        if let Some(key) = &idem_key {
            state.release_idempotency_key(key);
        }
    };

    let send_tx = core.tx_request(false);
    let send = match send_params(&send_tx, encoding, state.allow_skip_preflight) {
        Ok(params) => params,
        Err(error) => {
            release(state);
            outcome.insert("broadcast".to_string(), json!(false));
            outcome.insert("error".to_string(), json!(error));
            publish_settlement_outcome(state, Value::Object(outcome)).await;
            publish_contract_event(state, event_type, req_id, false).await;
            return;
        }
    };
    match solana_rpc(state, "sendTransaction", send).await {
        Ok(signature_value) => {
            let signature = signature_value.as_str().unwrap_or_default().to_string();
            if signature.is_empty() {
                release(state);
                outcome.insert("broadcast".to_string(), json!(false));
                outcome.insert(
                    "error".to_string(),
                    json!("sendTransaction did not return a signature"),
                );
                publish_settlement_outcome(state, Value::Object(outcome)).await;
                publish_contract_event(state, event_type, req_id, false).await;
                return;
            }
            let (target, timeout_ms, poll_ms) =
                resolve_confirm_target(confirm).unwrap_or_else(|_| {
                    (
                        "confirmed".to_string(),
                        DEFAULT_CONFIRM_TIMEOUT_MS,
                        DEFAULT_CONFIRM_POLL_INTERVAL_MS,
                    )
                });
            let confirmation =
                bounded_confirm(state, &signature, &target, timeout_ms, poll_ms).await;
            let reached = confirmation.reached;
            outcome.insert("ok".to_string(), json!(reached));
            outcome.insert("status".to_string(), json!("broadcast"));
            outcome.insert("broadcast".to_string(), json!(true));
            outcome.insert("signature".to_string(), json!(signature));
            outcome.insert("confirmation".to_string(), json!(confirmation));
            publish_settlement_outcome(state, Value::Object(outcome)).await;
            publish_contract_event(state, event_type, req_id, reached).await;
        }
        Err(error) => {
            release(state);
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            outcome.insert("broadcast".to_string(), json!(false));
            outcome.insert("error".to_string(), json!(error.clone()));
            publish_settlement_outcome(state, Value::Object(outcome)).await;
            publish_runtime_critical_event(
                state,
                "contract-nats-settlement-send-failed",
                "NATS settlement broadcast failed.",
                json!({ "requestId": req_id, "messageKind": message_kind, "error": error }),
            )
            .await;
        }
    }
}

fn base_settlement_outcome(
    message_kind: &str,
    schema_version: &str,
    req_id: &str,
    cluster: &str,
    ok: bool,
    status: &str,
) -> Map<String, Value> {
    let mut map = Map::new();
    map.insert("messageKind".to_string(), json!(message_kind));
    map.insert("source".to_string(), json!(SERVICE_NAME));
    map.insert("ok".to_string(), json!(ok));
    map.insert("status".to_string(), json!(status));
    map.insert("requestId".to_string(), json!(req_id));
    map.insert("schemaVersion".to_string(), json!(schema_version));
    map.insert("cluster".to_string(), json!(cluster));
    map.insert("generatedAtMs".to_string(), json!(now_ms()));
    map
}

async fn process_nats_settle(state: &AppState, payload: &[u8]) {
    match serde_json::from_slice::<SettlementRequest>(payload) {
        Ok(request) => {
            state
                .metrics
                .settlements_total
                .fetch_add(1, Ordering::Relaxed);
            let req_id = request_id(request.request_id.as_ref(), "contract-settlement");
            if request.schema_version != SETTLEMENT_SCHEMA_VERSION {
                state
                    .metrics
                    .settlement_errors_total
                    .fetch_add(1, Ordering::Relaxed);
                nats_payload_invalid(
                    state,
                    NatsKind::Settle,
                    &format!("schemaVersion must be {SETTLEMENT_SCHEMA_VERSION}"),
                );
                return;
            }
            let mut extra = Map::new();
            extra.insert("kind".to_string(), json!("settlement"));
            extra.insert("action".to_string(), json!(request.action.as_str()));
            extra.insert("escrowId".to_string(), json!(request.escrow_id));
            extra.insert("contractId".to_string(), json!(request.contract_id));
            let idem_key = explicit_request_id(request.request_id.as_ref())
                .map(|key| format!("nats:settle:{key}"));
            nats_settlement_flow(
                state,
                &req_id,
                SETTLEMENT_SCHEMA_VERSION,
                "solana.settlement.outcome",
                "solana.contract.settlement",
                &request.core(),
                &request.confirm,
                idem_key,
                extra,
            )
            .await;
        }
        Err(error) => nats_payload_invalid(state, NatsKind::Settle, &error.to_string()),
    }
}

async fn process_nats_resolve(state: &AppState, payload: &[u8]) {
    match serde_json::from_slice::<ResolutionRequest>(payload) {
        Ok(request) => {
            state
                .metrics
                .resolutions_total
                .fetch_add(1, Ordering::Relaxed);
            let req_id = request_id(request.request_id.as_ref(), "contract-resolution");
            let mut schema_errors = Vec::new();
            if request.schema_version != RESOLUTION_SCHEMA_VERSION {
                schema_errors.push(format!("schemaVersion must be {RESOLUTION_SCHEMA_VERSION}"));
            }
            if !request.decision.allowed_actions().contains(&request.action) {
                schema_errors.push(format!(
                    "decision {} does not permit settlement action {}",
                    request.decision.as_str(),
                    request.action.as_str()
                ));
            }
            if !schema_errors.is_empty() {
                state
                    .metrics
                    .resolution_errors_total
                    .fetch_add(1, Ordering::Relaxed);
                nats_payload_invalid(state, NatsKind::Resolve, &schema_errors.join("; "));
                return;
            }
            let mut extra = Map::new();
            extra.insert("kind".to_string(), json!("resolution"));
            extra.insert("decision".to_string(), json!(request.decision.as_str()));
            extra.insert("action".to_string(), json!(request.action.as_str()));
            extra.insert("escrowId".to_string(), json!(request.escrow_id));
            extra.insert("disputeId".to_string(), json!(request.dispute_id));
            extra.insert("arbiter".to_string(), json!(request.arbiter));
            let idem_key = explicit_request_id(request.request_id.as_ref())
                .map(|key| format!("nats:resolve:{key}"));
            nats_settlement_flow(
                state,
                &req_id,
                RESOLUTION_SCHEMA_VERSION,
                "solana.resolution.outcome",
                "solana.contract.resolution",
                &request.core(),
                &request.confirm,
                idem_key,
                extra,
            )
            .await;
        }
        Err(error) => nats_payload_invalid(state, NatsKind::Resolve, &error.to_string()),
    }
}

/// Verifier surface: confirm a settlement signature carried in an escrow result.
async fn process_nats_escrow_result(state: &AppState, payload: &[u8]) {
    let value = match serde_json::from_slice::<Value>(payload) {
        Ok(value) => value,
        Err(error) => {
            nats_payload_invalid(state, NatsKind::EscrowResults, &error.to_string());
            return;
        }
    };
    // Escrow settlement results may carry an RPC sendTransaction signature.
    let signature = value
        .pointer("/result/signature")
        .or_else(|| value.pointer("/signature"))
        .or_else(|| value.pointer("/result/result"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let Some(signature) = signature.filter(|sig| validate_signature(sig, "signature").is_ok())
    else {
        // Not a settlement result we can confirm; ignore quietly.
        return;
    };
    let req_id = value
        .pointer("/result/requestId")
        .or_else(|| value.pointer("/requestId"))
        .and_then(Value::as_str)
        .unwrap_or("escrow-result")
        .to_string();

    // Confirm to finality off the subscription loop so a slow poll can't
    // head-of-line block draining; bound the concurrent fan-out.
    let Some(slot) = ConfirmSlot::try_acquire(&state.confirm_in_flight) else {
        log_warn(
            "contract-escrow-confirm-shed",
            "Escrow confirmation shed because the in-flight verifier cap was reached.",
            json!({ "requestId": req_id, "maxInFlight": MAX_CONFIRM_POLLERS_IN_FLIGHT }),
        );
        return;
    };
    let task_state = state.clone();
    tokio::spawn(async move {
        let _slot = slot;
        let confirmation = confirm_signature(
            &task_state,
            &signature,
            "finalized",
            DEFAULT_CONFIRM_TIMEOUT_MS,
            DEFAULT_CONFIRM_POLL_INTERVAL_MS,
        )
        .await;
        let outcome = json!({
            "messageKind": "solana.escrow.confirmation",
            "source": SERVICE_NAME,
            "ok": confirmation.reached,
            "status": "verified",
            "requestId": req_id,
            "kind": "escrow-confirmation",
            "signature": signature,
            "confirmation": confirmation,
            "generatedAtMs": now_ms()
        });
        publish_settlement_outcome(&task_state, outcome).await;
    });
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
    let _otel = dd_telemetry::init("dd-contract-service");

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

    let settlement_enabled = env_bool("SOLANA_SETTLEMENT_ENABLED", false);
    let resolution_enabled = env_bool("SOLANA_RESOLUTION_ENABLED", false);
    let nats_settlement_enabled = env_bool("CONTRACT_NATS_SETTLEMENT_ENABLED", false);
    let settlement_auth_secret = env_secret("CONTRACT_SETTLEMENT_AUTH_SECRET");
    if (settlement_enabled || resolution_enabled) && settlement_auth_secret.is_none() {
        return Err(config_error(
            "SOLANA_SETTLEMENT_ENABLED/SOLANA_RESOLUTION_ENABLED require CONTRACT_SETTLEMENT_AUTH_SECRET",
        )
        .into());
    }
    if nats_settlement_enabled && !send_enabled {
        return Err(config_error(
            "CONTRACT_NATS_SETTLEMENT_ENABLED=true requires SOLANA_SEND_ENABLED=true",
        )
        .into());
    }
    enforce_nats_broadcast_ack(
        nats_settlement_enabled,
        env_bool("CONTRACT_NATS_SETTLEMENT_ACK_UNAUTHENTICATED_BUS", false),
    )
    .map_err(config_error)?;
    let mainnet_settlement_enabled = env_bool("SOLANA_MAINNET_SETTLEMENT_ENABLED", false);
    enforce_mainnet_settlement_gate(
        &default_cluster,
        send_enabled,
        settlement_enabled,
        resolution_enabled,
        mainnet_settlement_enabled,
    )
    .map_err(config_error)?;
    let escrow_confirm_enabled = env_bool("CONTRACT_ESCROW_CONFIRM_ENABLED", false);

    let rpc_timeout_seconds = env_u64("SOLANA_RPC_TIMEOUT_SECONDS", 20);
    let result_subject = env_value("CONTRACT_RESULT_SUBJECT", CONTRACTS_SOLANA_RESULTS_SUBJECT);
    let settlement_result_subject = env_value(
        "CONTRACT_SETTLEMENT_RESULT_SUBJECT",
        CONTRACTS_SOLANA_SETTLEMENT_RESULTS_SUBJECT,
    );
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
    let settle_subject = env_value("CONTRACT_SETTLE_SUBJECT", CONTRACTS_SOLANA_SETTLE_SUBJECT);
    let settle_queue_group = env_value(
        "CONTRACT_SETTLE_QUEUE_GROUP",
        CONTRACTS_SOLANA_SETTLE_QUEUE_GROUP,
    );
    let resolve_subject = env_value("CONTRACT_RESOLVE_SUBJECT", CONTRACTS_SOLANA_RESOLVE_SUBJECT);
    let resolve_queue_group = env_value(
        "CONTRACT_RESOLVE_QUEUE_GROUP",
        CONTRACTS_SOLANA_RESOLVE_QUEUE_GROUP,
    );
    let escrow_results_subject =
        env_value("CONTRACT_ESCROW_RESULT_SUBJECT", ESCROW_SOLANA_RESULTS_SUBJECT);
    let escrow_confirm_queue_group = env_value(
        "CONTRACT_ESCROW_CONFIRM_QUEUE_GROUP",
        "dd-contract-service-escrow-confirm",
    );

    let rpc_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(rpc_timeout_seconds))
        .redirect(reqwest::redirect::Policy::none())
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

    // Keyless blockchain feature suite. Reuses the validated Solana RPC URL +
    // cluster and the shared HTTP client; enforces its own mainnet/auth gates.
    let blockchain =
        blockchain::BlockchainState::from_env(rpc_client.clone(), &solana_rpc_url, &default_cluster)
            .map_err(config_error)?;
    let coordination = coordination::CoordinationState::from_env(rpc_client.clone())
        .map_err(config_error)?;
    let solana_features = solana_features::SolanaFeatureState::from_env(rpc_client.clone())
        .map_err(config_error)?;
    let rpc_max_in_flight = env_u64("SOLANA_RPC_MAX_IN_FLIGHT", MAX_RPC_IN_FLIGHT as u64)
        .clamp(1, 512) as usize;

    let state = AppState {
        rpc_client,
        solana_rpc_url,
        default_cluster,
        send_enabled,
        send_auth_secret,
        allow_skip_preflight,
        settlement_enabled,
        resolution_enabled,
        nats_settlement_enabled,
        mainnet_settlement_enabled,
        settlement_auth_secret,
        nats,
        result_subject,
        settlement_result_subject,
        event_subject,
        critical_event_subject,
        metrics: Arc::new(Metrics::default()),
        idempotency: Arc::new(Mutex::new(HashMap::new())),
        confirm_in_flight: Arc::new(AtomicU64::new(0)),
        rpc_slots: Arc::new(tokio::sync::Semaphore::new(rpc_max_in_flight)),
        coordination,
        solana_features,
        blockchain,
    };

    log_info(
        "contract-service-starting",
        "Contract service runtime configuration loaded.",
        json!({
            "cluster": &state.default_cluster,
            "sendEnabled": state.send_enabled,
            "skipPreflightAllowed": state.allow_skip_preflight,
            "settlementEnabled": state.settlement_enabled,
            "resolutionEnabled": state.resolution_enabled,
            "natsSettlementEnabled": state.nats_settlement_enabled,
            "mainnetSettlementEnabled": state.mainnet_settlement_enabled,
            "escrowConfirmEnabled": escrow_confirm_enabled,
            "resultSubject": &state.result_subject,
            "settlementResultSubject": &state.settlement_result_subject,
            "eventSubject": &state.event_subject,
            "criticalEventSubject": &state.critical_event_subject,
            "natsEnabled": state.nats.is_some(),
            "rpcMaxInFlight": rpc_max_in_flight,
            "coordinationEnabled": state.coordination.enabled(),
            "coordinationRequired": state.coordination.required(),
            "formalMethodsEnabled": state.solana_features.formal_enabled(),
            "formalMethodsGithubOrganizations": state.solana_features.allowed_github_orgs(),
            "blockchain": state.blockchain.startup_summary(),
        }),
    );

    if state.nats.is_some() {
        tokio::spawn(run_nats_loop(
            state.clone(),
            validate_subject,
            Some(queue_group),
            NatsKind::Validate,
        ));
        tokio::spawn(run_nats_loop(
            state.clone(),
            settle_subject,
            Some(settle_queue_group),
            NatsKind::Settle,
        ));
        tokio::spawn(run_nats_loop(
            state.clone(),
            resolve_subject,
            Some(resolve_queue_group),
            NatsKind::Resolve,
        ));
        if escrow_confirm_enabled {
            tokio::spawn(run_nats_loop(
                state.clone(),
                escrow_results_subject,
                Some(escrow_confirm_queue_group),
                NatsKind::EscrowResults,
            ));
        }
    }

    let app = Router::new()
        .route("/", get(home))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .route("/metrics", get(metrics))
        .route("/status", get(status_http))
        .route("/schema", get(schema_http))
        .route("/schema/settlement", get(settlement_schema_http))
        .route("/schema/resolution", get(resolution_schema_http))
        .route("/example", get(example_http))
        .route("/example/settlement", get(settlement_example_http))
        .route("/validate", post(validate_http))
        .route("/simulate", post(simulate_http))
        .route("/send", post(send_http))
        .route("/blockhash", get(blockhash_http))
        .route("/account", post(account_http))
        .route("/balance", post(balance_http))
        .route("/fee", post(fee_http))
        .route("/rent-exemption", get(rent_exemption_http))
        .route("/transaction", post(transaction_http))
        .route("/confirm", post(confirm_http))
        .route("/simulate-settlement", post(simulate_settlement_http))
        .route("/settle", post(settle_http))
        .route("/resolve", post(resolve_http))
        .merge(solana_features::router())
        .merge(blockchain::router())
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
    axum::serve(listener, app.layer(dd_telemetry::http_trace_layer()))
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}
