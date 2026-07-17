//! Purpose-built Solana program, escrow, chain, and verification APIs.
//!
//! These routes are stateless and safe across replicas. They expose useful
//! chain facts, verify escrow-account invariants, and delegate source-level
//! program proofs to `dd-formal-methods-server`. Signing remains external.

use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose, Engine as _};
use reqwest::{Client, Url};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::{json_response, normalize_commitment_or_default, solana_rpc, AppState};

const MAX_FORMAL_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_INLINE_SOURCE_BYTES: usize = 256 * 1024;
const MAX_GITHUB_PATHS: usize = 64;
const MAX_SIGNATURE_HISTORY: u64 = 100;
const MAX_PRIORITY_FEE_ACCOUNTS: usize = 128;

const ESCROW_KINDS: [&str; 11] = [
    "marketplace-order",
    "milestone",
    "freelance-contract",
    "digital-delivery",
    "otc-trade",
    "rental-deposit",
    "bounty",
    "subscription-release",
    "group-buy",
    "dispute-resolution",
    "collab-show",
];
const ESCROW_ACTIONS: [&str; 8] = [
    "fund",
    "release",
    "refund",
    "partial-release",
    "split-release",
    "dispute-award",
    "expire",
    "cancel",
];

#[derive(Clone)]
pub(crate) struct SolanaFeatureState {
    formal: Option<Arc<FormalConfig>>,
    metrics: Arc<FeatureMetrics>,
}

struct FormalConfig {
    client: Client,
    url: String,
    auth_secret: String,
    allowed_github_orgs: HashSet<String>,
}

#[derive(Default)]
struct FeatureMetrics {
    program_inspections_total: AtomicU64,
    escrow_inspections_total: AtomicU64,
    chain_queries_total: AtomicU64,
    formal_requests_total: AtomicU64,
    formal_errors_total: AtomicU64,
    github_policy_rejections_total: AtomicU64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProgramInspectRequest {
    program_id: String,
    commitment: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EscrowInspectRequest {
    escrow_account: String,
    expected_owner_program_id: String,
    minimum_lamports: Option<u64>,
    #[serde(default = "default_true")]
    require_rent_exempt: bool,
    commitment: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SignatureHistoryRequest {
    address: String,
    limit: Option<u64>,
    before: Option<String>,
    until: Option<String>,
    commitment: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PriorityFeesRequest {
    #[serde(default)]
    writable_accounts: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProgramVerificationRequest {
    request_id: String,
    program_id: Option<String>,
    source: Option<String>,
    filename: Option<String>,
    repository: Option<String>,
    git_ref: Option<String>,
    paths: Option<Vec<String>>,
    heuristics: Option<bool>,
}

impl SolanaFeatureState {
    pub(crate) fn from_env(client: Client) -> Result<Self, String> {
        let formal_enabled = crate::env_bool("CONTRACT_FORMAL_METHODS_ENABLED", false);
        let metrics = Arc::new(FeatureMetrics::default());
        let formal = if formal_enabled {
            let url = validate_internal_service_url(&crate::env_value(
                "FORMAL_METHODS_URL",
                "http://dd-formal-methods-server.default.svc.cluster.local:8110",
            ))?;
            let auth_secret = crate::env_secret("FORMAL_METHODS_AUTH_SECRET").ok_or_else(|| {
                "CONTRACT_FORMAL_METHODS_ENABLED=true requires FORMAL_METHODS_AUTH_SECRET"
                    .to_string()
            })?;
            let allowed_github_orgs =
                crate::env_value("CONTRACT_FORMAL_METHODS_GITHUB_ORGS", "fiducia-cloud")
                    .split(',')
                    .map(|org| org.trim().to_ascii_lowercase())
                    .filter(|org| !org.is_empty())
                    .collect::<HashSet<_>>();
            if allowed_github_orgs.is_empty() {
                return Err(
                    "CONTRACT_FORMAL_METHODS_GITHUB_ORGS must contain at least one organization"
                        .to_string(),
                );
            }
            Some(Arc::new(FormalConfig {
                client,
                url,
                auth_secret,
                allowed_github_orgs,
            }))
        } else {
            None
        };
        Ok(Self { formal, metrics })
    }

    pub(crate) fn formal_enabled(&self) -> bool {
        self.formal.is_some()
    }

    pub(crate) fn allowed_github_orgs(&self) -> Vec<String> {
        let mut values: Vec<String> = self
            .formal
            .as_ref()
            .map(|config| config.allowed_github_orgs.iter().cloned().collect())
            .unwrap_or_default();
        values.sort();
        values
    }

    pub(crate) async fn readiness(&self) -> Result<(), String> {
        let Some(config) = self.formal.clone() else {
            return Ok(());
        };
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            config.client.get(format!("{}/healthz", config.url)).send(),
        )
        .await
        .map_err(|_| "formal-methods readiness timed out".to_string())?
        .map_err(|error| format!("formal-methods readiness failed: {error}"))?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(format!(
                "formal-methods readiness returned HTTP {}",
                response.status()
            ))
        }
    }

    #[cfg(test)]
    pub(crate) fn disabled_for_tests() -> Self {
        Self {
            formal: None,
            metrics: Arc::new(FeatureMetrics::default()),
        }
    }

    pub(crate) fn render_metrics(&self, out: &mut String) {
        let m = &self.metrics;
        out.push_str("# HELP dd_contract_service_solana_features_total Purpose-built Solana feature outcomes.\n# TYPE dd_contract_service_solana_features_total counter\n");
        for (feature, value) in [
            (
                "program_inspect",
                m.program_inspections_total.load(Ordering::Relaxed),
            ),
            (
                "escrow_inspect",
                m.escrow_inspections_total.load(Ordering::Relaxed),
            ),
            ("chain_query", m.chain_queries_total.load(Ordering::Relaxed)),
            (
                "formal_request",
                m.formal_requests_total.load(Ordering::Relaxed),
            ),
            (
                "formal_error",
                m.formal_errors_total.load(Ordering::Relaxed),
            ),
            (
                "github_policy_rejection",
                m.github_policy_rejections_total.load(Ordering::Relaxed),
            ),
        ] {
            out.push_str(&format!(
                "dd_contract_service_solana_features_total{{feature=\"{feature}\"}} {value}\n"
            ));
        }
    }
}

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/capabilities", get(capabilities_http))
        .route("/program/inspect", post(program_inspect_http))
        .route("/program/verify", post(program_verify_http))
        .route("/escrow/inspect", post(escrow_inspect_http))
        .route("/chain/signatures", post(signature_history_http))
        .route("/chain/priority-fees", post(priority_fees_http))
}

async fn capabilities_http(State(state): State<AppState>) -> axum::response::Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    json_response(
        StatusCode::OK,
        json!({
            "ok": true,
            "service": crate::SERVICE_NAME,
            "purpose": "keyless Solana policy, verification, simulation, settlement, dispute-resolution, and finality gateway",
            "cluster": state.default_cluster,
            "custody": "none; all transactions are signed by callers",
            "capabilities": {
                "contracts": ["validate-instruction-envelope", "simulate-signed-transaction", "send-gated-signed-transaction", "confirm-finality"],
                "smartContracts": ["inspect-deployed-program", "verify-inline-source", "verify-github-repository"],
                "escrow": {
                    "kinds": ESCROW_KINDS,
                    "actions": ESCROW_ACTIONS,
                    "routes": ["/simulate-settlement", "/settle", "/resolve", "/escrow/inspect"],
                    "domainValidator": "dd-escrow-rs",
                },
                "chain": ["account", "balance", "blockhash", "fees", "priority-fees", "rent-exemption", "signature-history", "transaction", "confirmation"],
            },
            "integrations": {
                "formalMethods": {
                    "enabled": state.solana_features.formal_enabled(),
                    "allowedGithubOrganizations": state.solana_features.allowed_github_orgs(),
                },
                "broadcastCoordination": {
                    "enabled": state.coordination.enabled(),
                    "required": state.coordination.required(),
                    "postgresAdvisoryLock": state.coordination.enabled(),
                    "fiduciaIdempotencyLease": state.coordination.enabled(),
                },
                "github": "https://github.com/fiducia-cloud",
            },
            "broadcast": {
                "rawSendEnabled": state.send_enabled,
                "settlementEnabled": state.settlement_enabled,
                "resolutionEnabled": state.resolution_enabled,
                "mainnetSecondGateEnabled": state.mainnet_settlement_enabled,
            },
        }),
    )
}

async fn program_inspect_http(
    State(state): State<AppState>,
    Json(request): Json<ProgramInspectRequest>,
) -> axum::response::Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(error) = crate::validate_pubkey(&request.program_id, "programId") {
        return error_response(StatusCode::BAD_REQUEST, &error);
    }
    let commitment = match durable_commitment(request.commitment.as_deref()) {
        Ok(value) => value,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, &error),
    };
    let result = match solana_rpc(
        &state,
        "getAccountInfo",
        json!([request.program_id, { "encoding": "base64", "commitment": commitment }]),
    )
    .await
    {
        Ok(value) => value,
        Err(error) => return error_response(StatusCode::BAD_GATEWAY, &error),
    };
    let Some(account) = result.get("value").filter(|value| !value.is_null()) else {
        return error_response(StatusCode::NOT_FOUND, "program account was not found");
    };
    state
        .solana_features
        .metrics
        .program_inspections_total
        .fetch_add(1, Ordering::Relaxed);
    let executable = account
        .get("executable")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    json_response(
        StatusCode::OK,
        json!({
            "ok": executable,
            "programId": request.program_id,
            "commitment": commitment,
            "executable": executable,
            "owner": account.get("owner"),
            "lamports": account.get("lamports"),
            "dataBytes": account_data_bytes(account),
            "rentEpoch": account.get("rentEpoch"),
            "context": result.get("context"),
            "check": if executable { "deployed-program" } else { "account-is-not-executable" },
        }),
    )
}

async fn escrow_inspect_http(
    State(state): State<AppState>,
    Json(request): Json<EscrowInspectRequest>,
) -> axum::response::Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    for (value, label) in [
        (&request.escrow_account, "escrowAccount"),
        (&request.expected_owner_program_id, "expectedOwnerProgramId"),
    ] {
        if let Err(error) = crate::validate_pubkey(value, label) {
            return error_response(StatusCode::BAD_REQUEST, &error);
        }
    }
    let commitment = match durable_commitment(request.commitment.as_deref()) {
        Ok(value) => value,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, &error),
    };
    let result = match solana_rpc(
        &state,
        "getAccountInfo",
        json!([request.escrow_account, { "encoding": "base64", "commitment": commitment }]),
    )
    .await
    {
        Ok(value) => value,
        Err(error) => return error_response(StatusCode::BAD_GATEWAY, &error),
    };
    let Some(account) = result.get("value").filter(|value| !value.is_null()) else {
        return error_response(StatusCode::NOT_FOUND, "escrow account was not found");
    };
    let owner = account
        .get("owner")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let lamports = account.get("lamports").and_then(Value::as_u64).unwrap_or(0);
    let data_bytes = account_data_bytes(account);
    let minimum_balance = if request.require_rent_exempt {
        match solana_rpc(
            &state,
            "getMinimumBalanceForRentExemption",
            json!([data_bytes, { "commitment": commitment }]),
        )
        .await
        {
            Ok(value) => value.as_u64(),
            Err(error) => return error_response(StatusCode::BAD_GATEWAY, &error),
        }
    } else {
        None
    };
    let owner_matches = owner == request.expected_owner_program_id;
    let non_executable = !account
        .get("executable")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let balance_sufficient = request
        .minimum_lamports
        .is_none_or(|minimum| lamports >= minimum);
    let rent_exempt = minimum_balance.is_none_or(|minimum| lamports >= minimum);
    let ready = owner_matches && non_executable && balance_sufficient && rent_exempt;
    state
        .solana_features
        .metrics
        .escrow_inspections_total
        .fetch_add(1, Ordering::Relaxed);
    json_response(
        StatusCode::OK,
        json!({
            "ok": ready,
            "ready": ready,
            "escrowAccount": request.escrow_account,
            "expectedOwnerProgramId": request.expected_owner_program_id,
            "owner": owner,
            "lamports": lamports,
            "dataBytes": data_bytes,
            "minimumRentExemptLamports": minimum_balance,
            "checks": {
                "ownerMatches": owner_matches,
                "nonExecutable": non_executable,
                "minimumBalanceSatisfied": balance_sufficient,
                "rentExempt": rent_exempt,
            },
            "note": "account invariants only; dd-escrow-rs validates parties, terms, assets, release modes, and settlement policy",
        }),
    )
}

async fn signature_history_http(
    State(state): State<AppState>,
    Json(request): Json<SignatureHistoryRequest>,
) -> axum::response::Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if let Err(error) = crate::validate_pubkey(&request.address, "address") {
        return error_response(StatusCode::BAD_REQUEST, &error);
    }
    let commitment = match durable_commitment(request.commitment.as_deref()) {
        Ok(value) => value,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, &error),
    };
    for (signature, label) in [
        (request.before.as_deref(), "before"),
        (request.until.as_deref(), "until"),
    ] {
        if let Some(signature) = signature {
            if let Err(error) = crate::validate_signature(signature, label) {
                return error_response(StatusCode::BAD_REQUEST, &error);
            }
        }
    }
    let limit = request.limit.unwrap_or(25).clamp(1, MAX_SIGNATURE_HISTORY);
    let mut options = json!({ "limit": limit, "commitment": commitment });
    if let Some(before) = request.before {
        options["before"] = json!(before);
    }
    if let Some(until) = request.until {
        options["until"] = json!(until);
    }
    match solana_rpc(
        &state,
        "getSignaturesForAddress",
        json!([request.address, options]),
    )
    .await
    {
        Ok(signatures) => {
            state
                .solana_features
                .metrics
                .chain_queries_total
                .fetch_add(1, Ordering::Relaxed);
            let count = signatures.as_array().map_or(0, Vec::len);
            json_response(
                StatusCode::OK,
                json!({ "ok": true, "address": request.address, "commitment": commitment, "count": count, "signatures": signatures }),
            )
        }
        Err(error) => error_response(StatusCode::BAD_GATEWAY, &error),
    }
}

async fn priority_fees_http(
    State(state): State<AppState>,
    Json(request): Json<PriorityFeesRequest>,
) -> axum::response::Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    if request.writable_accounts.len() > MAX_PRIORITY_FEE_ACCOUNTS {
        return error_response(
            StatusCode::BAD_REQUEST,
            "writableAccounts accepts at most 128 public keys",
        );
    }
    for account in &request.writable_accounts {
        if let Err(error) = crate::validate_pubkey(account, "writableAccounts[]") {
            return error_response(StatusCode::BAD_REQUEST, &error);
        }
    }
    let result = match solana_rpc(
        &state,
        "getRecentPrioritizationFees",
        json!([request.writable_accounts]),
    )
    .await
    {
        Ok(value) => value,
        Err(error) => return error_response(StatusCode::BAD_GATEWAY, &error),
    };
    let mut fees = result
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("prioritizationFee").and_then(Value::as_u64))
        .collect::<Vec<_>>();
    fees.sort_unstable();
    let percentile = |numerator: usize| -> u64 {
        if fees.is_empty() {
            0
        } else {
            fees[((fees.len() - 1) * numerator) / 100]
        }
    };
    state
        .solana_features
        .metrics
        .chain_queries_total
        .fetch_add(1, Ordering::Relaxed);
    json_response(
        StatusCode::OK,
        json!({
            "ok": true,
            "sampleCount": fees.len(),
            "microLamportsPerComputeUnit": {
                "minimum": fees.first().copied().unwrap_or(0),
                "median": percentile(50),
                "p90": percentile(90),
                "maximum": fees.last().copied().unwrap_or(0),
            },
            "samples": result,
        }),
    )
}

async fn program_verify_http(
    State(state): State<AppState>,
    Json(request): Json<ProgramVerificationRequest>,
) -> axum::response::Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let Some(config) = state.solana_features.formal.clone() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "formal verification is disabled",
        );
    };
    let mut request_id_errors = Vec::new();
    crate::validate_request_id(Some(&request.request_id), &mut request_id_errors);
    if !request_id_errors.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, &request_id_errors.join("; "));
    }
    if let Some(program_id) = request.program_id.as_deref() {
        if let Err(error) = crate::validate_pubkey(program_id, "programId") {
            return error_response(StatusCode::BAD_REQUEST, &error);
        }
    }
    if request.source.is_some() == request.repository.is_some() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "set exactly one of source or repository",
        );
    }

    let (path, mode, payload, provenance) = if let Some(source) = request.source {
        if source.is_empty() || source.len() > MAX_INLINE_SOURCE_BYTES {
            return error_response(StatusCode::BAD_REQUEST, "source must be 1..=262144 bytes");
        }
        let filename = request.filename.unwrap_or_else(|| "program.rs".to_string());
        if !safe_relative_path(&filename) {
            return error_response(
                StatusCode::BAD_REQUEST,
                "filename must be a safe relative path",
            );
        }
        let source_digest = hex::encode(Sha256::digest(source.as_bytes()));
        (
            "/validate",
            "inline",
            json!({
                "schemaVersion": "formal-methods.v1",
                "source": source,
                "filename": filename.clone(),
                "heuristics": request.heuristics.unwrap_or(true),
            }),
            json!({ "sourceDigest": source_digest, "filename": filename }),
        )
    } else {
        let repository = request.repository.expect("exclusive option checked");
        let canonical = match validate_github_repository(&repository, &config.allowed_github_orgs) {
            Ok(value) => value,
            Err(error) => {
                state
                    .solana_features
                    .metrics
                    .github_policy_rejections_total
                    .fetch_add(1, Ordering::Relaxed);
                return error_response(StatusCode::BAD_REQUEST, &error);
            }
        };
        let paths = request.paths.unwrap_or_default();
        if paths.len() > MAX_GITHUB_PATHS || paths.iter().any(|path| !safe_relative_path(path)) {
            return error_response(
                StatusCode::BAD_REQUEST,
                "paths must contain at most 64 safe relative paths",
            );
        }
        if request
            .git_ref
            .as_deref()
            .is_some_and(|git_ref| !safe_git_ref(git_ref))
        {
            return error_response(StatusCode::BAD_REQUEST, "gitRef has invalid shape");
        }
        (
            "/analyses",
            "github",
            json!({
                "schemaVersion": "formal-methods.v1",
                "repoUrl": canonical.clone(),
                "gitRef": request.git_ref.clone(),
                "paths": paths,
                "languages": ["rust"],
                "heuristics": request.heuristics.unwrap_or(true),
            }),
            json!({ "repository": canonical, "gitRef": request.git_ref.clone() }),
        )
    };

    state
        .solana_features
        .metrics
        .formal_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let formal = match formal_post(&config, path, payload).await {
        Ok(value) => value,
        Err(error) => {
            state
                .solana_features
                .metrics
                .formal_errors_total
                .fetch_add(1, Ordering::Relaxed);
            return error_response(StatusCode::BAD_GATEWAY, &error);
        }
    };
    let verification_digest = hex::encode(Sha256::digest(formal.to_string().as_bytes()));
    json_response(
        StatusCode::OK,
        json!({
            "ok": true,
            "requestId": request.request_id,
            "programId": request.program_id,
            "mode": mode,
            "provenance": provenance,
            "verificationDigest": verification_digest,
            "formalMethods": formal,
        }),
    )
}

async fn formal_post(config: &FormalConfig, path: &str, payload: Value) -> Result<Value, String> {
    let response = config
        .client
        .post(format!("{}{path}", config.url))
        .header("x-server-auth", &config.auth_secret)
        .json(&payload)
        .send()
        .await
        .map_err(|error| format!("formal-methods request failed: {error}"))?;
    let status = response.status();
    if response.content_length().unwrap_or(0) > MAX_FORMAL_RESPONSE_BYTES {
        return Err("formal-methods response exceeded size limit".to_string());
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("formal-methods response failed: {error}"))?;
    if bytes.len() as u64 > MAX_FORMAL_RESPONSE_BYTES {
        return Err("formal-methods response exceeded size limit".to_string());
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("formal-methods response was not JSON: {error}"))?;
    if !status.is_success() {
        return Err(format!("formal-methods returned HTTP {status}"));
    }
    Ok(value)
}

fn durable_commitment(input: Option<&str>) -> Result<String, String> {
    let commitment = normalize_commitment_or_default(input)?;
    if commitment == "processed" {
        return Err(
            "processed commitment is not durable enough for program or escrow inspection"
                .to_string(),
        );
    }
    Ok(commitment)
}

fn account_data_bytes(account: &Value) -> u64 {
    if let Some(space) = account.get("space").and_then(Value::as_u64) {
        return space;
    }
    account
        .get("data")
        .and_then(Value::as_array)
        .and_then(|data| data.first())
        .and_then(Value::as_str)
        .and_then(|encoded| general_purpose::STANDARD.decode(encoded).ok())
        .map_or(0, |bytes| bytes.len() as u64)
}

fn validate_internal_service_url(raw: &str) -> Result<String, String> {
    let url = Url::parse(raw).map_err(|error| format!("service URL is invalid: {error}"))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err("service URL must not include credentials".to_string());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "service URL must include a host".to_string())?;
    let internal_http = url.scheme() == "http"
        && (host == "localhost" || host == "127.0.0.1" || host.ends_with(".svc.cluster.local"));
    if url.scheme() != "https" && !internal_http {
        return Err("service URL must use https or in-cluster http".to_string());
    }
    Ok(raw.trim_end_matches('/').to_string())
}

fn validate_github_repository(raw: &str, allowed_orgs: &HashSet<String>) -> Result<String, String> {
    let url = Url::parse(raw).map_err(|error| format!("repository URL is invalid: {error}"))?;
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("repository must be a credential-free https://github.com URL".to_string());
    }
    let segments = url
        .path_segments()
        .map(|segments| {
            segments
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if segments.len() != 2 {
        return Err("repository URL must have exactly owner/repository".to_string());
    }
    let owner = segments[0].to_ascii_lowercase();
    let repo = segments[1].trim_end_matches(".git");
    if !allowed_orgs.contains(&owner)
        || repo.is_empty()
        || !repo
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(
            "repository is outside the configured GitHub organization allowlist".to_string(),
        );
    }
    Ok(format!("https://github.com/{owner}/{repo}.git"))
}

fn safe_relative_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && !value.starts_with('/')
        && !value.contains('\\')
        && !value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.'))
}

fn safe_git_ref(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && !value.starts_with('-')
        && !value.contains("..")
        && !value.contains("@{")
        && !value.ends_with('.')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.'))
}

fn default_true() -> bool {
    true
}

fn error_response(status: StatusCode, error: &str) -> axum::response::Response {
    json_response(status, json!({ "ok": false, "error": error }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn orgs() -> HashSet<String> {
        ["fiducia-cloud".to_string()].into_iter().collect()
    }

    #[test]
    fn github_repository_policy_is_org_scoped() {
        assert_eq!(
            validate_github_repository("https://github.com/fiducia-cloud/escrow.rs", &orgs())
                .unwrap(),
            "https://github.com/fiducia-cloud/escrow.rs.git"
        );
        assert!(validate_github_repository("https://github.com/other/escrow.rs", &orgs()).is_err());
        assert!(
            validate_github_repository("http://github.com/fiducia-cloud/escrow.rs", &orgs())
                .is_err()
        );
    }

    #[test]
    fn source_paths_and_refs_reject_traversal() {
        assert!(safe_relative_path("programs/escrow/src/lib.rs"));
        assert!(!safe_relative_path("../secrets"));
        assert!(safe_git_ref("refs/heads/main"));
        assert!(!safe_git_ref("main..evil"));
    }

    #[test]
    fn account_space_prefers_rpc_space_field() {
        assert_eq!(account_data_bytes(&json!({ "space": 165 })), 165);
    }
}
