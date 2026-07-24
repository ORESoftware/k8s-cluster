use std::sync::atomic::{AtomicU64, Ordering};

use axum::{
    extract::State,
    http::header,
    response::{IntoResponse, Response},
};

use crate::state::AppState;

#[derive(Default)]
pub(crate) struct Metrics {
    pub(crate) http_requests_total: AtomicU64,
    pub(crate) validations_total: AtomicU64,
    pub(crate) validation_errors_total: AtomicU64,
    pub(crate) rpc_requests_total: AtomicU64,
    pub(crate) rpc_errors_total: AtomicU64,
    pub(crate) nats_messages_total: AtomicU64,
    pub(crate) nats_payload_rejected_total: AtomicU64,
    pub(crate) nats_results_published_total: AtomicU64,
    pub(crate) nats_events_published_total: AtomicU64,
    pub(crate) nats_critical_events_published_total: AtomicU64,
    pub(crate) nats_publish_errors_total: AtomicU64,
    pub(crate) send_blocked_total: AtomicU64,
    pub(crate) send_auth_failures_total: AtomicU64,
    pub(crate) policy_rejections_total: AtomicU64,
    pub(crate) errors_total: AtomicU64,
    pub(crate) settlements_total: AtomicU64,
    pub(crate) settlement_errors_total: AtomicU64,
    pub(crate) resolutions_total: AtomicU64,
    pub(crate) resolution_errors_total: AtomicU64,
    pub(crate) settlement_idempotent_hits_total: AtomicU64,
    pub(crate) confirmations_confirmed_total: AtomicU64,
    pub(crate) confirmations_finalized_total: AtomicU64,
    pub(crate) confirmations_failed_total: AtomicU64,
    pub(crate) confirmations_pending_total: AtomicU64,
    pub(crate) confirmations_deferred_total: AtomicU64,
    pub(crate) rpc_get_health_requests_total: AtomicU64,
    pub(crate) rpc_get_health_errors_total: AtomicU64,
    pub(crate) rpc_get_version_requests_total: AtomicU64,
    pub(crate) rpc_get_version_errors_total: AtomicU64,
    pub(crate) rpc_simulate_transaction_requests_total: AtomicU64,
    pub(crate) rpc_simulate_transaction_errors_total: AtomicU64,
    pub(crate) rpc_send_transaction_requests_total: AtomicU64,
    pub(crate) rpc_send_transaction_errors_total: AtomicU64,
    pub(crate) rpc_get_latest_blockhash_requests_total: AtomicU64,
    pub(crate) rpc_get_latest_blockhash_errors_total: AtomicU64,
    pub(crate) rpc_get_signature_statuses_requests_total: AtomicU64,
    pub(crate) rpc_get_signature_statuses_errors_total: AtomicU64,
    pub(crate) rpc_get_transaction_requests_total: AtomicU64,
    pub(crate) rpc_get_transaction_errors_total: AtomicU64,
    pub(crate) rpc_get_account_info_requests_total: AtomicU64,
    pub(crate) rpc_get_account_info_errors_total: AtomicU64,
    pub(crate) rpc_get_balance_requests_total: AtomicU64,
    pub(crate) rpc_get_balance_errors_total: AtomicU64,
    pub(crate) rpc_get_token_account_balance_requests_total: AtomicU64,
    pub(crate) rpc_get_token_account_balance_errors_total: AtomicU64,
    pub(crate) rpc_get_fee_for_message_requests_total: AtomicU64,
    pub(crate) rpc_get_fee_for_message_errors_total: AtomicU64,
    pub(crate) rpc_get_minimum_balance_for_rent_exemption_requests_total: AtomicU64,
    pub(crate) rpc_get_minimum_balance_for_rent_exemption_errors_total: AtomicU64,
    pub(crate) rpc_get_signatures_for_address_requests_total: AtomicU64,
    pub(crate) rpc_get_signatures_for_address_errors_total: AtomicU64,
    pub(crate) rpc_get_recent_prioritization_fees_requests_total: AtomicU64,
    pub(crate) rpc_get_recent_prioritization_fees_errors_total: AtomicU64,
}

fn rpc_method_counters<'a>(metrics: &'a Metrics, method: &str) -> (&'a AtomicU64, &'a AtomicU64) {
    match method {
        "getHealth" => (
            &metrics.rpc_get_health_requests_total,
            &metrics.rpc_get_health_errors_total,
        ),
        "getVersion" => (
            &metrics.rpc_get_version_requests_total,
            &metrics.rpc_get_version_errors_total,
        ),
        "simulateTransaction" => (
            &metrics.rpc_simulate_transaction_requests_total,
            &metrics.rpc_simulate_transaction_errors_total,
        ),
        "sendTransaction" => (
            &metrics.rpc_send_transaction_requests_total,
            &metrics.rpc_send_transaction_errors_total,
        ),
        "getLatestBlockhash" => (
            &metrics.rpc_get_latest_blockhash_requests_total,
            &metrics.rpc_get_latest_blockhash_errors_total,
        ),
        "getSignatureStatuses" => (
            &metrics.rpc_get_signature_statuses_requests_total,
            &metrics.rpc_get_signature_statuses_errors_total,
        ),
        "getTransaction" => (
            &metrics.rpc_get_transaction_requests_total,
            &metrics.rpc_get_transaction_errors_total,
        ),
        "getAccountInfo" => (
            &metrics.rpc_get_account_info_requests_total,
            &metrics.rpc_get_account_info_errors_total,
        ),
        "getBalance" => (
            &metrics.rpc_get_balance_requests_total,
            &metrics.rpc_get_balance_errors_total,
        ),
        "getTokenAccountBalance" => (
            &metrics.rpc_get_token_account_balance_requests_total,
            &metrics.rpc_get_token_account_balance_errors_total,
        ),
        "getFeeForMessage" => (
            &metrics.rpc_get_fee_for_message_requests_total,
            &metrics.rpc_get_fee_for_message_errors_total,
        ),
        "getMinimumBalanceForRentExemption" => (
            &metrics.rpc_get_minimum_balance_for_rent_exemption_requests_total,
            &metrics.rpc_get_minimum_balance_for_rent_exemption_errors_total,
        ),
        "getSignaturesForAddress" => (
            &metrics.rpc_get_signatures_for_address_requests_total,
            &metrics.rpc_get_signatures_for_address_errors_total,
        ),
        "getRecentPrioritizationFees" => (
            &metrics.rpc_get_recent_prioritization_fees_requests_total,
            &metrics.rpc_get_recent_prioritization_fees_errors_total,
        ),
        _ => (&metrics.rpc_requests_total, &metrics.rpc_errors_total),
    }
}

pub(crate) fn record_rpc_request(metrics: &Metrics, method: &str) {
    metrics.rpc_requests_total.fetch_add(1, Ordering::Relaxed);
    let (requests, _) = rpc_method_counters(metrics, method);
    requests.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn record_rpc_error(metrics: &Metrics, method: &str) {
    metrics.rpc_errors_total.fetch_add(1, Ordering::Relaxed);
    let (_, errors) = rpc_method_counters(metrics, method);
    errors.fetch_add(1, Ordering::Relaxed);
}

fn bool_label(value: bool) -> &'static str {
    if value {
        "true"
    } else {
        "false"
    }
}

/// RPC methods exposed as low-cardinality `rpc_method` label values in the
/// per-method counter families.
const METRICS_RPC_METHODS: [&str; 14] = [
    "getHealth",
    "getVersion",
    "simulateTransaction",
    "sendTransaction",
    "getLatestBlockhash",
    "getSignatureStatuses",
    "getTransaction",
    "getAccountInfo",
    "getBalance",
    "getTokenAccountBalance",
    "getFeeForMessage",
    "getMinimumBalanceForRentExemption",
    "getSignaturesForAddress",
    "getRecentPrioritizationFees",
];

/// Appends a single-line Prometheus counter family (HELP/TYPE + one sample).
pub(crate) fn push_counter(out: &mut String, name: &str, help: &str, value: u64) {
    out.push_str(&format!(
        "# HELP {name} {help}\n# TYPE {name} counter\n{name} {value}\n"
    ));
}

pub(crate) fn metrics_body(state: &AppState) -> String {
    let m = &state.metrics;
    let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
    let mut out = String::with_capacity(4096);

    push_counter(
        &mut out,
        "dd_contract_service_http_requests_total",
        "HTTP requests handled by the Solana contract service.",
        load(&m.http_requests_total),
    );
    push_counter(
        &mut out,
        "dd_contract_service_validations_total",
        "Contract validation requests handled.",
        load(&m.validations_total),
    );
    push_counter(
        &mut out,
        "dd_contract_service_validation_errors_total",
        "Contract validation requests rejected.",
        load(&m.validation_errors_total),
    );
    push_counter(
        &mut out,
        "dd_contract_service_policy_rejections_total",
        "Requests rejected by contract service safety policy before upstream RPC.",
        load(&m.policy_rejections_total),
    );
    push_counter(
        &mut out,
        "dd_contract_service_settlements_total",
        "Settlement requests handled by /settle and the settle NATS subject.",
        load(&m.settlements_total),
    );
    push_counter(
        &mut out,
        "dd_contract_service_settlement_errors_total",
        "Settlement requests that failed validation or broadcast.",
        load(&m.settlement_errors_total),
    );
    push_counter(
        &mut out,
        "dd_contract_service_resolutions_total",
        "Dispute resolution requests handled by /resolve and the resolve NATS subject.",
        load(&m.resolutions_total),
    );
    push_counter(
        &mut out,
        "dd_contract_service_resolution_errors_total",
        "Dispute resolution requests that failed validation or broadcast.",
        load(&m.resolution_errors_total),
    );
    push_counter(
        &mut out,
        "dd_contract_service_settlement_idempotent_hits_total",
        "Settlement/resolution broadcasts suppressed by the idempotency guard.",
        load(&m.settlement_idempotent_hits_total),
    );
    push_counter(
        &mut out,
        "dd_contract_service_rpc_requests_total",
        "Solana JSON-RPC requests sent.",
        load(&m.rpc_requests_total),
    );
    push_counter(
        &mut out,
        "dd_contract_service_rpc_errors_total",
        "Solana JSON-RPC requests that failed.",
        load(&m.rpc_errors_total),
    );

    // Per-method request/error families (stable label set).
    out.push_str("# HELP dd_contract_service_rpc_requests_by_method_total Solana JSON-RPC requests sent by low-cardinality method.\n# TYPE dd_contract_service_rpc_requests_by_method_total counter\n");
    for method in METRICS_RPC_METHODS {
        let (requests, _) = rpc_method_counters(m, method);
        out.push_str(&format!(
            "dd_contract_service_rpc_requests_by_method_total{{rpc_method=\"{method}\"}} {}\n",
            load(requests)
        ));
    }
    out.push_str("# HELP dd_contract_service_rpc_errors_by_method_total Solana JSON-RPC failures by low-cardinality method.\n# TYPE dd_contract_service_rpc_errors_by_method_total counter\n");
    for method in METRICS_RPC_METHODS {
        let (_, errors) = rpc_method_counters(m, method);
        out.push_str(&format!(
            "dd_contract_service_rpc_errors_by_method_total{{rpc_method=\"{method}\"}} {}\n",
            load(errors)
        ));
    }

    // Confirmation outcomes by terminal status.
    out.push_str("# HELP dd_contract_service_confirmations_total Settlement/resolution signature confirmation outcomes by terminal status.\n# TYPE dd_contract_service_confirmations_total counter\n");
    for (outcome, value) in [
        ("confirmed", load(&m.confirmations_confirmed_total)),
        ("finalized", load(&m.confirmations_finalized_total)),
        ("failed", load(&m.confirmations_failed_total)),
        ("pending", load(&m.confirmations_pending_total)),
        ("deferred", load(&m.confirmations_deferred_total)),
    ] {
        out.push_str(&format!(
            "dd_contract_service_confirmations_total{{outcome=\"{outcome}\"}} {value}\n"
        ));
    }

    push_counter(
        &mut out,
        "dd_contract_service_nats_messages_total",
        "NATS messages received across subscribed subjects.",
        load(&m.nats_messages_total),
    );
    push_counter(
        &mut out,
        "dd_contract_service_nats_payload_rejected_total",
        "NATS messages rejected before processing.",
        load(&m.nats_payload_rejected_total),
    );
    out.push_str("# HELP dd_contract_service_nats_published_total NATS messages published by subject kind.\n# TYPE dd_contract_service_nats_published_total counter\n");
    out.push_str(&format!(
        "dd_contract_service_nats_published_total{{subject_kind=\"result\"}} {}\n",
        load(&m.nats_results_published_total)
    ));
    out.push_str(&format!(
        "dd_contract_service_nats_published_total{{subject_kind=\"event\"}} {}\n",
        load(&m.nats_events_published_total)
    ));
    out.push_str(&format!(
        "dd_contract_service_nats_published_total{{subject_kind=\"critical\"}} {}\n",
        load(&m.nats_critical_events_published_total)
    ));
    push_counter(
        &mut out,
        "dd_contract_service_nats_publish_errors_total",
        "NATS publish failures observed.",
        load(&m.nats_publish_errors_total),
    );
    push_counter(
        &mut out,
        "dd_contract_service_send_blocked_total",
        "Raw transaction sends blocked by policy.",
        load(&m.send_blocked_total),
    );
    push_counter(
        &mut out,
        "dd_contract_service_send_auth_failures_total",
        "Send/settlement attempts rejected by an auth header check.",
        load(&m.send_auth_failures_total),
    );
    push_counter(
        &mut out,
        "dd_contract_service_errors_total",
        "Contract service errors observed.",
        load(&m.errors_total),
    );
    // Blockchain feature-suite counters share the same exposition.
    state.blockchain.render_metrics(&mut out);
    state.coordination.render_metrics(&mut out);
    state.solana_features.render_metrics(&mut out);

    format!(
        "# HELP dd_contract_service_info Static service configuration labels for the Solana contract service.\n\
# TYPE dd_contract_service_info gauge\n\
dd_contract_service_info{{cluster=\"{}\",send_enabled=\"{}\",skip_preflight_allowed=\"{}\",settlement_enabled=\"{}\",resolution_enabled=\"{}\",mainnet_settlement_enabled=\"{}\",coordination_enabled=\"{}\",formal_methods_enabled=\"{}\"}} 1\n{out}",
        state.default_cluster,
        bool_label(state.send_enabled),
        bool_label(state.allow_skip_preflight),
        bool_label(state.settlement_enabled),
        bool_label(state.resolution_enabled),
        bool_label(state.mainnet_settlement_enabled),
        bool_label(state.coordination.enabled()),
        bool_label(state.solana_features.formal_enabled()),
    )
}

pub(crate) async fn metrics(State(state): State<AppState>) -> Response {
    state
        .metrics
        .http_requests_total
        .fetch_add(1, Ordering::Relaxed);
    let body = metrics_body(&state);
    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], body).into_response()
}
