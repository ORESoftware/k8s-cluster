#![recursion_limit = "256"]

use std::{
    collections::HashMap,
    env,
    error::Error,
    net::SocketAddr,
    sync::{atomic::AtomicU64, Arc, Mutex},
    time::Duration,
};

use axum::{
    extract::DefaultBodyLimit,
    routing::{get, post},
    Router,
};
use dd_nats_subject_defs::{
    CONTRACTS_SOLANA_RESOLVE_QUEUE_GROUP, CONTRACTS_SOLANA_RESOLVE_SUBJECT,
    CONTRACTS_SOLANA_RESULTS_SUBJECT, CONTRACTS_SOLANA_SETTLEMENT_RESULTS_SUBJECT,
    CONTRACTS_SOLANA_SETTLE_QUEUE_GROUP, CONTRACTS_SOLANA_SETTLE_SUBJECT,
    CONTRACTS_SOLANA_VALIDATE_QUEUE_GROUP, CONTRACTS_SOLANA_VALIDATE_SUBJECT,
    ESCROW_SOLANA_RESULTS_SUBJECT, RUNTIME_CRITICAL_EVENTS_SUBJECT, RUNTIME_EVENTS_SUBJECT,
};
use serde_json::json;

mod blockchain;
mod confirm;
mod coordination;
mod handlers;
mod metrics;
mod nats;
mod rpc;
mod settlement;
mod shared;
mod solana_features;
mod state;
#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests;
mod validation;

pub(crate) use crate::metrics::push_counter;
pub(crate) use crate::nats::publish_blockchain_event;
pub(crate) use crate::rpc::solana_rpc;
pub(crate) use crate::shared::{
    env_bool, env_secret, env_u64, env_value, json_response, now_ms, sensitive_eq,
};
pub(crate) use crate::state::{AppState, SERVICE_NAME};
pub(crate) use crate::validation::{
    normalize_commitment_or_default, validate_pubkey, validate_request_id, validate_signature,
    validate_solana_rpc_url,
};

use crate::handlers::*;
use crate::metrics::{metrics, Metrics};
use crate::nats::{run_nats_loop, NatsKind};
use crate::settlement::{resolve_http, settle_http, simulate_settlement_http};
use crate::shared::{config_error, log_error, log_info};
use crate::state::{MAX_HTTP_BODY_BYTES, MAX_RPC_IN_FLIGHT};
use crate::validation::{
    enforce_broadcast_coordination, enforce_mainnet_settlement_gate, enforce_nats_broadcast_ack,
    normalize_cluster,
};

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        log_error(
            "contract-shutdown-signal-failed",
            "Contract service failed while waiting for Ctrl-C.",
            json!({ "error": error.to_string() }),
        );
    }
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
    let escrow_results_subject = env_value(
        "CONTRACT_ESCROW_RESULT_SUBJECT",
        ESCROW_SOLANA_RESULTS_SUBJECT,
    );
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
    let blockchain = blockchain::BlockchainState::from_env(
        rpc_client.clone(),
        &solana_rpc_url,
        &default_cluster,
    )
    .map_err(config_error)?;
    let coordination =
        coordination::CoordinationState::from_env(rpc_client.clone()).map_err(config_error)?;
    enforce_broadcast_coordination(
        send_enabled || settlement_enabled || resolution_enabled || nats_settlement_enabled,
        coordination.enabled(),
        coordination.required(),
    )
    .map_err(config_error)?;
    let solana_features =
        solana_features::SolanaFeatureState::from_env(rpc_client.clone()).map_err(config_error)?;
    let rpc_max_in_flight =
        env_u64("SOLANA_RPC_MAX_IN_FLIGHT", MAX_RPC_IN_FLIGHT as u64).clamp(1, 512) as usize;

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
