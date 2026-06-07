use std::{
    collections::HashSet,
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
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose, Engine as _};
use dd_nats_subject_defs::{RUNTIME_CRITICAL_EVENTS_SUBJECT, RUNTIME_EVENTS_SUBJECT};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const SCHEMA_VERSION: &str = "solana.escrow.v1";
const SERVICE_NAME: &str = "dd-escrow-rs";
const SERVICE_NAMESPACE: &str = "remote-dev";
const LOG_SCHEMA: &str = "dd.log.v1";
const LOG_SCOPE: &str = "dd-escrow-rs";
const DEFAULT_COMMITMENT: &str = "confirmed";
const SETTLEMENT_AUTH_HEADER: &str = "x-escrow-settlement-auth";
const DEFAULT_VALIDATE_SUBJECT: &str = "dd.remote.escrow.solana.validate";
const DEFAULT_RESULT_SUBJECT: &str = "dd.remote.escrow.solana.results";
const DEFAULT_QUEUE_GROUP: &str = "dd-escrow-rs";
const MAX_HTTP_BODY_BYTES: usize = 512 * 1024;
const MAX_NATS_PAYLOAD_BYTES: usize = 512 * 1024;
const MAX_SIGNED_TRANSACTION_BYTES: usize = 256 * 1024;
const MAX_REQUEST_ID_LEN: usize = 128;
const MAX_ESCROW_ID_LEN: usize = 128;
const MAX_LABEL_LEN: usize = 80;
const MAX_MEMO_BYTES: usize = 1024;
const MAX_METADATA_BYTES: usize = 4096;
const MAX_PARTIES: usize = 12;
const MAX_MILESTONES: usize = 24;
const MAX_TOKEN_AMOUNT_LEN: usize = 80;
const MAX_DISPUTE_WINDOW_SECONDS: u64 = 90 * 24 * 60 * 60;
const MAX_INSPECTION_SECONDS: u64 = 30 * 24 * 60 * 60;
const MAX_SEND_RETRIES: usize = 20;

#[derive(Clone)]
struct AppState {
    rpc_client: reqwest::Client,
    solana_rpc_url: String,
    default_cluster: String,
    settlement_enabled: bool,
    settlement_auth_secret: Option<String>,
    allow_skip_preflight: bool,
    nats: Option<async_nats::Client>,
    validate_subject: String,
    result_subject: String,
    event_subject: String,
    critical_event_subject: String,
    metrics: Arc<Metrics>,
}

#[derive(Default)]
struct Metrics {
    validations_total: AtomicU64,
    validation_errors_total: AtomicU64,
    simulations_total: AtomicU64,
    settlements_total: AtomicU64,
    settlement_errors_total: AtomicU64,
    rpc_requests_total: AtomicU64,
    rpc_errors_total: AtomicU64,
    policy_rejections_total: AtomicU64,
    auth_failures_total: AtomicU64,
    nats_messages_total: AtomicU64,
    nats_payload_rejected_total: AtomicU64,
    nats_results_published_total: AtomicU64,
    nats_events_published_total: AtomicU64,
    nats_critical_events_published_total: AtomicU64,
    nats_publish_errors_total: AtomicU64,
    errors_total: AtomicU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum EscrowKind {
    MarketplaceOrder,
    Milestone,
    FreelanceContract,
    DigitalDelivery,
    OtcTrade,
    RentalDeposit,
    Bounty,
    SubscriptionRelease,
    GroupBuy,
    DisputeResolution,
}

impl EscrowKind {
    fn as_str(self) -> &'static str {
        match self {
            EscrowKind::MarketplaceOrder => "marketplace-order",
            EscrowKind::Milestone => "milestone",
            EscrowKind::FreelanceContract => "freelance-contract",
            EscrowKind::DigitalDelivery => "digital-delivery",
            EscrowKind::OtcTrade => "otc-trade",
            EscrowKind::RentalDeposit => "rental-deposit",
            EscrowKind::Bounty => "bounty",
            EscrowKind::SubscriptionRelease => "subscription-release",
            EscrowKind::GroupBuy => "group-buy",
            EscrowKind::DisputeResolution => "dispute-resolution",
        }
    }
}

const ESCROW_KINDS: [EscrowKind; 10] = [
    EscrowKind::MarketplaceOrder,
    EscrowKind::Milestone,
    EscrowKind::FreelanceContract,
    EscrowKind::DigitalDelivery,
    EscrowKind::OtcTrade,
    EscrowKind::RentalDeposit,
    EscrowKind::Bounty,
    EscrowKind::SubscriptionRelease,
    EscrowKind::GroupBuy,
    EscrowKind::DisputeResolution,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum PartyRole {
    Buyer,
    Seller,
    Payer,
    Payee,
    Client,
    Contractor,
    Depositor,
    Recipient,
    Arbitrator,
    Broker,
    Platform,
    Contributor,
    Maintainer,
    Fulfiller,
    Landlord,
    Tenant,
}

impl PartyRole {
    fn as_str(self) -> &'static str {
        match self {
            PartyRole::Buyer => "buyer",
            PartyRole::Seller => "seller",
            PartyRole::Payer => "payer",
            PartyRole::Payee => "payee",
            PartyRole::Client => "client",
            PartyRole::Contractor => "contractor",
            PartyRole::Depositor => "depositor",
            PartyRole::Recipient => "recipient",
            PartyRole::Arbitrator => "arbitrator",
            PartyRole::Broker => "broker",
            PartyRole::Platform => "platform",
            PartyRole::Contributor => "contributor",
            PartyRole::Maintainer => "maintainer",
            PartyRole::Fulfiller => "fulfiller",
            PartyRole::Landlord => "landlord",
            PartyRole::Tenant => "tenant",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum AssetType {
    Sol,
    SplToken,
    Nft,
    CompressedNft,
    CustomProgram,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum ReleaseMode {
    BuyerApproval,
    MilestoneApproval,
    TimeLocked,
    OracleSignal,
    ArbiterDecision,
    MultiSig,
    DeliveryProof,
    ExpiryRefund,
    ManualOperator,
}

impl ReleaseMode {
    fn as_str(self) -> &'static str {
        match self {
            ReleaseMode::BuyerApproval => "buyer-approval",
            ReleaseMode::MilestoneApproval => "milestone-approval",
            ReleaseMode::TimeLocked => "time-locked",
            ReleaseMode::OracleSignal => "oracle-signal",
            ReleaseMode::ArbiterDecision => "arbiter-decision",
            ReleaseMode::MultiSig => "multi-sig",
            ReleaseMode::DeliveryProof => "delivery-proof",
            ReleaseMode::ExpiryRefund => "expiry-refund",
            ReleaseMode::ManualOperator => "manual-operator",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

#[derive(Debug, Clone)]
struct KindSpec {
    kind: EscrowKind,
    description: &'static str,
    min_parties: usize,
    required_roles: Vec<PartyRole>,
    release_modes: Vec<ReleaseMode>,
    settlement_actions: Vec<SettlementAction>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct KindCatalogEntry {
    kind: &'static str,
    description: &'static str,
    min_parties: usize,
    required_roles: Vec<&'static str>,
    release_modes: Vec<&'static str>,
    settlement_actions: Vec<&'static str>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct EscrowIntentRequest {
    schema_version: String,
    request_id: Option<String>,
    cluster: Option<String>,
    kind: EscrowKind,
    escrow_id: String,
    parties: Vec<EscrowParty>,
    asset: EscrowAsset,
    terms: EscrowTerms,
    settlement_plan: Option<SettlementPlan>,
    memo: Option<String>,
    metadata: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct EscrowParty {
    role: PartyRole,
    pubkey: String,
    label: Option<String>,
    required_signer: Option<bool>,
    payout_bps: Option<u16>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct EscrowAsset {
    asset_type: AssetType,
    mint: Option<String>,
    amount_lamports: Option<u64>,
    token_amount: Option<String>,
    decimals: Option<u8>,
    collection: Option<String>,
    escrow_vault: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct EscrowTerms {
    release_mode: ReleaseMode,
    settlement_actions: Option<Vec<SettlementAction>>,
    dispute_window_seconds: Option<u64>,
    inspection_period_seconds: Option<u64>,
    timeout_unix_seconds: Option<u64>,
    milestones: Option<Vec<EscrowMilestone>>,
    required_approvals: Option<Vec<PartyRole>>,
    max_partial_releases: Option<u8>,
    delivery_required: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct EscrowMilestone {
    id: String,
    label: Option<String>,
    amount_bps: Option<u16>,
    due_unix_seconds: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SettlementPlan {
    program_id: String,
    vault_pubkey: Option<String>,
    fee_bps: Option<u16>,
    memo_required: Option<bool>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct EscrowValidationResponse {
    ok: bool,
    request_id: String,
    schema_version: &'static str,
    cluster: String,
    escrow_id: String,
    kind: EscrowKind,
    asset_type: AssetType,
    release_mode: ReleaseMode,
    party_count: usize,
    milestone_count: usize,
    required_roles: Vec<&'static str>,
    allowed_settlement_actions: Vec<&'static str>,
    on_chain_settlement_ready: bool,
    digest: String,
    warnings: Vec<String>,
    generated_at_ms: u128,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct EscrowSettlementRequest {
    schema_version: String,
    request_id: Option<String>,
    cluster: Option<String>,
    kind: EscrowKind,
    escrow_id: String,
    action: SettlementAction,
    transaction: String,
    encoding: Option<String>,
    commitment: Option<String>,
    skip_preflight: Option<bool>,
    max_retries: Option<usize>,
    min_context_slot: Option<u64>,
    intent: Option<EscrowIntentRequest>,
}

#[derive(Debug)]
struct ValidatedSettlement {
    request_id: String,
    cluster: String,
    transaction_bytes: Vec<u8>,
    transaction_digest: String,
    warnings: Vec<String>,
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn now_unix_nano() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
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

fn json_error(status: StatusCode, message: impl Into<String>, details: Value) -> Response {
    (
        status,
        Json(json!({
            "ok": false,
            "error": message.into(),
            "details": details,
            "generatedAtMs": now_ms(),
        })),
    )
        .into_response()
}

fn config_error(message: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, message.into())
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
    validate_token(value, "requestId", MAX_REQUEST_ID_LEN, errors);
}

fn validate_token(value: &str, label: &str, max_len: usize, errors: &mut Vec<String>) {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        errors.push(format!("{label} must not be empty"));
        return;
    }
    if trimmed.len() != value.len() {
        errors.push(format!(
            "{label} must not contain leading or trailing whitespace"
        ));
    }
    if trimmed.len() > max_len {
        errors.push(format!("{label} must be at most {max_len} bytes"));
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':'))
    {
        errors.push(format!(
            "{label} may contain only ASCII letters, numbers, '.', '_', '-', and ':'"
        ));
    }
}

fn validate_label(value: &str, label: &str, errors: &mut Vec<String>) {
    validate_token(value, label, MAX_LABEL_LEN, errors);
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

fn normalize_commitment(input: Option<&str>) -> Result<String, String> {
    let value = input
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_COMMITMENT);
    let normalized = value.to_ascii_lowercase();
    match normalized.as_str() {
        "processed" | "confirmed" | "finalized" => Ok(normalized),
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

fn kind_spec(kind: EscrowKind) -> KindSpec {
    match kind {
        EscrowKind::MarketplaceOrder => KindSpec {
            kind,
            description: "Buyer/seller order escrow with approval, delivery proof, refund, or dispute settlement.",
            min_parties: 2,
            required_roles: vec![PartyRole::Buyer, PartyRole::Seller],
            release_modes: vec![
                ReleaseMode::BuyerApproval,
                ReleaseMode::DeliveryProof,
                ReleaseMode::ExpiryRefund,
                ReleaseMode::ArbiterDecision,
            ],
            settlement_actions: vec![
                SettlementAction::Fund,
                SettlementAction::Release,
                SettlementAction::Refund,
                SettlementAction::DisputeAward,
                SettlementAction::Expire,
                SettlementAction::Cancel,
            ],
        },
        EscrowKind::Milestone => KindSpec {
            kind,
            description: "Milestone escrow that can release partial payouts as approved work checkpoints complete.",
            min_parties: 2,
            required_roles: vec![PartyRole::Payer, PartyRole::Payee],
            release_modes: vec![
                ReleaseMode::MilestoneApproval,
                ReleaseMode::MultiSig,
                ReleaseMode::ArbiterDecision,
            ],
            settlement_actions: vec![
                SettlementAction::Fund,
                SettlementAction::PartialRelease,
                SettlementAction::Release,
                SettlementAction::Refund,
                SettlementAction::DisputeAward,
            ],
        },
        EscrowKind::FreelanceContract => KindSpec {
            kind,
            description: "Client/contractor escrow for scoped services, milestones, inspection, and dispute awards.",
            min_parties: 2,
            required_roles: vec![PartyRole::Client, PartyRole::Contractor],
            release_modes: vec![
                ReleaseMode::MilestoneApproval,
                ReleaseMode::BuyerApproval,
                ReleaseMode::ArbiterDecision,
            ],
            settlement_actions: vec![
                SettlementAction::Fund,
                SettlementAction::PartialRelease,
                SettlementAction::Release,
                SettlementAction::Refund,
                SettlementAction::DisputeAward,
                SettlementAction::Cancel,
            ],
        },
        EscrowKind::DigitalDelivery => KindSpec {
            kind,
            description: "Digital goods escrow that prefers delivery proof plus an inspection window before release.",
            min_parties: 2,
            required_roles: vec![PartyRole::Buyer, PartyRole::Seller],
            release_modes: vec![
                ReleaseMode::DeliveryProof,
                ReleaseMode::BuyerApproval,
                ReleaseMode::TimeLocked,
            ],
            settlement_actions: vec![
                SettlementAction::Fund,
                SettlementAction::Release,
                SettlementAction::Refund,
                SettlementAction::Expire,
                SettlementAction::Cancel,
            ],
        },
        EscrowKind::OtcTrade => KindSpec {
            kind,
            description: "OTC token/NFT trade escrow for brokered or multi-signature settlement.",
            min_parties: 2,
            required_roles: vec![PartyRole::Buyer, PartyRole::Seller],
            release_modes: vec![ReleaseMode::MultiSig, ReleaseMode::ArbiterDecision],
            settlement_actions: vec![
                SettlementAction::Fund,
                SettlementAction::Release,
                SettlementAction::Refund,
                SettlementAction::SplitRelease,
                SettlementAction::DisputeAward,
            ],
        },
        EscrowKind::RentalDeposit => KindSpec {
            kind,
            description: "Rental deposit escrow with time locks, inspection windows, refund, and damage awards.",
            min_parties: 2,
            required_roles: vec![PartyRole::Landlord, PartyRole::Tenant],
            release_modes: vec![
                ReleaseMode::TimeLocked,
                ReleaseMode::ExpiryRefund,
                ReleaseMode::ArbiterDecision,
            ],
            settlement_actions: vec![
                SettlementAction::Fund,
                SettlementAction::Refund,
                SettlementAction::SplitRelease,
                SettlementAction::DisputeAward,
                SettlementAction::Expire,
            ],
        },
        EscrowKind::Bounty => KindSpec {
            kind,
            description: "Bounty escrow for a payer and fulfiller, optionally reviewed by a maintainer.",
            min_parties: 2,
            required_roles: vec![PartyRole::Payer, PartyRole::Fulfiller],
            release_modes: vec![
                ReleaseMode::BuyerApproval,
                ReleaseMode::MilestoneApproval,
                ReleaseMode::ArbiterDecision,
            ],
            settlement_actions: vec![
                SettlementAction::Fund,
                SettlementAction::Release,
                SettlementAction::Refund,
                SettlementAction::PartialRelease,
                SettlementAction::DisputeAward,
                SettlementAction::Cancel,
            ],
        },
        EscrowKind::SubscriptionRelease => KindSpec {
            kind,
            description: "Recurring or streaming escrow for scheduled releases with optional oracle or operator approval.",
            min_parties: 2,
            required_roles: vec![PartyRole::Payer, PartyRole::Payee],
            release_modes: vec![
                ReleaseMode::TimeLocked,
                ReleaseMode::OracleSignal,
                ReleaseMode::ManualOperator,
            ],
            settlement_actions: vec![
                SettlementAction::Fund,
                SettlementAction::PartialRelease,
                SettlementAction::Release,
                SettlementAction::Refund,
                SettlementAction::Expire,
                SettlementAction::Cancel,
            ],
        },
        EscrowKind::GroupBuy => KindSpec {
            kind,
            description: "Group-buy escrow with multiple contributors and a seller or broker before final release/refund.",
            min_parties: 3,
            required_roles: vec![PartyRole::Contributor, PartyRole::Seller],
            release_modes: vec![
                ReleaseMode::MultiSig,
                ReleaseMode::TimeLocked,
                ReleaseMode::ManualOperator,
            ],
            settlement_actions: vec![
                SettlementAction::Fund,
                SettlementAction::Release,
                SettlementAction::Refund,
                SettlementAction::SplitRelease,
                SettlementAction::Expire,
                SettlementAction::Cancel,
            ],
        },
        EscrowKind::DisputeResolution => KindSpec {
            kind,
            description: "Dispute-first escrow that requires an arbitrator and settles by refund, split, or award.",
            min_parties: 3,
            required_roles: vec![PartyRole::Payer, PartyRole::Payee, PartyRole::Arbitrator],
            release_modes: vec![ReleaseMode::ArbiterDecision, ReleaseMode::MultiSig],
            settlement_actions: vec![
                SettlementAction::Fund,
                SettlementAction::Refund,
                SettlementAction::SplitRelease,
                SettlementAction::DisputeAward,
                SettlementAction::Cancel,
            ],
        },
    }
}

fn kind_catalog() -> Vec<KindCatalogEntry> {
    ESCROW_KINDS
        .iter()
        .copied()
        .map(|kind| {
            let spec = kind_spec(kind);
            KindCatalogEntry {
                kind: spec.kind.as_str(),
                description: spec.description,
                min_parties: spec.min_parties,
                required_roles: spec
                    .required_roles
                    .iter()
                    .copied()
                    .map(PartyRole::as_str)
                    .collect(),
                release_modes: spec
                    .release_modes
                    .iter()
                    .copied()
                    .map(ReleaseMode::as_str)
                    .collect(),
                settlement_actions: spec
                    .settlement_actions
                    .iter()
                    .copied()
                    .map(SettlementAction::as_str)
                    .collect(),
            }
        })
        .collect()
}

fn validate_token_amount(value: &str, label: &str, errors: &mut Vec<String>) {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        errors.push(format!("{label} must not be empty"));
        return;
    }
    if trimmed.len() != value.len() {
        errors.push(format!(
            "{label} must not contain leading or trailing whitespace"
        ));
    }
    if trimmed.len() > MAX_TOKEN_AMOUNT_LEN {
        errors.push(format!(
            "{label} must be at most {MAX_TOKEN_AMOUNT_LEN} digits"
        ));
    }
    if !trimmed.chars().all(|ch| ch.is_ascii_digit()) {
        errors.push(format!("{label} must be a positive integer string"));
    }
    if trimmed.chars().all(|ch| ch == '0') {
        errors.push(format!("{label} must be greater than zero"));
    }
}

fn validate_asset(
    asset: &EscrowAsset,
    request: &EscrowIntentRequest,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    if let Some(vault) = &asset.escrow_vault {
        if let Err(error) = validate_pubkey(vault, "asset.escrowVault") {
            errors.push(error);
        }
    }
    if let Some(collection) = &asset.collection {
        if let Err(error) = validate_pubkey(collection, "asset.collection") {
            errors.push(error);
        }
    }
    if let Some(decimals) = asset.decimals {
        if decimals > 12 {
            errors.push("asset.decimals must be at most 12".to_string());
        }
    }
    match asset.asset_type {
        AssetType::Sol => {
            match asset.amount_lamports {
                Some(amount) if amount > 0 => {}
                _ => errors.push(
                    "asset.amountLamports is required and must be greater than zero for SOL escrow"
                        .to_string(),
                ),
            }
            if asset.mint.is_some() {
                warnings.push("asset.mint is ignored for SOL escrow".to_string());
            }
        }
        AssetType::SplToken => {
            match &asset.mint {
                Some(mint) => {
                    if let Err(error) = validate_pubkey(mint, "asset.mint") {
                        errors.push(error);
                    }
                }
                None => errors.push("asset.mint is required for SPL token escrow".to_string()),
            }
            match &asset.token_amount {
                Some(amount) => validate_token_amount(amount, "asset.tokenAmount", errors),
                None => {
                    errors.push("asset.tokenAmount is required for SPL token escrow".to_string())
                }
            }
        }
        AssetType::Nft | AssetType::CompressedNft => {
            match &asset.mint {
                Some(mint) => {
                    if let Err(error) = validate_pubkey(mint, "asset.mint") {
                        errors.push(error);
                    }
                }
                None => errors.push("asset.mint is required for NFT escrow".to_string()),
            }
            if let Some(amount) = &asset.token_amount {
                let trimmed = amount.trim();
                if trimmed != "1" {
                    errors.push(
                        "asset.tokenAmount must be omitted or set to '1' for NFT escrow"
                            .to_string(),
                    );
                }
            }
        }
        AssetType::CustomProgram => {
            if request.settlement_plan.is_none() {
                errors.push(
                    "settlementPlan is required for custom-program escrow assets".to_string(),
                );
            }
        }
    }
}

fn validate_parties(
    request: &EscrowIntentRequest,
    spec: &KindSpec,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    if request.parties.len() < spec.min_parties {
        errors.push(format!(
            "{} escrow requires at least {} parties",
            spec.kind.as_str(),
            spec.min_parties
        ));
    }
    if request.parties.len() > MAX_PARTIES {
        errors.push(format!(
            "parties must include at most {MAX_PARTIES} entries"
        ));
    }
    let mut roles = HashSet::new();
    let mut labels = HashSet::new();
    let mut payout_sum: u32 = 0;
    let mut payout_count = 0;
    let mut required_signers = 0;
    for (index, party) in request.parties.iter().enumerate() {
        roles.insert(party.role);
        if let Err(error) = validate_pubkey(&party.pubkey, &format!("parties[{index}].pubkey")) {
            errors.push(error);
        }
        if let Some(label) = &party.label {
            validate_label(label, &format!("parties[{index}].label"), errors);
            if !labels.insert(label.trim().to_ascii_lowercase()) {
                errors.push(format!("parties[{index}].label must be unique"));
            }
        }
        if party.required_signer.unwrap_or(false) {
            required_signers += 1;
        }
        if let Some(payout_bps) = party.payout_bps {
            payout_count += 1;
            payout_sum += u32::from(payout_bps);
            if payout_bps > 10_000 {
                errors.push(format!("parties[{index}].payoutBps must be at most 10000"));
            }
        }
    }
    for role in &spec.required_roles {
        if !roles.contains(role) {
            errors.push(format!(
                "{} escrow requires a party with role {}",
                spec.kind.as_str(),
                role.as_str()
            ));
        }
    }
    if payout_count > 0 && payout_sum != 10_000 {
        errors.push(
            "party payoutBps values must sum to exactly 10000 when any payoutBps is provided"
                .to_string(),
        );
    }
    if required_signers == 0 {
        warnings.push("no parties are marked requiredSigner=true; settlement relies entirely on the submitted signed transaction".to_string());
    }
    if request.kind == EscrowKind::GroupBuy {
        let contributors = request
            .parties
            .iter()
            .filter(|party| party.role == PartyRole::Contributor)
            .count();
        if contributors < 2 {
            errors.push("group-buy escrow requires at least two contributor parties".to_string());
        }
    }
}

fn validate_terms(
    request: &EscrowIntentRequest,
    spec: &KindSpec,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    if !spec.release_modes.contains(&request.terms.release_mode) {
        errors.push(format!(
            "{} escrow does not allow releaseMode {}",
            spec.kind.as_str(),
            request.terms.release_mode.as_str()
        ));
    }
    if let Some(actions) = &request.terms.settlement_actions {
        if actions.is_empty() {
            errors.push("terms.settlementActions must not be empty when provided".to_string());
        }
        for action in actions {
            if !spec.settlement_actions.contains(action) {
                errors.push(format!(
                    "{} escrow does not allow settlement action {}",
                    spec.kind.as_str(),
                    action.as_str()
                ));
            }
        }
    }
    if let Some(seconds) = request.terms.dispute_window_seconds {
        if seconds > MAX_DISPUTE_WINDOW_SECONDS {
            errors.push(format!(
                "terms.disputeWindowSeconds must be at most {MAX_DISPUTE_WINDOW_SECONDS}"
            ));
        }
    }
    if let Some(seconds) = request.terms.inspection_period_seconds {
        if seconds > MAX_INSPECTION_SECONDS {
            errors.push(format!(
                "terms.inspectionPeriodSeconds must be at most {MAX_INSPECTION_SECONDS}"
            ));
        }
    }
    if (matches!(
        request.terms.release_mode,
        ReleaseMode::TimeLocked | ReleaseMode::ExpiryRefund
    ) || request.kind == EscrowKind::SubscriptionRelease)
        && request.terms.timeout_unix_seconds.is_none()
    {
        errors.push(
            "terms.timeoutUnixSeconds is required for time-locked or expiry-refund escrow"
                .to_string(),
        );
    }
    if let Some(timeout) = request.terms.timeout_unix_seconds {
        if timeout <= now_unix_seconds() {
            errors.push("terms.timeoutUnixSeconds must be in the future".to_string());
        }
    }
    if request.terms.release_mode == ReleaseMode::MilestoneApproval {
        match &request.terms.milestones {
            Some(milestones) if !milestones.is_empty() => {}
            _ => errors
                .push("terms.milestones is required for milestone-approval escrow".to_string()),
        }
    }
    if let Some(max_partial) = request.terms.max_partial_releases {
        if usize::from(max_partial) > MAX_MILESTONES {
            errors.push(format!(
                "terms.maxPartialReleases must be at most {MAX_MILESTONES}"
            ));
        }
    }
    if let Some(approvals) = &request.terms.required_approvals {
        if approvals.is_empty() {
            errors.push("terms.requiredApprovals must not be empty when provided".to_string());
        }
        let party_roles: HashSet<PartyRole> =
            request.parties.iter().map(|party| party.role).collect();
        for role in approvals {
            if !party_roles.contains(role) {
                errors.push(format!(
                    "terms.requiredApprovals includes role {} but no party has that role",
                    role.as_str()
                ));
            }
        }
    }
    if request.kind == EscrowKind::DigitalDelivery && request.terms.delivery_required != Some(true)
    {
        warnings.push("digital-delivery escrow should set terms.deliveryRequired=true".to_string());
    }
    if request.kind == EscrowKind::OtcTrade
        && !matches!(
            request.asset.asset_type,
            AssetType::SplToken | AssetType::Nft | AssetType::CompressedNft
        )
    {
        warnings.push("otc-trade escrow usually uses an SPL token or NFT asset".to_string());
    }
}

fn validate_milestones(
    milestones: &Option<Vec<EscrowMilestone>>,
    errors: &mut Vec<String>,
) -> usize {
    let Some(milestones) = milestones else {
        return 0;
    };
    if milestones.len() > MAX_MILESTONES {
        errors.push(format!(
            "terms.milestones must include at most {MAX_MILESTONES} entries"
        ));
    }
    let mut ids = HashSet::new();
    let mut bps_sum = 0_u32;
    let mut bps_count = 0_usize;
    for (index, milestone) in milestones.iter().enumerate() {
        validate_token(
            &milestone.id,
            &format!("terms.milestones[{index}].id"),
            MAX_LABEL_LEN,
            errors,
        );
        if !ids.insert(milestone.id.trim().to_ascii_lowercase()) {
            errors.push(format!("terms.milestones[{index}].id must be unique"));
        }
        if let Some(label) = &milestone.label {
            validate_label(label, &format!("terms.milestones[{index}].label"), errors);
        }
        if let Some(amount_bps) = milestone.amount_bps {
            bps_count += 1;
            bps_sum += u32::from(amount_bps);
            if amount_bps > 10_000 {
                errors.push(format!(
                    "terms.milestones[{index}].amountBps must be at most 10000"
                ));
            }
        }
        if let Some(due) = milestone.due_unix_seconds {
            if due <= now_unix_seconds() {
                errors.push(format!(
                    "terms.milestones[{index}].dueUnixSeconds must be in the future"
                ));
            }
        }
    }
    if bps_count > 0 && bps_count == milestones.len() && bps_sum != 10_000 {
        errors.push("terms.milestones amountBps values must sum to exactly 10000 when every milestone has amountBps".to_string());
    }
    milestones.len()
}

fn validate_settlement_plan(plan: &Option<SettlementPlan>, errors: &mut Vec<String>) {
    let Some(plan) = plan else {
        return;
    };
    if let Err(error) = validate_pubkey(&plan.program_id, "settlementPlan.programId") {
        errors.push(error);
    }
    if let Some(vault) = &plan.vault_pubkey {
        if let Err(error) = validate_pubkey(vault, "settlementPlan.vaultPubkey") {
            errors.push(error);
        }
    }
    if let Some(fee_bps) = plan.fee_bps {
        if fee_bps > 1000 {
            errors.push("settlementPlan.feeBps must be at most 1000".to_string());
        }
    }
}

fn validate_memo_and_metadata(request: &EscrowIntentRequest, errors: &mut Vec<String>) {
    if let Some(memo) = &request.memo {
        if memo.as_bytes().len() > MAX_MEMO_BYTES {
            errors.push(format!("memo must be at most {MAX_MEMO_BYTES} bytes"));
        }
    }
    if let Some(metadata) = &request.metadata {
        match serde_json::to_vec(metadata) {
            Ok(encoded) if encoded.len() <= MAX_METADATA_BYTES => {}
            Ok(encoded) => errors.push(format!(
                "metadata must serialize to at most {MAX_METADATA_BYTES} bytes, got {}",
                encoded.len()
            )),
            Err(error) => errors.push(format!("metadata could not be serialized: {error}")),
        }
    }
}

fn escrow_digest(request: &EscrowIntentRequest) -> String {
    let canonical = serde_json::to_vec(request).unwrap_or_default();
    let digest = Sha256::digest(canonical);
    format!("solana-escrow:{}", hex::encode(&digest[..16]))
}

fn transaction_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("solana-tx:{}", hex::encode(&digest[..16]))
}

fn validate_escrow_intent(
    request: &EscrowIntentRequest,
    default_cluster: &str,
) -> Result<EscrowValidationResponse, Vec<String>> {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    if request.schema_version != SCHEMA_VERSION {
        errors.push(format!(
            "schemaVersion must be {SCHEMA_VERSION}, got {}",
            request.schema_version
        ));
    }
    validate_request_id(request.request_id.as_ref(), &mut errors);
    validate_token(
        &request.escrow_id,
        "escrowId",
        MAX_ESCROW_ID_LEN,
        &mut errors,
    );
    let cluster = match normalize_request_cluster(request.cluster.as_deref(), default_cluster) {
        Ok(cluster) => cluster,
        Err(error) => {
            errors.push(error);
            default_cluster.to_string()
        }
    };
    let spec = kind_spec(request.kind);
    validate_parties(request, &spec, &mut errors, &mut warnings);
    validate_asset(&request.asset, request, &mut errors, &mut warnings);
    validate_terms(request, &spec, &mut errors, &mut warnings);
    let milestone_count = validate_milestones(&request.terms.milestones, &mut errors);
    validate_settlement_plan(&request.settlement_plan, &mut errors);
    validate_memo_and_metadata(request, &mut errors);
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(EscrowValidationResponse {
        ok: true,
        request_id: request_id(request.request_id.as_ref(), "escrow-validation"),
        schema_version: SCHEMA_VERSION,
        cluster,
        escrow_id: request.escrow_id.clone(),
        kind: request.kind,
        asset_type: request.asset.asset_type,
        release_mode: request.terms.release_mode,
        party_count: request.parties.len(),
        milestone_count,
        required_roles: spec
            .required_roles
            .iter()
            .copied()
            .map(PartyRole::as_str)
            .collect(),
        allowed_settlement_actions: spec
            .settlement_actions
            .iter()
            .copied()
            .map(SettlementAction::as_str)
            .collect(),
        on_chain_settlement_ready: request.settlement_plan.is_some(),
        digest: escrow_digest(request),
        warnings,
        generated_at_ms: now_ms(),
    })
}

fn validate_signed_transaction(transaction: &str, encoding: &str) -> Result<Vec<u8>, String> {
    let value = transaction.trim();
    if value.is_empty() {
        return Err("transaction must not be empty".to_string());
    }
    let bytes = match encoding {
        "base64" => general_purpose::STANDARD
            .decode(value)
            .map_err(|error| format!("transaction is not valid base64: {error}"))?,
        "base58" => bs58::decode(value)
            .into_vec()
            .map_err(|error| format!("transaction is not valid base58: {error}"))?,
        other => return Err(format!("unsupported transaction encoding: {other}")),
    };
    if bytes.len() > MAX_SIGNED_TRANSACTION_BYTES {
        return Err(format!(
            "transaction must be at most {MAX_SIGNED_TRANSACTION_BYTES} bytes, got {}",
            bytes.len()
        ));
    }
    Ok(bytes)
}

fn validate_settlement_request(
    request: &EscrowSettlementRequest,
    default_cluster: &str,
    allow_skip_preflight: bool,
) -> Result<ValidatedSettlement, Vec<String>> {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    if request.schema_version != SCHEMA_VERSION {
        errors.push(format!(
            "schemaVersion must be {SCHEMA_VERSION}, got {}",
            request.schema_version
        ));
    }
    validate_request_id(request.request_id.as_ref(), &mut errors);
    validate_token(
        &request.escrow_id,
        "escrowId",
        MAX_ESCROW_ID_LEN,
        &mut errors,
    );
    let cluster = match normalize_request_cluster(request.cluster.as_deref(), default_cluster) {
        Ok(cluster) => cluster,
        Err(error) => {
            errors.push(error);
            default_cluster.to_string()
        }
    };
    let spec = kind_spec(request.kind);
    if !spec.settlement_actions.contains(&request.action) {
        errors.push(format!(
            "{} escrow does not allow settlement action {}",
            request.kind.as_str(),
            request.action.as_str()
        ));
    }
    let encoding = match normalize_encoding(request.encoding.as_deref()) {
        Ok(encoding) => encoding,
        Err(error) => {
            errors.push(error);
            "base64"
        }
    };
    if let Err(error) = normalize_commitment(request.commitment.as_deref()) {
        errors.push(error);
    }
    if request.skip_preflight == Some(true) && !allow_skip_preflight {
        errors.push(
            "skipPreflight is disabled by policy; set SOLANA_ALLOW_SKIP_PREFLIGHT=true to allow it"
                .to_string(),
        );
    }
    if let Some(max_retries) = request.max_retries {
        if max_retries > MAX_SEND_RETRIES {
            errors.push(format!(
                "maxRetries must be at most {MAX_SEND_RETRIES}, got {max_retries}"
            ));
        }
    }
    let transaction_bytes = match validate_signed_transaction(&request.transaction, encoding) {
        Ok(bytes) => bytes,
        Err(error) => {
            errors.push(error);
            Vec::new()
        }
    };
    if let Some(intent) = &request.intent {
        match validate_escrow_intent(intent, default_cluster) {
            Ok(intent_response) => {
                if intent.kind != request.kind {
                    errors.push("intent.kind must match settlement kind".to_string());
                }
                if intent.escrow_id != request.escrow_id {
                    errors.push("intent.escrowId must match settlement escrowId".to_string());
                }
                if intent_response.cluster != cluster {
                    errors.push("intent.cluster must match settlement cluster".to_string());
                }
                warnings.extend(intent_response.warnings);
            }
            Err(intent_errors) => {
                errors.extend(
                    intent_errors
                        .into_iter()
                        .map(|error| format!("intent.{error}")),
                );
            }
        }
    } else {
        warnings.push("no intent was attached; settlement action is validated only against kind and transaction policy".to_string());
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(ValidatedSettlement {
        request_id: request_id(request.request_id.as_ref(), "escrow-settlement"),
        cluster,
        transaction_digest: transaction_digest(&transaction_bytes),
        transaction_bytes,
        warnings,
    })
}

fn authorize_settlement(
    headers: &HeaderMap,
    state: &AppState,
) -> Result<(), (StatusCode, &'static str)> {
    let Some(secret) = &state.settlement_auth_secret else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "settlement sending is not configured with ESCROW_SETTLEMENT_AUTH_SECRET",
        ));
    };
    let Some(value) = headers
        .get(SETTLEMENT_AUTH_HEADER)
        .and_then(|value| value.to_str().ok())
    else {
        return Err((
            StatusCode::UNAUTHORIZED,
            "missing x-escrow-settlement-auth header",
        ));
    };
    if !sensitive_eq(value.trim(), secret) {
        return Err((
            StatusCode::UNAUTHORIZED,
            "invalid x-escrow-settlement-auth header",
        ));
    }
    Ok(())
}

fn simulate_params(request: &EscrowSettlementRequest, encoding: &str, commitment: &str) -> Value {
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

fn send_params(request: &EscrowSettlementRequest, encoding: &str, commitment: &str) -> Value {
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

async fn rpc_json(
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

async fn home() -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": SCHEMA_VERSION,
        "supportedKinds": kind_catalog(),
        "endpoints": {
            "types": "/types",
            "schema": "/schema",
            "example": "/example",
            "validate": "POST /validate",
            "simulateSettlement": "POST /simulate-settlement",
            "settle": "POST /settle",
            "status": "/status",
            "metrics": "/metrics"
        }
    }))
}

async fn healthz() -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": SCHEMA_VERSION,
    }))
}

async fn types_http() -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "kinds": kind_catalog(),
    }))
}

async fn schema_http() -> impl IntoResponse {
    Json(json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://dd.local/schemas/solana.escrow.v1.json",
        "title": "solana.escrow.v1",
        "type": "object",
        "required": ["schemaVersion", "kind", "escrowId", "parties", "asset", "terms"],
        "properties": {
            "schemaVersion": { "const": SCHEMA_VERSION },
            "requestId": { "type": "string", "maxLength": MAX_REQUEST_ID_LEN },
            "cluster": { "enum": ["mainnet-beta", "devnet", "testnet", "localnet", "custom"] },
            "kind": { "enum": ESCROW_KINDS.iter().map(|kind| kind.as_str()).collect::<Vec<_>>() },
            "escrowId": { "type": "string", "maxLength": MAX_ESCROW_ID_LEN },
            "parties": { "type": "array", "minItems": 2, "maxItems": MAX_PARTIES },
            "asset": { "type": "object" },
            "terms": { "type": "object" },
            "settlementPlan": { "type": "object" },
            "memo": { "type": "string", "maxLength": MAX_MEMO_BYTES },
            "metadata": { "type": "object" }
        }
    }))
}

fn example_request() -> EscrowIntentRequest {
    let system_program = "11111111111111111111111111111111".to_string();
    EscrowIntentRequest {
        schema_version: SCHEMA_VERSION.to_string(),
        request_id: Some("escrow-demo".to_string()),
        cluster: Some("devnet".to_string()),
        kind: EscrowKind::MarketplaceOrder,
        escrow_id: "order.demo.001".to_string(),
        parties: vec![
            EscrowParty {
                role: PartyRole::Buyer,
                pubkey: system_program.clone(),
                label: Some("buyer".to_string()),
                required_signer: Some(true),
                payout_bps: None,
            },
            EscrowParty {
                role: PartyRole::Seller,
                pubkey: system_program.clone(),
                label: Some("seller".to_string()),
                required_signer: Some(false),
                payout_bps: Some(10_000),
            },
        ],
        asset: EscrowAsset {
            asset_type: AssetType::Sol,
            mint: None,
            amount_lamports: Some(1_000_000),
            token_amount: None,
            decimals: None,
            collection: None,
            escrow_vault: Some(system_program.clone()),
        },
        terms: EscrowTerms {
            release_mode: ReleaseMode::BuyerApproval,
            settlement_actions: Some(vec![
                SettlementAction::Fund,
                SettlementAction::Release,
                SettlementAction::Refund,
                SettlementAction::DisputeAward,
            ]),
            dispute_window_seconds: Some(7 * 24 * 60 * 60),
            inspection_period_seconds: Some(48 * 60 * 60),
            timeout_unix_seconds: Some(now_unix_seconds() + 30 * 24 * 60 * 60),
            milestones: None,
            required_approvals: Some(vec![PartyRole::Buyer]),
            max_partial_releases: None,
            delivery_required: Some(true),
        },
        settlement_plan: Some(SettlementPlan {
            program_id: system_program,
            vault_pubkey: None,
            fee_bps: Some(50),
            memo_required: Some(true),
        }),
        memo: Some("example marketplace escrow intent".to_string()),
        metadata: Some(json!({ "source": "dd-escrow-rs-example" })),
    }
}

async fn example_http() -> impl IntoResponse {
    Json(example_request())
}

async fn validate_http(
    State(state): State<AppState>,
    Json(request): Json<EscrowIntentRequest>,
) -> Response {
    state
        .metrics
        .validations_total
        .fetch_add(1, Ordering::Relaxed);
    match validate_escrow_intent(&request, &state.default_cluster) {
        Ok(response) => Json(response).into_response(),
        Err(errors) => {
            state
                .metrics
                .validation_errors_total
                .fetch_add(1, Ordering::Relaxed);
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            json_error(
                StatusCode::BAD_REQUEST,
                "escrow intent validation failed",
                json!({ "errors": errors }),
            )
        }
    }
}

async fn simulate_settlement_http(
    State(state): State<AppState>,
    Json(request): Json<EscrowSettlementRequest>,
) -> Response {
    state
        .metrics
        .simulations_total
        .fetch_add(1, Ordering::Relaxed);
    let validation = match validate_settlement_request(
        &request,
        &state.default_cluster,
        state.allow_skip_preflight,
    ) {
        Ok(validation) => validation,
        Err(errors) => {
            state
                .metrics
                .settlement_errors_total
                .fetch_add(1, Ordering::Relaxed);
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            return json_error(
                StatusCode::BAD_REQUEST,
                "settlement simulation validation failed",
                json!({ "errors": errors }),
            );
        }
    };
    let encoding = normalize_encoding(request.encoding.as_deref()).unwrap_or("base64");
    let commitment = normalize_commitment(request.commitment.as_deref())
        .unwrap_or_else(|_| DEFAULT_COMMITMENT.to_string());
    match rpc_json(
        &state,
        "simulateTransaction",
        simulate_params(&request, encoding, &commitment),
        &validation.request_id,
    )
    .await
    {
        Ok(result) => Json(json!({
            "ok": true,
            "requestId": validation.request_id,
            "schemaVersion": SCHEMA_VERSION,
            "cluster": validation.cluster,
            "escrowId": request.escrow_id,
            "kind": request.kind,
            "action": request.action,
            "transactionBytes": validation.transaction_bytes.len(),
            "transactionDigest": validation.transaction_digest,
            "rpcMethod": "simulateTransaction",
            "result": result,
            "warnings": validation.warnings,
            "generatedAtMs": now_ms(),
        }))
        .into_response(),
        Err(error) => {
            state
                .metrics
                .settlement_errors_total
                .fetch_add(1, Ordering::Relaxed);
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            json_error(
                StatusCode::BAD_GATEWAY,
                "Solana settlement simulation failed",
                json!({ "error": error }),
            )
        }
    }
}

async fn settle_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<EscrowSettlementRequest>,
) -> Response {
    state
        .metrics
        .settlements_total
        .fetch_add(1, Ordering::Relaxed);
    if !state.settlement_enabled {
        state
            .metrics
            .policy_rejections_total
            .fetch_add(1, Ordering::Relaxed);
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "on-chain settlement sending is disabled; set SOLANA_SETTLEMENT_ENABLED=true to enable it",
            json!({}),
        );
    }
    if let Err((status, message)) = authorize_settlement(&headers, &state) {
        state
            .metrics
            .auth_failures_total
            .fetch_add(1, Ordering::Relaxed);
        return json_error(status, message, json!({}));
    }
    let validation = match validate_settlement_request(
        &request,
        &state.default_cluster,
        state.allow_skip_preflight,
    ) {
        Ok(validation) => validation,
        Err(errors) => {
            state
                .metrics
                .settlement_errors_total
                .fetch_add(1, Ordering::Relaxed);
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            return json_error(
                StatusCode::BAD_REQUEST,
                "settlement request validation failed",
                json!({ "errors": errors }),
            );
        }
    };
    let encoding = normalize_encoding(request.encoding.as_deref()).unwrap_or("base64");
    let commitment = normalize_commitment(request.commitment.as_deref())
        .unwrap_or_else(|_| DEFAULT_COMMITMENT.to_string());
    match rpc_json(
        &state,
        "sendTransaction",
        send_params(&request, encoding, &commitment),
        &validation.request_id,
    )
    .await
    {
        Ok(result) => {
            publish_escrow_event(
                &state,
                "solana.escrow.settlement",
                &validation.request_id,
                true,
            )
            .await;
            Json(json!({
                "ok": true,
                "requestId": validation.request_id,
                "schemaVersion": SCHEMA_VERSION,
                "cluster": validation.cluster,
                "escrowId": request.escrow_id,
                "kind": request.kind,
                "action": request.action,
                "transactionBytes": validation.transaction_bytes.len(),
                "transactionDigest": validation.transaction_digest,
                "rpcMethod": "sendTransaction",
                "result": result,
                "warnings": validation.warnings,
                "generatedAtMs": now_ms(),
            }))
            .into_response()
        }
        Err(error) => {
            state
                .metrics
                .settlement_errors_total
                .fetch_add(1, Ordering::Relaxed);
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            publish_runtime_critical_event(
                &state,
                "escrow-settlement-send-failed",
                "Escrow settlement sendTransaction failed.",
                json!({
                    "requestId": validation.request_id,
                    "escrowId": request.escrow_id,
                    "kind": request.kind.as_str(),
                    "action": request.action.as_str(),
                    "error": error,
                }),
            )
            .await;
            json_error(
                StatusCode::BAD_GATEWAY,
                "Solana settlement send failed",
                json!({}),
            )
        }
    }
}

async fn status_http(State(state): State<AppState>) -> impl IntoResponse {
    let health = rpc_json(&state, "getHealth", json!([]), "escrow-status-health").await;
    let version = rpc_json(&state, "getVersion", json!([]), "escrow-status-version").await;
    let ok = health.is_ok() && version.is_ok();
    Json(json!({
        "ok": ok,
        "service": SERVICE_NAME,
        "schemaVersion": SCHEMA_VERSION,
        "cluster": state.default_cluster,
        "settlementEnabled": state.settlement_enabled,
        "skipPreflightAllowed": state.allow_skip_preflight,
        "natsEnabled": state.nats.is_some(),
        "validateSubject": state.validate_subject,
        "resultSubject": state.result_subject,
        "solana": {
            "health": health.unwrap_or_else(|error| json!({ "error": error })),
            "version": version.unwrap_or_else(|error| json!({ "error": error }))
        },
        "generatedAtMs": now_ms(),
    }))
}

fn label_value(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn metrics_body(state: &AppState) -> String {
    let metrics = &state.metrics;
    let cluster = label_value(&state.default_cluster);
    let settlement_enabled = if state.settlement_enabled {
        "true"
    } else {
        "false"
    };
    format!(
        concat!(
            "# HELP dd_escrow_rs_info Static service info.\n",
            "# TYPE dd_escrow_rs_info gauge\n",
            "dd_escrow_rs_info{{cluster=\"{}\",settlement_enabled=\"{}\"}} 1\n",
            "# HELP dd_escrow_rs_validations_total Escrow intent validations.\n",
            "# TYPE dd_escrow_rs_validations_total counter\n",
            "dd_escrow_rs_validations_total {}\n",
            "# HELP dd_escrow_rs_validation_errors_total Escrow validation failures.\n",
            "# TYPE dd_escrow_rs_validation_errors_total counter\n",
            "dd_escrow_rs_validation_errors_total {}\n",
            "# HELP dd_escrow_rs_simulations_total Settlement simulation requests.\n",
            "# TYPE dd_escrow_rs_simulations_total counter\n",
            "dd_escrow_rs_simulations_total {}\n",
            "# HELP dd_escrow_rs_settlements_total Settlement send requests.\n",
            "# TYPE dd_escrow_rs_settlements_total counter\n",
            "dd_escrow_rs_settlements_total {}\n",
            "# HELP dd_escrow_rs_settlement_errors_total Settlement validation or RPC errors.\n",
            "# TYPE dd_escrow_rs_settlement_errors_total counter\n",
            "dd_escrow_rs_settlement_errors_total {}\n",
            "# HELP dd_escrow_rs_rpc_requests_total Solana JSON-RPC requests.\n",
            "# TYPE dd_escrow_rs_rpc_requests_total counter\n",
            "dd_escrow_rs_rpc_requests_total {}\n",
            "# HELP dd_escrow_rs_rpc_errors_total Solana JSON-RPC errors.\n",
            "# TYPE dd_escrow_rs_rpc_errors_total counter\n",
            "dd_escrow_rs_rpc_errors_total {}\n",
            "# HELP dd_escrow_rs_policy_rejections_total Requests rejected by local safety policy.\n",
            "# TYPE dd_escrow_rs_policy_rejections_total counter\n",
            "dd_escrow_rs_policy_rejections_total {}\n",
            "# HELP dd_escrow_rs_auth_failures_total Settlement auth failures.\n",
            "# TYPE dd_escrow_rs_auth_failures_total counter\n",
            "dd_escrow_rs_auth_failures_total {}\n",
            "# HELP dd_escrow_rs_nats_messages_total NATS validation messages received.\n",
            "# TYPE dd_escrow_rs_nats_messages_total counter\n",
            "dd_escrow_rs_nats_messages_total {}\n",
            "# HELP dd_escrow_rs_nats_payload_rejected_total NATS payloads rejected before validation.\n",
            "# TYPE dd_escrow_rs_nats_payload_rejected_total counter\n",
            "dd_escrow_rs_nats_payload_rejected_total {}\n",
            "# HELP dd_escrow_rs_nats_published_total NATS messages published by kind.\n",
            "# TYPE dd_escrow_rs_nats_published_total counter\n",
            "dd_escrow_rs_nats_published_total{{subject_kind=\"result\"}} {}\n",
            "dd_escrow_rs_nats_published_total{{subject_kind=\"event\"}} {}\n",
            "dd_escrow_rs_nats_published_total{{subject_kind=\"critical\"}} {}\n",
            "# HELP dd_escrow_rs_nats_publish_errors_total NATS publish errors.\n",
            "# TYPE dd_escrow_rs_nats_publish_errors_total counter\n",
            "dd_escrow_rs_nats_publish_errors_total {}\n",
            "# HELP dd_escrow_rs_errors_total Aggregate service errors.\n",
            "# TYPE dd_escrow_rs_errors_total counter\n",
            "dd_escrow_rs_errors_total {}\n",
        ),
        cluster,
        settlement_enabled,
        metrics.validations_total.load(Ordering::Relaxed),
        metrics.validation_errors_total.load(Ordering::Relaxed),
        metrics.simulations_total.load(Ordering::Relaxed),
        metrics.settlements_total.load(Ordering::Relaxed),
        metrics.settlement_errors_total.load(Ordering::Relaxed),
        metrics.rpc_requests_total.load(Ordering::Relaxed),
        metrics.rpc_errors_total.load(Ordering::Relaxed),
        metrics.policy_rejections_total.load(Ordering::Relaxed),
        metrics.auth_failures_total.load(Ordering::Relaxed),
        metrics.nats_messages_total.load(Ordering::Relaxed),
        metrics.nats_payload_rejected_total.load(Ordering::Relaxed),
        metrics.nats_results_published_total.load(Ordering::Relaxed),
        metrics.nats_events_published_total.load(Ordering::Relaxed),
        metrics
            .nats_critical_events_published_total
            .load(Ordering::Relaxed),
        metrics.nats_publish_errors_total.load(Ordering::Relaxed),
        metrics.errors_total.load(Ordering::Relaxed),
    )
}

async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    (
        [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
        metrics_body(&state),
    )
}

async fn publish_validation_result(state: &AppState, payload: Value) {
    let Some(nats) = &state.nats else {
        return;
    };
    let Ok(encoded) = serde_json::to_vec(&payload) else {
        state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
        log_error(
            "escrow-validation-result-serialize-failed",
            "Escrow validation result could not be serialized for NATS.",
            json!({}),
        );
        return;
    };
    match nats
        .publish(state.result_subject.clone(), encoded.into())
        .await
    {
        Ok(()) => {
            state
                .metrics
                .nats_results_published_total
                .fetch_add(1, Ordering::Relaxed);
        }
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            state
                .metrics
                .nats_publish_errors_total
                .fetch_add(1, Ordering::Relaxed);
            publish_runtime_critical_event(
                state,
                "escrow-validation-result-publish-failed",
                "Escrow validation result NATS publish failed.",
                json!({ "subject": state.result_subject, "error": error.to_string() }),
            )
            .await;
        }
    }
}

async fn publish_escrow_event(state: &AppState, event_type: &str, request_id: &str, ok: bool) {
    let Some(nats) = &state.nats else {
        return;
    };
    let payload = json!({
        "type": event_type,
        "source": SERVICE_NAME,
        "requestId": request_id,
        "ok": ok,
        "chain": "solana",
        "schemaVersion": SCHEMA_VERSION,
        "atMs": now_ms(),
    });
    match nats
        .publish(state.event_subject.clone(), payload.to_string().into())
        .await
    {
        Ok(()) => {
            state
                .metrics
                .nats_events_published_total
                .fetch_add(1, Ordering::Relaxed);
        }
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            state
                .metrics
                .nats_publish_errors_total
                .fetch_add(1, Ordering::Relaxed);
            log_warn(
                "escrow-event-publish-failed",
                "Escrow lifecycle event NATS publish failed.",
                json!({
                    "subject": state.event_subject,
                    "eventType": event_type,
                    "requestId": request_id,
                    "error": error.to_string(),
                }),
            );
        }
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
                    "escrow-critical-event-publish-failed",
                    "Escrow service critical event NATS publish failed.",
                    json!({
                        "subject": state.critical_event_subject,
                        "eventName": event_name,
                        "error": error.to_string(),
                    }),
                );
            }
        },
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            log_error(
                "escrow-critical-event-serialize-failed",
                "Escrow service critical event payload serialization failed.",
                json!({
                    "eventName": event_name,
                    "error": error.to_string(),
                }),
            );
        }
    }
}

async fn run_nats_loop(state: AppState) {
    let Some(nats) = state.nats.clone() else {
        log_info(
            "escrow-nats-loop-disabled",
            "Escrow validation NATS loop is disabled because NATS_URL is not configured.",
            json!({}),
        );
        return;
    };
    log_info(
        "escrow-nats-loop-starting",
        "Escrow validation NATS loop is starting.",
        json!({
            "subject": state.validate_subject,
            "queueGroup": DEFAULT_QUEUE_GROUP,
            "resultSubject": state.result_subject,
            "eventSubject": state.event_subject,
            "criticalEventSubject": state.critical_event_subject,
        }),
    );
    let mut subscription = match nats
        .queue_subscribe(
            state.validate_subject.clone(),
            DEFAULT_QUEUE_GROUP.to_string(),
        )
        .await
    {
        Ok(subscription) => subscription,
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            publish_runtime_critical_event(
                &state,
                "escrow-nats-subscribe-failed",
                "Escrow service could not subscribe to validation requests.",
                json!({ "error": error.to_string() }),
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
                "escrow-nats-payload-too-large",
                "Escrow service rejected an oversized NATS validation request.",
                json!({
                    "payloadBytes": payload.len(),
                    "maxPayloadBytes": MAX_NATS_PAYLOAD_BYTES,
                }),
            )
            .await;
            continue;
        }
        match serde_json::from_slice::<EscrowIntentRequest>(&payload) {
            Ok(request) => {
                state
                    .metrics
                    .validations_total
                    .fetch_add(1, Ordering::Relaxed);
                let request_id = request_id(request.request_id.as_ref(), "escrow-validation");
                let result = match validate_escrow_intent(&request, &state.default_cluster) {
                    Ok(response) => {
                        json!({
                            "messageKind": "solana.escrow.validation.result",
                            "source": SERVICE_NAME,
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
                            "messageKind": "solana.escrow.validation.result",
                            "source": SERVICE_NAME,
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
                publish_validation_result(&state, result).await;
                publish_escrow_event(&state, "solana.escrow.validation", &request_id, ok).await;
            }
            Err(error) => {
                state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                state
                    .metrics
                    .nats_payload_rejected_total
                    .fetch_add(1, Ordering::Relaxed);
                publish_runtime_critical_event(
                    &state,
                    "escrow-nats-payload-invalid",
                    "Escrow service rejected an invalid NATS validation request.",
                    json!({ "error": error.to_string() }),
                )
                .await;
            }
        }
    }
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        log_error(
            "escrow-shutdown-signal-failed",
            "Escrow service failed while waiting for Ctrl-C.",
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
    let port = env_value("PORT", "8115");
    let configured_cluster = env_value("SOLANA_CLUSTER", "devnet");
    let default_cluster =
        normalize_cluster(Some(&configured_cluster), "devnet").map_err(config_error)?;
    let allow_private_rpc = env_bool("SOLANA_ALLOW_PRIVATE_RPC", false);
    let solana_rpc_url = validate_solana_rpc_url(
        &env_value("SOLANA_RPC_URL", "https://api.devnet.solana.com"),
        allow_private_rpc,
    )
    .map_err(config_error)?;
    let settlement_enabled = env_bool("SOLANA_SETTLEMENT_ENABLED", false);
    let settlement_auth_secret = env_secret("ESCROW_SETTLEMENT_AUTH_SECRET");
    if settlement_enabled && settlement_auth_secret.is_none() {
        return Err(config_error(
            "SOLANA_SETTLEMENT_ENABLED=true requires ESCROW_SETTLEMENT_AUTH_SECRET",
        )
        .into());
    }
    let allow_skip_preflight = env_bool("SOLANA_ALLOW_SKIP_PREFLIGHT", false);
    let rpc_timeout_seconds = env_u64("SOLANA_RPC_TIMEOUT_SECONDS", 20);
    let validate_subject = env_value("ESCROW_VALIDATE_SUBJECT", DEFAULT_VALIDATE_SUBJECT);
    let result_subject = env_value("ESCROW_RESULT_SUBJECT", DEFAULT_RESULT_SUBJECT);
    let event_subject = env_value("ESCROW_EVENT_SUBJECT", RUNTIME_EVENTS_SUBJECT);
    let critical_event_subject = env_value(
        "NATS_CRITICAL_EVENT_SUBJECT",
        RUNTIME_CRITICAL_EVENTS_SUBJECT,
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
                    "escrow-nats-connect-failed",
                    "Escrow service failed to connect to NATS.",
                    json!({ "url": url, "error": error.to_string() }),
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
        settlement_enabled,
        settlement_auth_secret,
        allow_skip_preflight,
        nats,
        validate_subject,
        result_subject,
        event_subject,
        critical_event_subject,
        metrics: Arc::new(Metrics::default()),
    };
    log_info(
        "escrow-service-starting",
        "Escrow service runtime configuration loaded.",
        json!({
            "cluster": state.default_cluster,
            "settlementEnabled": state.settlement_enabled,
            "skipPreflightAllowed": state.allow_skip_preflight,
            "validateSubject": state.validate_subject,
            "resultSubject": state.result_subject,
            "eventSubject": state.event_subject,
            "criticalEventSubject": state.critical_event_subject,
            "natsEnabled": state.nats.is_some(),
        }),
    );
    if state.nats.is_some() {
        tokio::spawn(run_nats_loop(state.clone()));
    }
    let app = Router::new()
        .route("/", get(home))
        .route("/healthz", get(healthz))
        .route("/docs/api", get(api_docs_html))
        .route("/api/docs", get(api_docs_html))
        .route("/api/docs.json", get(api_docs_json))
        .route("/metrics", get(metrics))
        .route("/status", get(status_http))
        .route("/types", get(types_http))
        .route("/schema", get(schema_http))
        .route("/example", get(example_http))
        .route("/validate", post(validate_http))
        .route("/simulate-settlement", post(simulate_settlement_http))
        .route("/settle", post(settle_http))
        .layer(DefaultBodyLimit::max(MAX_HTTP_BODY_BYTES))
        .with_state(state)
        .merge(dd_runtime_config_client::router());
    tokio::spawn(dd_runtime_config_client::register_with_control_plane());
    let address: SocketAddr = format!("{host}:{port}").parse()?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    log_info(
        "escrow-service-listening",
        "Escrow service HTTP listener is ready.",
        json!({ "address": address.to_string() }),
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_request() -> EscrowIntentRequest {
        example_request()
    }

    fn sample_state() -> AppState {
        AppState {
            rpc_client: reqwest::Client::new(),
            solana_rpc_url: "https://api.devnet.solana.com".to_string(),
            default_cluster: "devnet".to_string(),
            settlement_enabled: true,
            settlement_auth_secret: Some("secret".to_string()),
            allow_skip_preflight: false,
            nats: None,
            validate_subject: DEFAULT_VALIDATE_SUBJECT.to_string(),
            result_subject: DEFAULT_RESULT_SUBJECT.to_string(),
            event_subject: RUNTIME_EVENTS_SUBJECT.to_string(),
            critical_event_subject: RUNTIME_CRITICAL_EVENTS_SUBJECT.to_string(),
            metrics: Arc::new(Metrics::default()),
        }
    }

    #[test]
    fn catalog_has_ten_escrow_kinds() {
        let catalog = kind_catalog();
        assert_eq!(catalog.len(), 10);
        assert!(catalog
            .iter()
            .any(|entry| entry.kind == "marketplace-order"));
        assert!(catalog.iter().any(|entry| entry.kind == "group-buy"));
    }

    #[test]
    fn marketplace_order_validates() {
        let request = sample_request();
        let response =
            validate_escrow_intent(&request, "devnet").expect("sample escrow should validate");
        assert_eq!(response.kind, EscrowKind::MarketplaceOrder);
        assert!(response.on_chain_settlement_ready);
        assert_eq!(response.party_count, 2);
    }

    #[test]
    fn invalid_pubkey_is_rejected() {
        let mut request = sample_request();
        request.parties[0].pubkey = "not-a-solana-key".to_string();
        let errors = validate_escrow_intent(&request, "devnet").expect_err("must reject pubkey");
        assert!(errors.iter().any(|error| error.contains("valid base58")));
    }

    #[test]
    fn group_buy_requires_two_contributors() {
        let mut request = sample_request();
        request.kind = EscrowKind::GroupBuy;
        request.parties[0].role = PartyRole::Contributor;
        let errors = validate_escrow_intent(&request, "devnet").expect_err("must reject group-buy");
        assert!(errors
            .iter()
            .any(|error| error.contains("at least two contributor")));
    }

    #[test]
    fn settlement_action_must_match_kind() {
        let request = EscrowSettlementRequest {
            schema_version: SCHEMA_VERSION.to_string(),
            request_id: Some("settle-demo".to_string()),
            cluster: Some("devnet".to_string()),
            kind: EscrowKind::MarketplaceOrder,
            escrow_id: "order.demo.001".to_string(),
            action: SettlementAction::PartialRelease,
            transaction: general_purpose::STANDARD.encode([1_u8, 2, 3]),
            encoding: Some("base64".to_string()),
            commitment: None,
            skip_preflight: None,
            max_retries: None,
            min_context_slot: None,
            intent: None,
        };
        let errors =
            validate_settlement_request(&request, "devnet", false).expect_err("must reject action");
        assert!(errors.iter().any(|error| error.contains("does not allow")));
    }

    #[test]
    fn settlement_auth_requires_matching_header() {
        let state = sample_state();
        let mut headers = HeaderMap::new();
        assert!(authorize_settlement(&headers, &state).is_err());
        headers.insert(SETTLEMENT_AUTH_HEADER, "secret".parse().unwrap());
        assert!(authorize_settlement(&headers, &state).is_ok());
        headers.insert(SETTLEMENT_AUTH_HEADER, "wrong".parse().unwrap());
        assert!(authorize_settlement(&headers, &state).is_err());
    }

    #[test]
    fn private_rpc_url_is_rejected_by_default() {
        let error = validate_solana_rpc_url("http://127.0.0.1:8899", false)
            .expect_err("private HTTP RPC must be blocked");
        assert!(error.contains("https"));
        assert!(validate_solana_rpc_url("http://127.0.0.1:8899", true).is_ok());
    }

    #[test]
    fn signed_transaction_rejects_oversized_payload() {
        let encoded =
            general_purpose::STANDARD.encode(vec![7_u8; MAX_SIGNED_TRANSACTION_BYTES + 1]);
        let error = validate_signed_transaction(&encoded, "base64")
            .expect_err("must reject oversized transaction");
        assert!(error.contains("transaction must be at most"));
    }

    #[test]
    fn metrics_include_core_counters() {
        let state = sample_state();
        state
            .metrics
            .settlements_total
            .fetch_add(1, Ordering::Relaxed);
        let body = metrics_body(&state);
        assert!(body.contains("dd_escrow_rs_info{cluster=\"devnet\""));
        assert!(body.contains("dd_escrow_rs_settlements_total 1"));
    }
}
