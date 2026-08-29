use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};

use axum::http::HeaderMap;
use base64::{engine::general_purpose, Engine as _};
use dd_nats_subject_defs::{
    ESCROW_SOLANA_RESULTS_SUBJECT, ESCROW_SOLANA_VALIDATE_SUBJECT,
    RUNTIME_CRITICAL_EVENTS_SUBJECT, RUNTIME_EVENTS_SUBJECT,
};
use serde_json::json;

use crate::backends::{contract_service_body, ContractServiceOp};
use crate::config::{
    validate_contract_service_url, validate_solana_rpc_url, SettlementBackend,
    MAX_SIGNED_TRANSACTION_BYTES, SCHEMA_VERSION, SETTLEMENT_AUTH_HEADER,
};
use crate::domain::{
    kind_catalog, kind_spec, EscrowKind, PartyRole, ReleaseMode, ResolutionOutcome,
    SettlementAction,
};
use crate::handlers::{collab_show_example, example_request};
use crate::metrics::{metrics_body, Metrics};
use crate::settlement::{
    authorize_settlement, validate_resolution, validate_settlement_request,
    validate_signed_transaction,
};
use crate::state::AppState;
use crate::types::{
    EscrowIntentRequest, EscrowParty, EscrowResolution, EscrowSettlementRequest,
    ResolutionAllocation,
};
use crate::validation::validate_escrow_intent;

fn sample_request() -> EscrowIntentRequest {
    example_request()
}

fn sample_state() -> AppState {
    AppState {
        rpc_client: reqwest::Client::new(),
        solana_rpc_url: "https://api.devnet.solana.com".to_string(),
        default_cluster: "devnet".to_string(),
        settlement_backend: SettlementBackend::SolanaRpc,
        contract_service_url: None,
        contract_service_send_secret: None,
        contract_service_timeout: Duration::from_secs(20),
        settlement_enabled: true,
        settlement_auth_secret: Some("secret".to_string()),
        settlement_require_intent: true,
        allowed_program_ids: Vec::new(),
        allow_skip_preflight: false,
        nats: None,
        validate_subject: ESCROW_SOLANA_VALIDATE_SUBJECT.to_string(),
        result_subject: ESCROW_SOLANA_RESULTS_SUBJECT.to_string(),
        event_subject: RUNTIME_EVENTS_SUBJECT.to_string(),
        critical_event_subject: RUNTIME_CRITICAL_EVENTS_SUBJECT.to_string(),
        metrics: Arc::new(Metrics::default()),
    }
}

#[test]
fn catalog_has_eleven_escrow_kinds() {
    let catalog = kind_catalog();
    assert_eq!(catalog.len(), 11);
    assert!(catalog
        .iter()
        .any(|entry| entry.kind == "marketplace-order"));
    assert!(catalog.iter().any(|entry| entry.kind == "group-buy"));
    assert!(catalog.iter().any(|entry| entry.kind == "collab-show"));
}

#[test]
fn marketplace_order_validates() {
    let request = sample_request();
    let response =
        validate_escrow_intent(&request, "devnet", &[]).expect("sample escrow should validate");
    assert_eq!(response.kind, EscrowKind::MarketplaceOrder);
    assert!(response.on_chain_settlement_ready);
    assert_eq!(response.party_count, 2);
    assert_eq!(response.readiness.risk_tier, "low");
    assert!(response
        .checks
        .iter()
        .any(|check| check.name == "settlement-plan" && check.ok));
}

#[test]
fn invalid_pubkey_is_rejected() {
    let mut request = sample_request();
    request.parties[0].pubkey = "not-a-solana-key".to_string();
    let errors =
        validate_escrow_intent(&request, "devnet", &[]).expect_err("must reject pubkey");
    assert!(errors.iter().any(|error| error.contains("valid base58")));
}

#[test]
fn group_buy_requires_two_contributors() {
    let mut request = sample_request();
    request.kind = EscrowKind::GroupBuy;
    request.parties[0].role = PartyRole::Contributor;
    let errors =
        validate_escrow_intent(&request, "devnet", &[]).expect_err("must reject group-buy");
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
        resolution: None,
    };
    let errors = validate_settlement_request(&request, "devnet", false, &[], false)
        .expect_err("must reject action");
    assert!(errors.iter().any(|error| error.contains("does not allow")));
}

#[test]
fn live_settlement_requires_intent_by_default() {
    let request = EscrowSettlementRequest {
        schema_version: SCHEMA_VERSION.to_string(),
        request_id: Some("settle-demo".to_string()),
        cluster: Some("devnet".to_string()),
        kind: EscrowKind::MarketplaceOrder,
        escrow_id: "order.demo.001".to_string(),
        action: SettlementAction::Release,
        transaction: general_purpose::STANDARD.encode([1_u8, 2, 3]),
        encoding: Some("base64".to_string()),
        commitment: None,
        skip_preflight: None,
        max_retries: None,
        min_context_slot: None,
        intent: None,
        resolution: None,
    };
    let errors = validate_settlement_request(&request, "devnet", false, &[], true)
        .expect_err("must require intent for live settlement");
    assert!(errors
        .iter()
        .any(|error| error.contains("intent is required")));
}

#[test]
fn settlement_plan_respects_program_allowlist() {
    let request = sample_request();
    let different_program = bs58::encode([9_u8; 32]).into_string();
    let errors = validate_escrow_intent(&request, "devnet", &[different_program])
        .expect_err("must reject non-allowlisted program");
    assert!(errors
        .iter()
        .any(|error| error.contains("ESCROW_ALLOWED_PROGRAM_IDS")));

    let allowed = request
        .settlement_plan
        .as_ref()
        .map(|plan| plan.program_id.clone())
        .unwrap();
    assert!(validate_escrow_intent(&request, "devnet", &[allowed]).is_ok());
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

fn party(role: PartyRole) -> EscrowParty {
    EscrowParty {
        role,
        pubkey: "11111111111111111111111111111111".to_string(),
        label: None,
        required_signer: None,
        payout_bps: None,
    }
}

fn run_resolution(
    kind: EscrowKind,
    action: SettlementAction,
    resolution: &EscrowResolution,
    parties: &[EscrowParty],
    release_mode: ReleaseMode,
) -> (Vec<String>, Vec<String>) {
    let spec = kind_spec(kind);
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    validate_resolution(
        action,
        resolution,
        parties,
        &spec,
        release_mode,
        &mut errors,
        &mut warnings,
    );
    (errors, warnings)
}

#[test]
fn resolution_outcome_must_match_action() {
    let resolution = EscrowResolution {
        outcome: ResolutionOutcome::Refund,
        winner_role: None,
        refund_role: None,
        allocations: None,
        rationale: None,
    };
    let (errors, _) = run_resolution(
        EscrowKind::MarketplaceOrder,
        SettlementAction::Release,
        &resolution,
        &[party(PartyRole::Buyer), party(PartyRole::Seller)],
        ReleaseMode::BuyerApproval,
    );
    assert!(errors.iter().any(|error| error.contains("not consistent")));
}

#[test]
fn refund_resolution_requires_refundable_party() {
    let resolution = EscrowResolution {
        outcome: ResolutionOutcome::Refund,
        winner_role: None,
        refund_role: None,
        allocations: None,
        rationale: None,
    };
    let (errors, _) = run_resolution(
        EscrowKind::MarketplaceOrder,
        SettlementAction::Refund,
        &resolution,
        &[party(PartyRole::Seller)],
        ReleaseMode::BuyerApproval,
    );
    assert!(errors
        .iter()
        .any(|error| error.contains("refundable party")));
}

#[test]
fn dispute_award_requires_arbiter_under_arbiter_decision() {
    let resolution = EscrowResolution {
        outcome: ResolutionOutcome::DisputeAward,
        winner_role: Some(PartyRole::Buyer),
        refund_role: None,
        allocations: None,
        rationale: None,
    };
    let (errors, _) = run_resolution(
        EscrowKind::MarketplaceOrder,
        SettlementAction::DisputeAward,
        &resolution,
        &[party(PartyRole::Buyer), party(PartyRole::Seller)],
        ReleaseMode::ArbiterDecision,
    );
    assert!(errors
        .iter()
        .any(|error| error.contains("requires an arbitrator party")));
}

#[test]
fn split_allocations_must_sum_to_10000() {
    let resolution = EscrowResolution {
        outcome: ResolutionOutcome::Split,
        winner_role: None,
        refund_role: None,
        allocations: Some(vec![ResolutionAllocation {
            role: PartyRole::Seller,
            pubkey: None,
            payout_bps: 5_000,
        }]),
        rationale: None,
    };
    let (errors, _) = run_resolution(
        EscrowKind::GroupBuy,
        SettlementAction::SplitRelease,
        &resolution,
        &[party(PartyRole::Contributor), party(PartyRole::Seller)],
        ReleaseMode::TimeLocked,
    );
    assert!(errors
        .iter()
        .any(|error| error.contains("sum to exactly 10000")));
}

#[test]
fn valid_split_resolution_passes() {
    let resolution = EscrowResolution {
        outcome: ResolutionOutcome::Split,
        winner_role: None,
        refund_role: None,
        allocations: Some(vec![
            ResolutionAllocation {
                role: PartyRole::Contributor,
                pubkey: None,
                payout_bps: 4_000,
            },
            ResolutionAllocation {
                role: PartyRole::Seller,
                pubkey: None,
                payout_bps: 6_000,
            },
        ]),
        rationale: Some("agreed split".to_string()),
    };
    let (errors, _) = run_resolution(
        EscrowKind::GroupBuy,
        SettlementAction::SplitRelease,
        &resolution,
        &[party(PartyRole::Contributor), party(PartyRole::Seller)],
        ReleaseMode::TimeLocked,
    );
    assert!(errors.is_empty(), "expected no errors, got {errors:?}");
}

#[test]
fn collab_show_example_validates() {
    let request = collab_show_example();
    let response = validate_escrow_intent(&request, "devnet", &[])
        .expect("collab-show example should validate");
    assert_eq!(response.kind, EscrowKind::CollabShow);
    assert_eq!(response.party_count, 3);
    assert!(response.on_chain_settlement_ready);
    assert!(response.required_roles.contains(&"creator"));
}

#[test]
fn collab_show_requires_two_creators() {
    let mut request = collab_show_example();
    // Demote the second creator so only one creator remains.
    request.parties[1].role = PartyRole::Platform;
    request.parties[1].payout_bps = None;
    request.parties[0].payout_bps = Some(10_000);
    let errors = validate_escrow_intent(&request, "devnet", &[])
        .expect_err("must reject a single-creator collab show");
    assert!(errors
        .iter()
        .any(|error| error.contains("at least two creator")));
}

#[test]
fn collab_show_requires_arbiter() {
    let mut request = collab_show_example();
    // Drop the arbiter party.
    request.parties.retain(|p| p.role != PartyRole::Arbitrator);
    let errors = validate_escrow_intent(&request, "devnet", &[])
        .expect_err("must reject a collab show with no arbiter");
    assert!(errors
        .iter()
        .any(|error| error.contains("role arbitrator")));
}

#[test]
fn collab_show_requires_revenue_split() {
    let mut request = collab_show_example();
    // Drop one creator's payoutBps: the revenue split is now undefined.
    request.parties[0].payout_bps = None;
    let errors = validate_escrow_intent(&request, "devnet", &[])
        .expect_err("must reject a collab show with no revenue split");
    assert!(errors
        .iter()
        .any(|error| error.contains("payoutBps for the agreed revenue split")));
}

#[test]
fn collab_show_requires_show_deadline() {
    let mut request = collab_show_example();
    request.terms.timeout_unix_seconds = None;
    let errors = validate_escrow_intent(&request, "devnet", &[])
        .expect_err("must reject a collab show with no deadline");
    assert!(errors
        .iter()
        .any(|error| error.contains("timeoutUnixSeconds (the show date")));
}

#[test]
fn collab_show_requires_dispute_window() {
    let mut request = collab_show_example();
    request.terms.dispute_window_seconds = None;
    let errors = validate_escrow_intent(&request, "devnet", &[])
        .expect_err("must reject a collab show with no dispute window");
    assert!(errors
        .iter()
        .any(|error| error.contains("disputeWindowSeconds for breach adjudication")));
}

#[test]
fn collab_show_arbiter_must_not_take_payout() {
    let mut request = collab_show_example();
    // Give the arbiter a payout slice and rebalance so the global sum still hits 10000.
    request.parties[0].payout_bps = Some(5_000);
    request.parties[1].payout_bps = Some(4_000);
    request.parties[2].payout_bps = Some(1_000); // arbiter
    let errors = validate_escrow_intent(&request, "devnet", &[])
        .expect_err("must reject an arbiter that shares the payout split");
    assert!(errors
        .iter()
        .any(|error| error.contains("arbiter must not carry payoutBps")));
}

#[test]
fn collab_show_no_show_awards_other_creator() {
    let resolution = EscrowResolution {
        outcome: ResolutionOutcome::DisputeAward,
        winner_role: Some(PartyRole::Creator),
        refund_role: None,
        allocations: None,
        rationale: Some("creator-b no-showed".to_string()),
    };
    let (errors, _) = run_resolution(
        EscrowKind::CollabShow,
        SettlementAction::DisputeAward,
        &resolution,
        &[
            party(PartyRole::Creator),
            party(PartyRole::Creator),
            party(PartyRole::Arbitrator),
        ],
        ReleaseMode::ArbiterDecision,
    );
    assert!(errors.is_empty(), "expected no errors, got {errors:?}");
}

#[test]
fn collab_show_arbiter_split_validates() {
    let resolution = EscrowResolution {
        outcome: ResolutionOutcome::Split,
        winner_role: None,
        refund_role: None,
        allocations: Some(vec![
            ResolutionAllocation {
                role: PartyRole::Creator,
                pubkey: None,
                payout_bps: 7_000,
            },
            ResolutionAllocation {
                role: PartyRole::Creator,
                pubkey: None,
                payout_bps: 3_000,
            },
        ]),
        rationale: Some("arbiter-set partial-fault split".to_string()),
    };
    let (errors, _) = run_resolution(
        EscrowKind::CollabShow,
        SettlementAction::SplitRelease,
        &resolution,
        &[
            party(PartyRole::Creator),
            party(PartyRole::Creator),
            party(PartyRole::Arbitrator),
        ],
        ReleaseMode::ArbiterDecision,
    );
    assert!(errors.is_empty(), "expected no errors, got {errors:?}");
}

#[test]
fn contract_service_send_body_maps_fields_and_omits_simulate_keys() {
    let request = EscrowSettlementRequest {
        schema_version: SCHEMA_VERSION.to_string(),
        request_id: Some("settle-demo".to_string()),
        cluster: Some("devnet".to_string()),
        kind: EscrowKind::MarketplaceOrder,
        escrow_id: "order.demo.001".to_string(),
        action: SettlementAction::Release,
        transaction: general_purpose::STANDARD.encode([1_u8, 2, 3]),
        encoding: Some("base64".to_string()),
        commitment: None,
        skip_preflight: None,
        max_retries: Some(5),
        min_context_slot: None,
        intent: None,
        resolution: None,
    };
    let body = contract_service_body(
        &request,
        ContractServiceOp::Send,
        "devnet",
        "base64",
        "confirmed",
        "settle-demo",
    );
    assert_eq!(body["cluster"], json!("devnet"));
    assert_eq!(body["encoding"], json!("base64"));
    assert_eq!(body["skipPreflight"], json!(false));
    assert_eq!(body["maxRetries"], json!(5));
    assert!(body.get("sigVerify").is_none());
    assert!(body.get("replaceRecentBlockhash").is_none());

    let simulate = contract_service_body(
        &request,
        ContractServiceOp::Simulate,
        "devnet",
        "base64",
        "confirmed",
        "settle-demo",
    );
    assert_eq!(simulate["sigVerify"], json!(false));
    assert_eq!(simulate["replaceRecentBlockhash"], json!(true));
    assert!(simulate.get("skipPreflight").is_none());
}

#[test]
fn contract_service_url_allows_cluster_local() {
    let url = validate_contract_service_url(
        "http://dd-contract-service.default.svc.cluster.local:8101",
    )
    .expect("cluster-local contract-service URL should be allowed");
    assert!(url.contains("dd-contract-service"));
    assert!(validate_contract_service_url("http://user:pass@host:8101").is_err());
}
