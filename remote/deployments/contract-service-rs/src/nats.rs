use std::{sync::atomic::Ordering, time::Duration};

use futures_util::StreamExt;
use serde_json::{json, Map, Value};

use crate::confirm::{bounded_confirm, confirm_signature, ConfirmSlot};
use crate::rpc::solana_rpc;
use crate::settlement::{
    resolve_confirm_target, validate_settlement_core, ConfirmOptions, ResolutionRequest,
    SettlementCore, SettlementRequest,
};
use crate::shared::{
    explicit_request_id, log_error, log_info, log_warn, now_ms, request_id, structured_log_record,
};
use crate::state::{
    AppState, DEFAULT_CONFIRM_POLL_INTERVAL_MS, DEFAULT_CONFIRM_TIMEOUT_MS,
    MAX_CONFIRM_POLLERS_IN_FLIGHT, MAX_NATS_PAYLOAD_BYTES, RESOLUTION_SCHEMA_VERSION, SERVICE_NAME,
    SETTLEMENT_SCHEMA_VERSION,
};
use crate::validation::{
    send_params, simulate_params, validate_contract_request, validate_signature, ContractRequest,
};

async fn publish_contract_result(state: &AppState, payload: Value) {
    let Some(nats) = &state.nats else {
        return;
    };
    let Ok(encoded) = serde_json::to_vec(&payload) else {
        state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
        log_error(
            "contract-result-serialize-failed",
            "Contract validation result could not be serialized for NATS.",
            json!({}),
        );
        return;
    };
    if let Err(error) = nats
        .publish(state.result_subject.clone(), encoded.into())
        .await
    {
        state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
        state
            .metrics
            .nats_publish_errors_total
            .fetch_add(1, Ordering::Relaxed);
        publish_runtime_critical_event(
            state,
            "contract-result-publish-failed",
            "Contract validation result NATS publish failed.",
            json!({
                "subject": &state.result_subject,
                "error": error.to_string(),
            }),
        )
        .await;
    } else {
        state
            .metrics
            .nats_results_published_total
            .fetch_add(1, Ordering::Relaxed);
    }
}

pub(crate) async fn publish_settlement_outcome(state: &AppState, payload: Value) {
    let Some(nats) = &state.nats else {
        return;
    };
    let Ok(encoded) = serde_json::to_vec(&payload) else {
        state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
        log_error(
            "contract-settlement-result-serialize-failed",
            "Settlement/resolution outcome could not be serialized for NATS.",
            json!({}),
        );
        return;
    };
    if let Err(error) = nats
        .publish(state.settlement_result_subject.clone(), encoded.into())
        .await
    {
        state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
        state
            .metrics
            .nats_publish_errors_total
            .fetch_add(1, Ordering::Relaxed);
        publish_runtime_critical_event(
            state,
            "contract-settlement-result-publish-failed",
            "Settlement/resolution outcome NATS publish failed.",
            json!({
                "subject": &state.settlement_result_subject,
                "error": error.to_string(),
            }),
        )
        .await;
    } else {
        state
            .metrics
            .nats_results_published_total
            .fetch_add(1, Ordering::Relaxed);
    }
}

/// Publish-only helper for the blockchain feature suite: fire-and-forget a JSON
/// payload to a fixed subject (index events, MEV alerts, bridge attestations),
/// counting the same NATS metrics as the contract publish paths. No-op when NATS
/// is not connected.
pub(crate) async fn publish_blockchain_event(state: &AppState, subject: &str, payload: Value) {
    let Some(nats) = &state.nats else {
        return;
    };
    let Ok(encoded) = serde_json::to_vec(&payload) else {
        state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
        return;
    };
    if nats
        .publish(subject.to_string(), encoded.into())
        .await
        .is_err()
    {
        state
            .metrics
            .nats_publish_errors_total
            .fetch_add(1, Ordering::Relaxed);
    } else {
        state
            .metrics
            .nats_results_published_total
            .fetch_add(1, Ordering::Relaxed);
    }
}

pub(crate) async fn publish_contract_event(
    state: &AppState,
    event_type: &str,
    request_id: &str,
    ok: bool,
) {
    let Some(nats) = &state.nats else {
        return;
    };
    let payload = json!({
        "type": event_type,
        "source": "dd-contract-service",
        "requestId": request_id,
        "ok": ok,
        "chain": "solana",
        "atMs": now_ms(),
    });
    if let Err(error) = nats
        .publish(state.event_subject.clone(), payload.to_string().into())
        .await
    {
        state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
        state
            .metrics
            .nats_publish_errors_total
            .fetch_add(1, Ordering::Relaxed);
        log_warn(
            "contract-event-publish-failed",
            "Contract lifecycle event NATS publish failed.",
            json!({
                "subject": &state.event_subject,
                "eventType": event_type,
                "requestId": request_id,
                "error": error.to_string(),
            }),
        );
    } else {
        state
            .metrics
            .nats_events_published_total
            .fetch_add(1, Ordering::Relaxed);
    }
}

pub(crate) async fn publish_runtime_critical_event(
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
                    "contract-critical-event-publish-failed",
                    "Contract service critical event NATS publish failed.",
                    json!({
                        "subject": &state.critical_event_subject,
                        "eventName": event_name,
                        "error": error.to_string(),
                    }),
                );
            }
        },
        Err(error) => {
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            log_error(
                "contract-critical-event-serialize-failed",
                "Contract service critical event payload serialization failed.",
                json!({
                    "eventName": event_name,
                    "error": error.to_string(),
                }),
            );
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum NatsKind {
    Validate,
    Settle,
    Resolve,
    EscrowResults,
}

impl NatsKind {
    fn label(self) -> &'static str {
        match self {
            NatsKind::Validate => "validate",
            NatsKind::Settle => "settle",
            NatsKind::Resolve => "resolve",
            NatsKind::EscrowResults => "escrow-results",
        }
    }
}

pub(crate) async fn run_nats_loop(
    state: AppState,
    subject: String,
    queue_group: Option<String>,
    kind: NatsKind,
) {
    let Some(nats) = state.nats.clone() else {
        log_info(
            "contract-nats-loop-disabled",
            "Contract service NATS loop is disabled because NATS_URL is not configured.",
            json!({}),
        );
        return;
    };
    log_info(
        "contract-nats-loop-starting",
        "Contract service NATS subscription loop is starting.",
        json!({
            "subject": &subject,
            "queueGroup": &queue_group,
            "kind": kind.label(),
            "natsSettlementEnabled": state.nats_settlement_enabled,
        }),
    );
    loop {
        let subscribe = match &queue_group {
            Some(group) => nats.queue_subscribe(subject.clone(), group.clone()).await,
            None => nats.subscribe(subject.clone()).await,
        };
        let mut subscription = match subscribe {
            Ok(subscription) => subscription,
            Err(error) => {
                state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                publish_runtime_critical_event(
                    &state,
                    "contract-nats-subscribe-failed",
                    "Contract service could not subscribe to a NATS subject; retrying in 5s.",
                    json!({ "subject": &subject, "kind": kind.label(), "error": error.to_string() }),
                )
                .await;
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
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
                    "contract-nats-payload-too-large",
                    "Contract service rejected an oversized NATS message.",
                    json!({
                        "kind": kind.label(),
                        "payloadBytes": payload.len(),
                        "maxPayloadBytes": MAX_NATS_PAYLOAD_BYTES,
                    }),
                )
                .await;
                continue;
            }
            match kind {
                NatsKind::Validate => process_nats_validate(&state, &payload).await,
                NatsKind::Settle => process_nats_settle(&state, &payload).await,
                NatsKind::Resolve => process_nats_resolve(&state, &payload).await,
                NatsKind::EscrowResults => process_nats_escrow_result(&state, &payload).await,
            }
        }

        // The stream only ends when the subscription is closed or the connection is
        // torn down without async-nats restoring it. That silently kills a consumer,
        // so alert and re-subscribe instead of dying quietly.
        publish_runtime_critical_event(
            &state,
            "contract-nats-loop-ended",
            "Contract service NATS subscription loop ended unexpectedly; re-subscribing in 5s.",
            json!({ "subject": &subject, "kind": kind.label() }),
        )
        .await;
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

fn nats_payload_invalid(state: &AppState, kind: NatsKind, error: &str) {
    state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .nats_payload_rejected_total
        .fetch_add(1, Ordering::Relaxed);
    log_warn(
        "contract-nats-payload-invalid",
        "Contract service rejected an invalid NATS message.",
        json!({ "kind": kind.label(), "error": error }),
    );
}

async fn process_nats_validate(state: &AppState, payload: &[u8]) {
    match serde_json::from_slice::<ContractRequest>(payload) {
        Ok(request) => {
            state
                .metrics
                .validations_total
                .fetch_add(1, Ordering::Relaxed);
            let request_id = request_id(request.request_id.as_ref(), "contract-validation");
            let result = match validate_contract_request(&request, &state.default_cluster) {
                Ok(response) => json!({
                    "messageKind": "solana.contract.validation.result",
                    "source": "dd-contract-service",
                    "result": response
                }),
                Err(errors) => {
                    state
                        .metrics
                        .validation_errors_total
                        .fetch_add(1, Ordering::Relaxed);
                    state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                    json!({
                        "messageKind": "solana.contract.validation.result",
                        "source": "dd-contract-service",
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
            publish_contract_result(state, result).await;
            publish_contract_event(state, "solana.contract.validation", &request_id, ok).await;
        }
        Err(error) => nats_payload_invalid(state, NatsKind::Validate, &error.to_string()),
    }
}

/// Drives validate -> simulate -> (optional broadcast+confirm) -> publish for a
/// settlement-style NATS message. Broadcast only happens when
/// `CONTRACT_NATS_SETTLEMENT_ENABLED=true`, because NATS messages carry no auth
/// header; otherwise the service validates, simulates, and reports.
#[allow(clippy::too_many_arguments)]
async fn nats_settlement_flow(
    state: &AppState,
    req_id: &str,
    schema_version: &str,
    message_kind: &str,
    event_type: &str,
    core: &SettlementCore,
    confirm: &Option<ConfirmOptions>,
    idem_key: Option<String>,
    mut extra: Map<String, Value>,
) {
    let (cluster, encoding, decoded_bytes) =
        match validate_settlement_core(core, &state.default_cluster) {
            Ok(validated) => validated,
            Err(errors) => {
                let mut outcome = base_settlement_outcome(
                    message_kind,
                    schema_version,
                    req_id,
                    &state.default_cluster,
                    false,
                    "rejected",
                );
                outcome.append(&mut extra);
                outcome.insert("errors".to_string(), json!(errors));
                publish_settlement_outcome(state, Value::Object(outcome)).await;
                publish_contract_event(state, event_type, req_id, false).await;
                return;
            }
        };

    // Always simulate for visibility, regardless of broadcast policy.
    let sim_tx = core.tx_request(true);
    let simulation = match simulate_params(&sim_tx, encoding) {
        Ok(params) => solana_rpc(state, "simulateTransaction", params)
            .await
            .unwrap_or_else(|error| json!({ "error": error })),
        Err(error) => json!({ "error": error }),
    };

    let mut outcome = base_settlement_outcome(
        message_kind,
        schema_version,
        req_id,
        &cluster,
        false,
        "validated",
    );
    outcome.append(&mut extra);
    outcome.insert("encoding".to_string(), json!(encoding));
    outcome.insert("transactionBytes".to_string(), json!(decoded_bytes));
    outcome.insert("simulation".to_string(), simulation);

    if !state.nats_settlement_enabled {
        outcome.insert("broadcast".to_string(), json!(false));
        outcome.insert(
            "note".to_string(),
            json!("NATS-initiated broadcast is disabled; set CONTRACT_NATS_SETTLEMENT_ENABLED=true to broadcast"),
        );
        publish_settlement_outcome(state, Value::Object(outcome)).await;
        publish_contract_event(state, event_type, req_id, true).await;
        return;
    }

    // Broadcast path: guard double-broadcast only on explicit request ids
    // (an absent id must not collapse distinct messages onto one key).
    if let Some(key) = &idem_key {
        if !state.claim_idempotency_key(key) {
            state
                .metrics
                .settlement_idempotent_hits_total
                .fetch_add(1, Ordering::Relaxed);
            outcome.insert("idempotent".to_string(), json!(true));
            outcome.insert("broadcast".to_string(), json!(false));
            publish_settlement_outcome(state, Value::Object(outcome)).await;
            return;
        }
    }
    let release = |state: &AppState| {
        if let Some(key) = &idem_key {
            state.release_idempotency_key(key);
        }
    };

    let send_tx = core.tx_request(false);
    let send = match send_params(&send_tx, encoding, state.allow_skip_preflight) {
        Ok(params) => params,
        Err(error) => {
            release(state);
            outcome.insert("broadcast".to_string(), json!(false));
            outcome.insert("error".to_string(), json!(error));
            publish_settlement_outcome(state, Value::Object(outcome)).await;
            publish_contract_event(state, event_type, req_id, false).await;
            return;
        }
    };
    match solana_rpc(state, "sendTransaction", send).await {
        Ok(signature_value) => {
            let signature = signature_value.as_str().unwrap_or_default().to_string();
            if signature.is_empty() {
                release(state);
                outcome.insert("broadcast".to_string(), json!(false));
                outcome.insert(
                    "error".to_string(),
                    json!("sendTransaction did not return a signature"),
                );
                publish_settlement_outcome(state, Value::Object(outcome)).await;
                publish_contract_event(state, event_type, req_id, false).await;
                return;
            }
            let (target, timeout_ms, poll_ms) =
                resolve_confirm_target(confirm).unwrap_or_else(|_| {
                    (
                        "confirmed".to_string(),
                        DEFAULT_CONFIRM_TIMEOUT_MS,
                        DEFAULT_CONFIRM_POLL_INTERVAL_MS,
                    )
                });
            let confirmation =
                bounded_confirm(state, &signature, &target, timeout_ms, poll_ms).await;
            let reached = confirmation.reached;
            outcome.insert("ok".to_string(), json!(reached));
            outcome.insert("status".to_string(), json!("broadcast"));
            outcome.insert("broadcast".to_string(), json!(true));
            outcome.insert("signature".to_string(), json!(signature));
            outcome.insert("confirmation".to_string(), json!(confirmation));
            publish_settlement_outcome(state, Value::Object(outcome)).await;
            publish_contract_event(state, event_type, req_id, reached).await;
        }
        Err(error) => {
            release(state);
            state.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            outcome.insert("broadcast".to_string(), json!(false));
            outcome.insert("error".to_string(), json!(error.clone()));
            publish_settlement_outcome(state, Value::Object(outcome)).await;
            publish_runtime_critical_event(
                state,
                "contract-nats-settlement-send-failed",
                "NATS settlement broadcast failed.",
                json!({ "requestId": req_id, "messageKind": message_kind, "error": error }),
            )
            .await;
        }
    }
}

fn base_settlement_outcome(
    message_kind: &str,
    schema_version: &str,
    req_id: &str,
    cluster: &str,
    ok: bool,
    status: &str,
) -> Map<String, Value> {
    let mut map = Map::new();
    map.insert("messageKind".to_string(), json!(message_kind));
    map.insert("source".to_string(), json!(SERVICE_NAME));
    map.insert("ok".to_string(), json!(ok));
    map.insert("status".to_string(), json!(status));
    map.insert("requestId".to_string(), json!(req_id));
    map.insert("schemaVersion".to_string(), json!(schema_version));
    map.insert("cluster".to_string(), json!(cluster));
    map.insert("generatedAtMs".to_string(), json!(now_ms()));
    map
}

async fn process_nats_settle(state: &AppState, payload: &[u8]) {
    match serde_json::from_slice::<SettlementRequest>(payload) {
        Ok(request) => {
            state
                .metrics
                .settlements_total
                .fetch_add(1, Ordering::Relaxed);
            let req_id = request_id(request.request_id.as_ref(), "contract-settlement");
            if request.schema_version != SETTLEMENT_SCHEMA_VERSION {
                state
                    .metrics
                    .settlement_errors_total
                    .fetch_add(1, Ordering::Relaxed);
                nats_payload_invalid(
                    state,
                    NatsKind::Settle,
                    &format!("schemaVersion must be {SETTLEMENT_SCHEMA_VERSION}"),
                );
                return;
            }
            let mut extra = Map::new();
            extra.insert("kind".to_string(), json!("settlement"));
            extra.insert("action".to_string(), json!(request.action.as_str()));
            extra.insert("escrowId".to_string(), json!(request.escrow_id));
            extra.insert("contractId".to_string(), json!(request.contract_id));
            let idem_key = explicit_request_id(request.request_id.as_ref())
                .map(|key| format!("nats:settle:{key}"));
            nats_settlement_flow(
                state,
                &req_id,
                SETTLEMENT_SCHEMA_VERSION,
                "solana.settlement.outcome",
                "solana.contract.settlement",
                &request.core(),
                &request.confirm,
                idem_key,
                extra,
            )
            .await;
        }
        Err(error) => nats_payload_invalid(state, NatsKind::Settle, &error.to_string()),
    }
}

async fn process_nats_resolve(state: &AppState, payload: &[u8]) {
    match serde_json::from_slice::<ResolutionRequest>(payload) {
        Ok(request) => {
            state
                .metrics
                .resolutions_total
                .fetch_add(1, Ordering::Relaxed);
            let req_id = request_id(request.request_id.as_ref(), "contract-resolution");
            let mut schema_errors = Vec::new();
            if request.schema_version != RESOLUTION_SCHEMA_VERSION {
                schema_errors.push(format!("schemaVersion must be {RESOLUTION_SCHEMA_VERSION}"));
            }
            if !request.decision.allowed_actions().contains(&request.action) {
                schema_errors.push(format!(
                    "decision {} does not permit settlement action {}",
                    request.decision.as_str(),
                    request.action.as_str()
                ));
            }
            if !schema_errors.is_empty() {
                state
                    .metrics
                    .resolution_errors_total
                    .fetch_add(1, Ordering::Relaxed);
                nats_payload_invalid(state, NatsKind::Resolve, &schema_errors.join("; "));
                return;
            }
            let mut extra = Map::new();
            extra.insert("kind".to_string(), json!("resolution"));
            extra.insert("decision".to_string(), json!(request.decision.as_str()));
            extra.insert("action".to_string(), json!(request.action.as_str()));
            extra.insert("escrowId".to_string(), json!(request.escrow_id));
            extra.insert("disputeId".to_string(), json!(request.dispute_id));
            extra.insert("arbiter".to_string(), json!(request.arbiter));
            let idem_key = explicit_request_id(request.request_id.as_ref())
                .map(|key| format!("nats:resolve:{key}"));
            nats_settlement_flow(
                state,
                &req_id,
                RESOLUTION_SCHEMA_VERSION,
                "solana.resolution.outcome",
                "solana.contract.resolution",
                &request.core(),
                &request.confirm,
                idem_key,
                extra,
            )
            .await;
        }
        Err(error) => nats_payload_invalid(state, NatsKind::Resolve, &error.to_string()),
    }
}

/// Verifier surface: confirm a settlement signature carried in an escrow result.
async fn process_nats_escrow_result(state: &AppState, payload: &[u8]) {
    let value = match serde_json::from_slice::<Value>(payload) {
        Ok(value) => value,
        Err(error) => {
            nats_payload_invalid(state, NatsKind::EscrowResults, &error.to_string());
            return;
        }
    };
    // Escrow settlement results may carry an RPC sendTransaction signature.
    let signature = value
        .pointer("/result/signature")
        .or_else(|| value.pointer("/signature"))
        .or_else(|| value.pointer("/result/result"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let Some(signature) = signature.filter(|sig| validate_signature(sig, "signature").is_ok())
    else {
        // Not a settlement result we can confirm; ignore quietly.
        return;
    };
    let req_id = value
        .pointer("/result/requestId")
        .or_else(|| value.pointer("/requestId"))
        .and_then(Value::as_str)
        .unwrap_or("escrow-result")
        .to_string();

    // Confirm to finality off the subscription loop so a slow poll can't
    // head-of-line block draining; bound the concurrent fan-out.
    let Some(slot) = ConfirmSlot::try_acquire(&state.confirm_in_flight) else {
        log_warn(
            "contract-escrow-confirm-shed",
            "Escrow confirmation shed because the in-flight verifier cap was reached.",
            json!({ "requestId": req_id, "maxInFlight": MAX_CONFIRM_POLLERS_IN_FLIGHT }),
        );
        return;
    };
    let task_state = state.clone();
    tokio::spawn(async move {
        let _slot = slot;
        let confirmation = confirm_signature(
            &task_state,
            &signature,
            "finalized",
            DEFAULT_CONFIRM_TIMEOUT_MS,
            DEFAULT_CONFIRM_POLL_INTERVAL_MS,
        )
        .await;
        let outcome = json!({
            "messageKind": "solana.escrow.confirmation",
            "source": SERVICE_NAME,
            "ok": confirmation.reached,
            "status": "verified",
            "requestId": req_id,
            "kind": "escrow-confirmation",
            "signature": signature,
            "confirmation": confirmation,
            "generatedAtMs": now_ms()
        });
        publish_settlement_outcome(&task_state, outcome).await;
    });
}
