use std::{
    collections::HashMap,
    sync::{atomic::AtomicU64, Arc, Mutex},
};

use crate::metrics::Metrics;
use crate::shared::now_ms;
use crate::{blockchain, coordination, solana_features};

pub(crate) const SCHEMA_VERSION: &str = "solana.contract.v1";
pub(crate) const MAX_HTTP_BODY_BYTES: usize = 512 * 1024;
pub(crate) const MAX_NATS_PAYLOAD_BYTES: usize = 512 * 1024;
pub(crate) const MAX_SIGNED_TRANSACTION_BYTES: usize = 256 * 1024;
pub(crate) const MAX_RPC_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_RPC_IN_FLIGHT: usize = 64;
pub(crate) const MAX_INSTRUCTIONS: usize = 16;
pub(crate) const MAX_ACCOUNTS_PER_INSTRUCTION: usize = 64;
pub(crate) const MAX_INSTRUCTION_DATA_BYTES: usize = 16 * 1024;
pub(crate) const MAX_MEMO_BYTES: usize = 512;
pub(crate) const MAX_REQUEST_ID_LEN: usize = 128;
pub(crate) const MAX_LABEL_LEN: usize = 64;
pub(crate) const DEFAULT_COMPUTE_UNITS: u32 = 200_000;
pub(crate) const MAX_COMPUTE_UNITS_PER_INSTRUCTION: u32 = 1_400_000;
pub(crate) const MAX_TRANSACTION_COMPUTE_UNITS: u64 = 1_400_000;
pub(crate) const MAX_SEND_RETRIES: usize = 20;
pub(crate) const DEFAULT_COMMITMENT: &str = "confirmed";
pub(crate) const SEND_AUTH_HEADER: &str = "x-contract-send-auth";
pub(crate) const SETTLEMENT_AUTH_HEADER: &str = "x-contract-settlement-auth";
pub(crate) const SETTLEMENT_SCHEMA_VERSION: &str = "solana.settlement.v1";
pub(crate) const RESOLUTION_SCHEMA_VERSION: &str = "solana.resolution.v1";
pub(crate) const MAX_SIGNATURE_LEN: usize = 96;
pub(crate) const MAX_RATIONALE_BYTES: usize = 2048;
pub(crate) const MAX_RENT_EXEMPTION_BYTES: u64 = 10 * 1024 * 1024;
pub(crate) const MAX_CONFIRM_SIGNATURES: usize = 8;
pub(crate) const DEFAULT_CONFIRM_TIMEOUT_MS: u64 = 30_000;
pub(crate) const MAX_CONFIRM_TIMEOUT_MS: u64 = 120_000;
pub(crate) const MIN_CONFIRM_POLL_INTERVAL_MS: u64 = 250;
pub(crate) const MAX_CONFIRM_POLL_INTERVAL_MS: u64 = 10_000;
pub(crate) const DEFAULT_CONFIRM_POLL_INTERVAL_MS: u64 = 1_500;
pub(crate) const MAX_CONFIRM_POLLS: u32 = 240;
pub(crate) const IDEMPOTENCY_TTL_MS: u128 = 10 * 60 * 1000;
pub(crate) const MAX_IDEMPOTENCY_ENTRIES: usize = 8_192;
// Service-wide cap on concurrent confirmation pollers across /confirm, /settle,
// /resolve, and the escrow-results verifier. Bounds sustained outbound Solana
// RPC fan-out so no set of requests (nor a flood of escrow result messages on
// the currently unauthenticated NATS bus) can amplify load on the upstream RPC
// endpoint; excess confirmations are shed and reported as "deferred".
pub(crate) const MAX_CONFIRM_POLLERS_IN_FLIGHT: u64 = 64;
pub(crate) const SERVICE_NAME: &str = "dd-contract-service";
pub(crate) const SERVICE_NAMESPACE: &str = "remote-dev";
pub(crate) const LOG_SCHEMA: &str = "dd.log.v1";
pub(crate) const LOG_SCOPE: &str = "contract-service-rs";

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) rpc_client: reqwest::Client,
    pub(crate) solana_rpc_url: String,
    pub(crate) default_cluster: String,
    pub(crate) send_enabled: bool,
    pub(crate) send_auth_secret: Option<String>,
    pub(crate) allow_skip_preflight: bool,
    pub(crate) settlement_enabled: bool,
    pub(crate) resolution_enabled: bool,
    pub(crate) nats_settlement_enabled: bool,
    pub(crate) mainnet_settlement_enabled: bool,
    pub(crate) settlement_auth_secret: Option<String>,
    pub(crate) nats: Option<async_nats::Client>,
    pub(crate) result_subject: String,
    pub(crate) settlement_result_subject: String,
    pub(crate) event_subject: String,
    pub(crate) critical_event_subject: String,
    pub(crate) metrics: Arc<Metrics>,
    pub(crate) idempotency: Arc<Mutex<HashMap<String, u128>>>,
    pub(crate) confirm_in_flight: Arc<AtomicU64>,
    pub(crate) rpc_slots: Arc<tokio::sync::Semaphore>,
    pub(crate) coordination: coordination::CoordinationState,
    pub(crate) solana_features: solana_features::SolanaFeatureState,
    /// Keyless, off-by-default blockchain feature suite (wallets, executor,
    /// relayer, multisig, indexing, MEV monitoring, NFT storage, staking, bridge).
    pub(crate) blockchain: blockchain::BlockchainState,
}

impl AppState {
    /// Records a settlement/resolution request id for at-most-once broadcast.
    /// Returns `false` when the id was already seen within the TTL window (a
    /// replay), so callers can skip a duplicate on-chain broadcast.
    pub(crate) fn claim_idempotency_key(&self, key: &str) -> bool {
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
    pub(crate) fn release_idempotency_key(&self, key: &str) {
        let mut guard = match self.idempotency.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.remove(key);
    }
}
