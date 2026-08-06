use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};

use axum::http::HeaderMap;
use base64::{engine::general_purpose, Engine as _};
use serde_json::json;

use crate::confirm::*;
use crate::metrics::*;
use crate::rpc::*;
use crate::settlement::*;
use crate::shared::*;
use crate::state::*;
use crate::validation::*;
use crate::{blockchain, coordination, solana_features};

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
        transaction: general_purpose::STANDARD.encode(vec![7_u8; MAX_SIGNED_TRANSACTION_BYTES + 1]),
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
fn broadcasts_require_enabled_fail_closed_coordination() {
    assert!(enforce_broadcast_coordination(false, false, false).is_ok());
    assert!(enforce_broadcast_coordination(true, false, false).is_err());
    assert!(enforce_broadcast_coordination(true, true, false).is_err());
    assert!(enforce_broadcast_coordination(true, true, true).is_ok());
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
    assert!(body.contains("dd_contract_service_nats_published_total{subject_kind=\"result\"} 2"));
    assert!(body.contains("dd_contract_service_nats_published_total{subject_kind=\"critical\"} 1"));
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
    assert_eq!(
        counter.load(Ordering::Acquire),
        MAX_CONFIRM_POLLERS_IN_FLIGHT
    );
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
