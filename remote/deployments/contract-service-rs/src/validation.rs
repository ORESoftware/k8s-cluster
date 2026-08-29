use std::net::IpAddr;

use axum::http::{HeaderMap, StatusCode};
use base64::{engine::general_purpose, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::shared::{now_ms, request_id, sensitive_eq};
use crate::state::{
    AppState, DEFAULT_COMMITMENT, DEFAULT_COMPUTE_UNITS, MAX_ACCOUNTS_PER_INSTRUCTION,
    MAX_COMPUTE_UNITS_PER_INSTRUCTION, MAX_INSTRUCTIONS, MAX_INSTRUCTION_DATA_BYTES, MAX_LABEL_LEN,
    MAX_MEMO_BYTES, MAX_REQUEST_ID_LEN, MAX_SEND_RETRIES, MAX_SIGNATURE_LEN,
    MAX_SIGNED_TRANSACTION_BYTES, MAX_TRANSACTION_COMPUTE_UNITS, SCHEMA_VERSION, SEND_AUTH_HEADER,
    SETTLEMENT_AUTH_HEADER,
};

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContractRequest {
    pub(crate) schema_version: String,
    pub(crate) request_id: Option<String>,
    pub(crate) cluster: Option<String>,
    pub(crate) program_id: String,
    pub(crate) payer: Option<String>,
    pub(crate) recent_blockhash: Option<String>,
    pub(crate) commitment: Option<String>,
    pub(crate) memo: Option<String>,
    pub(crate) instructions: Vec<ContractInstructionInput>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContractInstructionInput {
    pub(crate) name: String,
    pub(crate) program_id: Option<String>,
    pub(crate) accounts: Vec<AccountMetaInput>,
    pub(crate) data_base64: Option<String>,
    pub(crate) data_base58: Option<String>,
    pub(crate) compute_units: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccountMetaInput {
    pub(crate) pubkey: String,
    pub(crate) is_signer: Option<bool>,
    pub(crate) is_writable: Option<bool>,
    pub(crate) label: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContractValidationResponse {
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
pub(crate) struct TransactionRpcRequest {
    pub(crate) request_id: Option<String>,
    pub(crate) cluster: Option<String>,
    pub(crate) transaction: String,
    pub(crate) encoding: Option<String>,
    pub(crate) commitment: Option<String>,
    pub(crate) sig_verify: Option<bool>,
    pub(crate) replace_recent_blockhash: Option<bool>,
    pub(crate) skip_preflight: Option<bool>,
    pub(crate) max_retries: Option<usize>,
    pub(crate) min_context_slot: Option<u64>,
}

pub(crate) fn validate_request_id(input: Option<&String>, errors: &mut Vec<String>) {
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

pub(crate) fn normalize_cluster(input: Option<&str>, fallback: &str) -> Result<String, String> {
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

pub(crate) fn normalize_request_cluster(
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

pub(crate) fn normalize_commitment(input: Option<&str>) -> Result<Option<String>, String> {
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

pub(crate) fn normalize_commitment_or_default(input: Option<&str>) -> Result<String, String> {
    Ok(normalize_commitment(input)?.unwrap_or_else(|| DEFAULT_COMMITMENT.to_string()))
}

pub(crate) fn normalize_encoding(input: Option<&str>) -> Result<&'static str, String> {
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

pub(crate) fn validate_pubkey(value: &str, label: &str) -> Result<(), String> {
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

pub(crate) fn authorize_send(
    headers: &HeaderMap,
    state: &AppState,
) -> Result<(), (StatusCode, &'static str)> {
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

pub(crate) fn authorize_settlement(
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
pub(crate) fn validate_signature(value: &str, label: &str) -> Result<String, String> {
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

/// NATS-initiated settlement messages carry no auth header, and the NATS bus has
/// no per-subject authorization, so enabling NATS-triggered broadcast lets any
/// publisher to the settle/resolve subjects trigger an on-chain send. Require an
/// explicit acknowledgment so this cannot be turned on by flipping a single
/// boolean; lock down NATS (authz / NetworkPolicy) before setting the ack.
pub(crate) fn enforce_nats_broadcast_ack(
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
pub(crate) fn enforce_mainnet_settlement_gate(
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

/// Any route or background consumer that can reach `sendTransaction` must use
/// both cross-replica fences. This prevents a future deployment from enabling a
/// broadcast flag while silently leaving coordination optional or disabled.
pub(crate) fn enforce_broadcast_coordination(
    broadcast_capable: bool,
    coordination_enabled: bool,
    coordination_required: bool,
) -> Result<(), String> {
    if broadcast_capable && (!coordination_enabled || !coordination_required) {
        return Err(
            "Solana broadcast capabilities require CONTRACT_COORDINATION_ENABLED=true and CONTRACT_COORDINATION_REQUIRED=true"
                .to_string(),
        );
    }
    Ok(())
}

pub(crate) fn validate_solana_rpc_url(
    raw: &str,
    allow_private_rpc: bool,
) -> Result<String, String> {
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

pub(crate) fn validate_contract_request(
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

pub(crate) fn validate_signed_transaction(
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

pub(crate) fn simulate_params(
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

pub(crate) fn send_params(
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
