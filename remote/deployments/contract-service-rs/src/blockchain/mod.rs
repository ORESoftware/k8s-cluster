//! Blockchain feature suite for `dd-contract-service`.
//!
//! Ten subsystems — chain core, wallet management, smart-contract executor,
//! transaction relayer, multi-signature coordinator, indexing, MEV/arbitrage
//! monitoring, NFT/media storage, staking, and a cross-chain bridge coordinator
//! — wired into the existing keyless Solana gateway as additional modules.
//!
//! Design contract (do not regress):
//! * **Keyless, custody-ready.** No private keys are stored and nothing here
//!   signs. Anything that would sign goes through [`SignerBackend`], whose only
//!   variant today is `External` ("client must sign"). Execute/relay paths accept
//!   an already client-signed raw transaction.
//! * **Off by default.** Every feature is gated by a `*_ENABLED` flag defaulting
//!   to `false`; any broadcast/execute path additionally requires the shared
//!   `CONTRACT_BLOCKCHAIN_AUTH_SECRET` and, against mainnet, an explicit second
//!   gate (`CONTRACT_BLOCKCHAIN_MAINNET_ENABLED`).
//! * **Bounded, ephemeral state.** Registries/indexes are in-memory and bounded
//!   (no Postgres DDL from Rust, per the repo contract).

mod bridge;
mod evm;
mod executor;
mod indexer;
mod mev;
mod multisig;
mod nft;
mod relayer;
mod staking;
mod wallet;

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axum::{
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};

use crate::AppState;

/// Header carrying the shared blockchain execute/broadcast auth secret.
pub(super) const BLOCKCHAIN_AUTH_HEADER: &str = "x-contract-blockchain-auth";

// Bounded-store caps — keep memory predictable for an ephemeral scaffold.
const MAX_WALLETS: usize = 4_096;
const MAX_PROPOSALS: usize = 2_048;
const MAX_RELAYS: usize = 4_096;
const MAX_INDEX_EVENTS: usize = 8_192;
const MAX_WATCHES: usize = 256;
const MAX_BRIDGES: usize = 2_048;
const MAX_MEDIA_OBJECTS: usize = 1_024;
const MAX_MEDIA_BYTES: usize = 256 * 1024;
pub(super) const MAX_BLOCKCHAIN_LABEL: usize = 128;

/// Pluggable signing seam. Today only `External`: the service never holds a key,
/// so signing is always the caller's responsibility. A future `Kms` variant is
/// the documented extension point for custodial signing.
#[derive(Clone, Copy)]
pub(super) enum SignerBackend {
    External,
}

impl SignerBackend {
    fn label(&self) -> &'static str {
        match self {
            SignerBackend::External => "external",
        }
    }
}

/// Pluggable content store for NFT media. Today only a bounded in-memory map; a
/// future `Ipfs`/`S3` variant is the documented extension point.
pub(super) enum MediaStore {
    InMemory(HashMap<String, MediaObject>),
}

pub(super) struct MediaObject {
    pub content_type: String,
    pub bytes: Vec<u8>,
    pub created_ms: u128,
}

/// One configured chain in the registry.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ChainKind {
    Solana,
    Evm,
}

/// Static configuration for the whole suite, resolved once at startup.
pub(super) struct BlockchainConfig {
    // Retained for parity with the parent service config; Solana RPC calls go
    // through the shared `AppState` path rather than this copy.
    #[allow(dead_code)]
    pub solana_rpc_url: String,
    pub solana_cluster: String,
    pub evm_rpc_url: Option<String>,
    pub evm_chain_id: Option<u64>,
    pub evm_network: String,

    pub wallet_enabled: bool,
    pub executor_enabled: bool,
    pub executor_execute_enabled: bool,
    pub relayer_enabled: bool,
    pub relayer_broadcast_enabled: bool,
    pub multisig_enabled: bool,
    pub indexer_enabled: bool,
    pub mev_enabled: bool,
    pub nft_enabled: bool,
    pub staking_enabled: bool,
    pub staking_execute_enabled: bool,
    pub bridge_enabled: bool,
    pub bridge_broadcast_enabled: bool,

    pub mainnet_broadcast_enabled: bool,
    pub execute_auth_secret: Option<String>,

    pub index_events_subject: String,
    pub mev_alerts_subject: String,
    pub bridge_attestations_subject: String,
}

impl BlockchainConfig {
    fn from_env(solana_rpc_url: &str, solana_cluster: &str) -> Result<Self, String> {
        let evm_allow_private = crate::env_bool("EVM_ALLOW_PRIVATE_RPC", false);
        let evm_rpc_url = match crate::env_secret("EVM_RPC_URL") {
            Some(raw) => Some(
                crate::validate_solana_rpc_url(&raw, evm_allow_private)
                    .map_err(|error| error.replace("SOLANA_RPC_URL", "EVM_RPC_URL"))?,
            ),
            None => None,
        };
        let evm_chain_id = match crate::env_u64("EVM_CHAIN_ID", 0) {
            0 => None,
            value => Some(value),
        };
        let evm_network = crate::env_value("EVM_NETWORK", "sepolia");

        let executor_execute_enabled = crate::env_bool("BLOCKCHAIN_EXECUTOR_EXECUTE_ENABLED", false);
        let relayer_broadcast_enabled =
            crate::env_bool("BLOCKCHAIN_RELAYER_BROADCAST_ENABLED", false);
        let staking_execute_enabled = crate::env_bool("BLOCKCHAIN_STAKING_EXECUTE_ENABLED", false);
        let bridge_broadcast_enabled = crate::env_bool("BLOCKCHAIN_BRIDGE_BROADCAST_ENABLED", false);
        let mainnet_broadcast_enabled =
            crate::env_bool("CONTRACT_BLOCKCHAIN_MAINNET_ENABLED", false);
        let execute_auth_secret = crate::env_secret("CONTRACT_BLOCKCHAIN_AUTH_SECRET");

        let any_broadcast = executor_execute_enabled
            || relayer_broadcast_enabled
            || staking_execute_enabled
            || bridge_broadcast_enabled;

        // Any gated execute/broadcast capability requires the shared auth secret.
        if any_broadcast && execute_auth_secret.is_none() {
            return Err(
                "enabling a blockchain execute/broadcast capability requires CONTRACT_BLOCKCHAIN_AUTH_SECRET"
                    .to_string(),
            );
        }

        // Mainnet second gate: a single feature flag cannot move real funds.
        let targets_mainnet = solana_cluster == "mainnet-beta"
            || evm::is_evm_mainnet(evm_chain_id, &evm_network);
        if any_broadcast && targets_mainnet && !mainnet_broadcast_enabled {
            return Err(
                "blockchain execute/broadcast against a mainnet target requires CONTRACT_BLOCKCHAIN_MAINNET_ENABLED=true"
                    .to_string(),
            );
        }

        Ok(Self {
            solana_rpc_url: solana_rpc_url.to_string(),
            solana_cluster: solana_cluster.to_string(),
            evm_rpc_url,
            evm_chain_id,
            evm_network,
            wallet_enabled: crate::env_bool("BLOCKCHAIN_WALLET_ENABLED", false),
            executor_enabled: crate::env_bool("BLOCKCHAIN_EXECUTOR_ENABLED", false),
            executor_execute_enabled,
            relayer_enabled: crate::env_bool("BLOCKCHAIN_RELAYER_ENABLED", false),
            relayer_broadcast_enabled,
            multisig_enabled: crate::env_bool("BLOCKCHAIN_MULTISIG_ENABLED", false),
            indexer_enabled: crate::env_bool("BLOCKCHAIN_INDEXER_ENABLED", false),
            mev_enabled: crate::env_bool("BLOCKCHAIN_MEV_ENABLED", false),
            nft_enabled: crate::env_bool("BLOCKCHAIN_NFT_ENABLED", false),
            staking_enabled: crate::env_bool("BLOCKCHAIN_STAKING_ENABLED", false),
            staking_execute_enabled,
            bridge_enabled: crate::env_bool("BLOCKCHAIN_BRIDGE_ENABLED", false),
            bridge_broadcast_enabled,
            mainnet_broadcast_enabled,
            execute_auth_secret,
            index_events_subject: crate::env_value(
                "BLOCKCHAIN_INDEX_EVENTS_SUBJECT",
                dd_nats_subject_defs::BLOCKCHAIN_INDEX_EVENTS_SUBJECT,
            ),
            mev_alerts_subject: crate::env_value(
                "BLOCKCHAIN_MEV_ALERTS_SUBJECT",
                dd_nats_subject_defs::BLOCKCHAIN_MEV_ALERTS_SUBJECT,
            ),
            bridge_attestations_subject: crate::env_value(
                "BLOCKCHAIN_BRIDGE_ATTESTATIONS_SUBJECT",
                dd_nats_subject_defs::BLOCKCHAIN_BRIDGE_ATTESTATIONS_SUBJECT,
            ),
        })
    }

    pub(super) fn evm_configured(&self) -> bool {
        self.evm_rpc_url.is_some()
    }

    /// True when broadcasting/executing on `kind` would touch a mainnet network.
    /// Used by the startup gate; retained for per-request checks in later phases.
    #[allow(dead_code)]
    pub(super) fn targets_mainnet(&self, kind: ChainKind) -> bool {
        match kind {
            ChainKind::Solana => self.solana_cluster == "mainnet-beta",
            ChainKind::Evm => evm::is_evm_mainnet(self.evm_chain_id, &self.evm_network),
        }
    }
}

#[derive(Default)]
pub(super) struct BlockchainMetrics {
    pub requests_total: AtomicU64,
    pub rejected_disabled_total: AtomicU64,
    pub auth_failures_total: AtomicU64,
    pub broadcast_blocked_total: AtomicU64,
    pub evm_rpc_requests_total: AtomicU64,
    pub evm_rpc_errors_total: AtomicU64,
    pub wallets_registered_total: AtomicU64,
    pub executor_simulations_total: AtomicU64,
    pub relayer_submissions_total: AtomicU64,
    pub multisig_proposals_total: AtomicU64,
    pub multisig_approvals_total: AtomicU64,
    pub index_events_total: AtomicU64,
    pub mev_alerts_total: AtomicU64,
    pub nft_media_stored_total: AtomicU64,
    pub staking_validations_total: AtomicU64,
    pub bridge_transfers_total: AtomicU64,
    pub bridge_attestations_total: AtomicU64,
}

impl BlockchainMetrics {
    /// Appends the blockchain counters to the shared `/metrics` body.
    pub(super) fn render(&self, out: &mut String) {
        let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        let rows: [(&str, &str, u64); 17] = [
            ("dd_contract_service_blockchain_requests_total", "Blockchain feature HTTP requests handled.", load(&self.requests_total)),
            ("dd_contract_service_blockchain_rejected_disabled_total", "Blockchain requests rejected because the feature is disabled.", load(&self.rejected_disabled_total)),
            ("dd_contract_service_blockchain_auth_failures_total", "Blockchain execute/broadcast auth failures.", load(&self.auth_failures_total)),
            ("dd_contract_service_blockchain_broadcast_blocked_total", "Blockchain broadcasts blocked by a disabled gate.", load(&self.broadcast_blocked_total)),
            ("dd_contract_service_blockchain_evm_rpc_requests_total", "EVM JSON-RPC requests issued.", load(&self.evm_rpc_requests_total)),
            ("dd_contract_service_blockchain_evm_rpc_errors_total", "EVM JSON-RPC requests that errored.", load(&self.evm_rpc_errors_total)),
            ("dd_contract_service_blockchain_wallets_registered_total", "Watch-only wallets registered.", load(&self.wallets_registered_total)),
            ("dd_contract_service_blockchain_executor_simulations_total", "Smart-contract executor simulations.", load(&self.executor_simulations_total)),
            ("dd_contract_service_blockchain_relayer_submissions_total", "Transaction relayer submissions accepted.", load(&self.relayer_submissions_total)),
            ("dd_contract_service_blockchain_multisig_proposals_total", "Multisig proposals created.", load(&self.multisig_proposals_total)),
            ("dd_contract_service_blockchain_multisig_approvals_total", "Multisig approvals collected.", load(&self.multisig_approvals_total)),
            ("dd_contract_service_blockchain_index_events_total", "Index events recorded.", load(&self.index_events_total)),
            ("dd_contract_service_blockchain_mev_alerts_total", "MEV/arbitrage monitoring alerts emitted.", load(&self.mev_alerts_total)),
            ("dd_contract_service_blockchain_nft_media_stored_total", "NFT media objects stored.", load(&self.nft_media_stored_total)),
            ("dd_contract_service_blockchain_staking_validations_total", "Staking intents validated.", load(&self.staking_validations_total)),
            ("dd_contract_service_blockchain_bridge_transfers_total", "Cross-chain bridge transfer intents created.", load(&self.bridge_transfers_total)),
            ("dd_contract_service_blockchain_bridge_attestations_total", "Cross-chain bridge attestations verified.", load(&self.bridge_attestations_total)),
        ];
        for (name, help, value) in rows {
            crate::push_counter(out, name, help, value);
        }
    }
}

/// Owned, shareable state for the suite. Cheap to clone (`Arc` inner).
#[derive(Clone)]
pub(crate) struct BlockchainState {
    inner: Arc<Inner>,
}

pub(super) struct Inner {
    pub(super) config: BlockchainConfig,
    pub(super) metrics: BlockchainMetrics,
    pub(super) signer: SignerBackend,
    pub(super) rpc_client: reqwest::Client,
    pub(super) wallets: Mutex<HashMap<String, wallet::WalletRecord>>,
    pub(super) proposals: Mutex<HashMap<String, multisig::Proposal>>,
    pub(super) relays: Mutex<HashMap<String, relayer::RelayRecord>>,
    pub(super) watches: Mutex<Vec<indexer::WatchItem>>,
    pub(super) index_events: Mutex<VecDeque<indexer::IndexedEvent>>,
    pub(super) bridges: Mutex<HashMap<String, bridge::BridgeTransfer>>,
    pub(super) media: Mutex<MediaStore>,
}

impl BlockchainState {
    /// Builds the suite state from env, reusing the already-validated Solana RPC
    /// URL/cluster and the shared `reqwest` client from the parent service.
    pub(crate) fn from_env(
        rpc_client: reqwest::Client,
        solana_rpc_url: &str,
        solana_cluster: &str,
    ) -> Result<Self, String> {
        let config = BlockchainConfig::from_env(solana_rpc_url, solana_cluster)?;
        Ok(Self {
            inner: Arc::new(Inner {
                config,
                metrics: BlockchainMetrics::default(),
                signer: SignerBackend::External,
                rpc_client,
                wallets: Mutex::new(HashMap::new()),
                proposals: Mutex::new(HashMap::new()),
                relays: Mutex::new(HashMap::new()),
                watches: Mutex::new(Vec::new()),
                index_events: Mutex::new(VecDeque::new()),
                bridges: Mutex::new(HashMap::new()),
                media: Mutex::new(MediaStore::InMemory(HashMap::new())),
            }),
        })
    }

    pub(super) fn config(&self) -> &BlockchainConfig {
        &self.inner.config
    }

    pub(super) fn metrics(&self) -> &BlockchainMetrics {
        &self.inner.metrics
    }

    pub(super) fn inner(&self) -> &Inner {
        &self.inner
    }

    /// Snapshot of enabled-flags for logging at startup.
    pub(crate) fn startup_summary(&self) -> Value {
        let c = self.config();
        json!({
            "signer": self.inner.signer.label(),
            "evmConfigured": c.evm_configured(),
            "evmNetwork": c.evm_network,
            "evmChainId": c.evm_chain_id,
            "wallet": c.wallet_enabled,
            "executor": c.executor_enabled,
            "executorExecute": c.executor_execute_enabled,
            "relayer": c.relayer_enabled,
            "relayerBroadcast": c.relayer_broadcast_enabled,
            "multisig": c.multisig_enabled,
            "indexer": c.indexer_enabled,
            "mev": c.mev_enabled,
            "nft": c.nft_enabled,
            "staking": c.staking_enabled,
            "stakingExecute": c.staking_execute_enabled,
            "bridge": c.bridge_enabled,
            "bridgeBroadcast": c.bridge_broadcast_enabled,
            "mainnetBroadcast": c.mainnet_broadcast_enabled,
        })
    }

    /// Appends the blockchain Prometheus counters to the shared metrics body.
    pub(crate) fn render_metrics(&self, out: &mut String) {
        self.metrics().render(out);
    }

    pub(super) async fn evm_rpc(&self, method: &str, params: Value) -> Result<Value, String> {
        let Some(url) = &self.config().evm_rpc_url else {
            return Err("EVM RPC is not configured (set EVM_RPC_URL)".to_string());
        };
        self.metrics()
            .evm_rpc_requests_total
            .fetch_add(1, Ordering::Relaxed);
        let result = evm::evm_rpc(&self.inner.rpc_client, url, method, params).await;
        if result.is_err() {
            self.metrics()
                .evm_rpc_errors_total
                .fetch_add(1, Ordering::Relaxed);
        }
        result
    }
}

// ---- shared helpers used by every feature module ----------------------------

pub(super) fn json_ok(value: Value) -> Response {
    (StatusCode::OK, Json(value)).into_response()
}

pub(super) fn json_err(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "ok": false, "error": message }))).into_response()
}

/// Rejects a request with `503` when a feature flag is off, recording the metric.
pub(super) fn require_enabled(
    state: &BlockchainState,
    enabled: bool,
    flag: &str,
) -> Result<(), Response> {
    if enabled {
        return Ok(());
    }
    state
        .metrics()
        .rejected_disabled_total
        .fetch_add(1, Ordering::Relaxed);
    Err(json_err(
        StatusCode::SERVICE_UNAVAILABLE,
        &format!("feature disabled; set {flag}=true to enable"),
    ))
}

/// Constant-time check of the shared blockchain execute/broadcast auth header.
pub(super) fn authorize_execute(state: &BlockchainState, headers: &HeaderMap) -> Result<(), Response> {
    let Some(secret) = &state.config().execute_auth_secret else {
        return Err(json_err(
            StatusCode::SERVICE_UNAVAILABLE,
            "execute/broadcast is not configured with CONTRACT_BLOCKCHAIN_AUTH_SECRET",
        ));
    };
    let provided = headers
        .get(BLOCKCHAIN_AUTH_HEADER)
        .and_then(|value| value.to_str().ok());
    match provided {
        Some(value) if crate::sensitive_eq(value.trim(), secret) => Ok(()),
        _ => {
            state
                .metrics()
                .auth_failures_total
                .fetch_add(1, Ordering::Relaxed);
            Err(json_err(
                StatusCode::UNAUTHORIZED,
                "missing or invalid x-contract-blockchain-auth header",
            ))
        }
    }
}

/// Monotonic-ish id with a per-process counter to avoid same-millisecond
/// collisions in the bounded in-memory stores.
pub(super) fn gen_id(prefix: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{}-{:x}", crate::now_ms(), seq)
}

/// Evicts the oldest entry (by a timestamp accessor) once `map` is at `cap`.
pub(super) fn evict_oldest<V>(
    map: &mut HashMap<String, V>,
    cap: usize,
    timestamp: impl Fn(&V) -> u128,
) {
    if map.len() < cap {
        return;
    }
    if let Some(oldest) = map
        .iter()
        .min_by_key(|(_, value)| timestamp(value))
        .map(|(key, _)| key.clone())
    {
        map.remove(&oldest);
    }
}

/// Validates a short free-text label.
pub(super) fn validate_label(value: &str, field: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{field} must not be empty"));
    }
    if trimmed.chars().count() > MAX_BLOCKCHAIN_LABEL {
        return Err(format!("{field} must be at most {MAX_BLOCKCHAIN_LABEL} characters"));
    }
    Ok(trimmed.to_string())
}

/// Parses + validates an address for a chain, returning its canonical form.
pub(super) fn validate_chain_address(kind: ChainKind, address: &str) -> Result<String, String> {
    match kind {
        ChainKind::Solana => {
            crate::validate_pubkey(address, "address").map(|()| address.trim().to_string())
        }
        ChainKind::Evm => evm::validate_evm_address(address),
    }
}

/// Resolves a chain id string (`"solana"`/`"evm"`) to a [`ChainKind`].
pub(super) fn parse_chain(value: &str) -> Result<ChainKind, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "solana" | "sol" => Ok(ChainKind::Solana),
        "evm" | "eth" | "ethereum" => Ok(ChainKind::Evm),
        other => Err(format!("unsupported chain: {other} (expected solana or evm)")),
    }
}

pub(super) fn chain_label(kind: ChainKind) -> &'static str {
    match kind {
        ChainKind::Solana => "solana",
        ChainKind::Evm => "evm",
    }
}

/// Bumps the shared request counter; called at the top of each handler.
pub(super) fn record_request(state: &BlockchainState) {
    state.metrics().requests_total.fetch_add(1, Ordering::Relaxed);
}

// ---- router -----------------------------------------------------------------

/// All blockchain routes, merged into the main router before `with_state`.
pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/chains", get(chains_http))
        .route("/chain/:id/status", get(chain_status_http))
        .merge(wallet::routes())
        .merge(executor::routes())
        .merge(relayer::routes())
        .merge(multisig::routes())
        .merge(indexer::routes())
        .merge(mev::routes())
        .merge(nft::routes())
        .merge(staking::routes())
        .merge(bridge::routes())
}

async fn chains_http(axum::extract::State(state): axum::extract::State<AppState>) -> Response {
    let bc = &state.blockchain;
    record_request(bc);
    let c = bc.config();
    json_ok(json!({
        "ok": true,
        "signer": bc.inner.signer.label(),
        "chains": [
            {
                "id": "solana",
                "kind": "solana",
                "cluster": c.solana_cluster,
                "configured": true,
                "capabilities": ["validate", "simulate", "read", "confirm"],
            },
            {
                "id": "evm",
                "kind": "evm",
                "network": c.evm_network,
                "chainId": c.evm_chain_id,
                "configured": c.evm_configured(),
                "mainnet": evm::is_evm_mainnet(c.evm_chain_id, &c.evm_network),
                "capabilities": ["read", "call", "estimateGas", "relay"],
            }
        ],
        "features": bc.startup_summary(),
    }))
}

async fn chain_status_http(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    let bc = &state.blockchain;
    record_request(bc);
    let kind = match parse_chain(&id) {
        Ok(kind) => kind,
        Err(error) => return json_err(StatusCode::BAD_REQUEST, &error),
    };
    match kind {
        ChainKind::Solana => match crate::solana_rpc(&state, "getHealth", json!([])).await {
            Ok(result) => json_ok(json!({ "ok": true, "chain": "solana", "health": result })),
            Err(error) => json_err(StatusCode::BAD_GATEWAY, &error),
        },
        ChainKind::Evm => {
            if !bc.config().evm_configured() {
                return json_err(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "EVM RPC is not configured (set EVM_RPC_URL)",
                );
            }
            match bc.evm_rpc("eth_blockNumber", json!([])).await {
                Ok(result) => json_ok(json!({ "ok": true, "chain": "evm", "blockNumber": result })),
                Err(error) => json_err(StatusCode::BAD_GATEWAY, &error),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> BlockchainState {
        BlockchainState::from_env(reqwest::Client::new(), "https://api.devnet.solana.com", "devnet")
            .unwrap()
    }

    #[test]
    fn defaults_are_all_disabled() {
        let bc = state();
        let c = bc.config();
        assert!(!c.wallet_enabled);
        assert!(!c.executor_execute_enabled);
        assert!(!c.relayer_broadcast_enabled);
        assert!(!c.bridge_broadcast_enabled);
        assert!(c.execute_auth_secret.is_none());
    }

    #[test]
    fn parse_chain_accepts_aliases() {
        assert!(matches!(parse_chain("solana").unwrap(), ChainKind::Solana));
        assert!(matches!(parse_chain("ETH").unwrap(), ChainKind::Evm));
        assert!(parse_chain("dogecoin").is_err());
    }

    #[test]
    fn evict_oldest_drops_min_timestamp() {
        let mut map: HashMap<String, u128> = HashMap::new();
        map.insert("a".into(), 10);
        map.insert("b".into(), 5);
        map.insert("c".into(), 20);
        evict_oldest(&mut map, 3, |v| *v);
        assert!(!map.contains_key("b"));
        assert_eq!(map.len(), 2);
    }
}
