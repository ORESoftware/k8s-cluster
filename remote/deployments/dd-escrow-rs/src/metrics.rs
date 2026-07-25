use std::sync::atomic::{AtomicU64, Ordering};

use axum::{extract::State, response::IntoResponse};

use crate::state::AppState;

#[derive(Default)]
pub(crate) struct Metrics {
    pub(crate) validations_total: AtomicU64,
    pub(crate) validation_errors_total: AtomicU64,
    pub(crate) simulations_total: AtomicU64,
    pub(crate) settlements_total: AtomicU64,
    pub(crate) settlement_errors_total: AtomicU64,
    pub(crate) rpc_requests_total: AtomicU64,
    pub(crate) rpc_errors_total: AtomicU64,
    pub(crate) contract_service_simulate_total: AtomicU64,
    pub(crate) contract_service_send_total: AtomicU64,
    pub(crate) contract_service_errors_total: AtomicU64,
    pub(crate) resolution_validations_total: AtomicU64,
    pub(crate) resolution_errors_total: AtomicU64,
    pub(crate) policy_rejections_total: AtomicU64,
    pub(crate) auth_failures_total: AtomicU64,
    pub(crate) nats_messages_total: AtomicU64,
    pub(crate) nats_payload_rejected_total: AtomicU64,
    pub(crate) nats_results_published_total: AtomicU64,
    pub(crate) nats_events_published_total: AtomicU64,
    pub(crate) nats_critical_events_published_total: AtomicU64,
    pub(crate) nats_publish_errors_total: AtomicU64,
    pub(crate) errors_total: AtomicU64,
}

pub(crate) fn label_value(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

pub(crate) fn metrics_body(state: &AppState) -> String {
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
            "# HELP dd_escrow_rs_contract_service_requests_total Settlement operations delegated to dd-contract-service by op.\n",
            "# TYPE dd_escrow_rs_contract_service_requests_total counter\n",
            "dd_escrow_rs_contract_service_requests_total{{op=\"simulate\"}} {}\n",
            "dd_escrow_rs_contract_service_requests_total{{op=\"send\"}} {}\n",
            "# HELP dd_escrow_rs_contract_service_errors_total dd-contract-service delegation errors.\n",
            "# TYPE dd_escrow_rs_contract_service_errors_total counter\n",
            "dd_escrow_rs_contract_service_errors_total {}\n",
            "# HELP dd_escrow_rs_resolution_validations_total Resolution validations evaluated.\n",
            "# TYPE dd_escrow_rs_resolution_validations_total counter\n",
            "dd_escrow_rs_resolution_validations_total {}\n",
            "# HELP dd_escrow_rs_resolution_errors_total Resolution validations rejected.\n",
            "# TYPE dd_escrow_rs_resolution_errors_total counter\n",
            "dd_escrow_rs_resolution_errors_total {}\n",
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
        metrics.contract_service_simulate_total.load(Ordering::Relaxed),
        metrics.contract_service_send_total.load(Ordering::Relaxed),
        metrics.contract_service_errors_total.load(Ordering::Relaxed),
        metrics.resolution_validations_total.load(Ordering::Relaxed),
        metrics.resolution_errors_total.load(Ordering::Relaxed),
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

pub(crate) async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    (
        [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
        metrics_body(&state),
    )
}
