mod backends;
mod config;
mod domain;
mod handlers;
mod logging;
mod metrics;
mod nats;
mod settlement;
mod state;
#[cfg(test)]
mod tests;
mod types;
mod util;
mod validation;

use std::{env, error::Error, net::SocketAddr, sync::Arc, time::Duration};

use axum::{
    extract::DefaultBodyLimit,
    routing::{get, post},
    Router,
};
use dd_nats_subject_defs::{
    ESCROW_SOLANA_RESULTS_SUBJECT, ESCROW_SOLANA_VALIDATE_SUBJECT,
    RUNTIME_CRITICAL_EVENTS_SUBJECT, RUNTIME_EVENTS_SUBJECT,
};
use serde_json::json;

use crate::config::{
    config_error, env_bool, env_pubkey_list, env_secret, env_u64, env_value,
    validate_contract_service_url, validate_solana_rpc_url, SettlementBackend,
    DEFAULT_CONTRACT_SERVICE_TIMEOUT_SECONDS, MAX_HTTP_BODY_BYTES,
};
use crate::handlers::{
    api_docs_html, api_docs_json, audit_http, capabilities_http, example_http, healthz, home,
    resolve_http, schema_http, settle_http, simulate_settlement_http, status_http, types_http,
    validate_http,
};
use crate::logging::{log_error, log_info};
use crate::metrics::{metrics, Metrics};
use crate::nats::run_nats_loop;
use crate::state::AppState;
use crate::validation::normalize_cluster;

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        log_error(
            "escrow-shutdown-signal-failed",
            "Escrow service failed while waiting for Ctrl-C.",
            json!({ "error": error.to_string() }),
        );
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let _otel = dd_telemetry::init("dd-escrow-rs");

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
    let mainnet_settlement_enabled = env_bool("SOLANA_MAINNET_SETTLEMENT_ENABLED", false);
    let settlement_auth_secret = env_secret("ESCROW_SETTLEMENT_AUTH_SECRET");
    if settlement_enabled && settlement_auth_secret.is_none() {
        return Err(config_error(
            "SOLANA_SETTLEMENT_ENABLED=true requires ESCROW_SETTLEMENT_AUTH_SECRET",
        )
        .into());
    }
    if settlement_enabled && default_cluster == "mainnet-beta" && !mainnet_settlement_enabled {
        return Err(config_error(
            "mainnet settlement requires SOLANA_MAINNET_SETTLEMENT_ENABLED=true",
        )
        .into());
    }
    let settlement_require_intent = env_bool("ESCROW_SETTLEMENT_REQUIRE_INTENT", true);
    let allowed_program_ids =
        env_pubkey_list("ESCROW_ALLOWED_PROGRAM_IDS").map_err(config_error)?;
    let allow_skip_preflight = env_bool("SOLANA_ALLOW_SKIP_PREFLIGHT", false);
    let settlement_backend =
        SettlementBackend::parse(&env_value("ESCROW_SETTLEMENT_BACKEND", "contract-service"))
            .map_err(config_error)?;
    let contract_service_url = match env_secret("CONTRACT_SERVICE_URL") {
        Some(raw) => Some(validate_contract_service_url(&raw).map_err(config_error)?),
        None => None,
    };
    if settlement_backend == SettlementBackend::ContractService && contract_service_url.is_none() {
        return Err(config_error(
            "ESCROW_SETTLEMENT_BACKEND=contract-service requires CONTRACT_SERVICE_URL",
        )
        .into());
    }
    let contract_service_send_secret = env_secret("CONTRACT_SERVICE_SEND_AUTH_SECRET");
    let contract_service_timeout = Duration::from_secs(env_u64(
        "CONTRACT_SERVICE_TIMEOUT_SECONDS",
        DEFAULT_CONTRACT_SERVICE_TIMEOUT_SECONDS,
    ));
    let rpc_timeout_seconds = env_u64("SOLANA_RPC_TIMEOUT_SECONDS", 20);
    let validate_subject = env_value("ESCROW_VALIDATE_SUBJECT", ESCROW_SOLANA_VALIDATE_SUBJECT);
    let result_subject = env_value("ESCROW_RESULT_SUBJECT", ESCROW_SOLANA_RESULTS_SUBJECT);
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
        settlement_backend,
        contract_service_url,
        contract_service_send_secret,
        contract_service_timeout,
        settlement_enabled,
        settlement_auth_secret,
        settlement_require_intent,
        allowed_program_ids,
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
            "settlementRequiresIntent": state.settlement_require_intent,
            "settlementBackend": state.settlement_backend.as_str(),
            "contractServiceConfigured": state.contract_service_url.is_some(),
            "allowedProgramCount": state.allowed_program_ids.len(),
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
        .route("/capabilities", get(capabilities_http))
        .route("/schema", get(schema_http))
        .route("/example", get(example_http))
        .route("/validate", post(validate_http))
        .route("/audit", post(audit_http))
        .route("/resolve", post(resolve_http))
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
    axum::serve(listener, app.layer(dd_telemetry::http_trace_layer()))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}
