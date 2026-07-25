use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use serde::Serialize;
use serde_json::{json, Value};

use crate::metrics::Metrics;
use crate::rpc::solana_rpc;
use crate::state::{
    AppState, MAX_CONFIRM_POLLERS_IN_FLIGHT, MAX_CONFIRM_POLLS, MAX_CONFIRM_POLL_INTERVAL_MS,
    MAX_CONFIRM_TIMEOUT_MS, MIN_CONFIRM_POLL_INTERVAL_MS,
};

/// RAII slot for one in-flight confirmation poller. Decrements the service-wide
/// counter on drop so a panicking or early-returning task can't leak a slot.
pub(crate) struct ConfirmSlot(Arc<AtomicU64>);

impl ConfirmSlot {
    /// Reserves a slot if the in-flight count is under the cap, else `None`.
    pub(crate) fn try_acquire(counter: &Arc<AtomicU64>) -> Option<Self> {
        let prior = counter.fetch_add(1, Ordering::AcqRel);
        if prior >= MAX_CONFIRM_POLLERS_IN_FLIGHT {
            counter.fetch_sub(1, Ordering::AcqRel);
            None
        } else {
            Some(ConfirmSlot(counter.clone()))
        }
    }
}

impl Drop for ConfirmSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConfirmOutcome {
    pub(crate) signature: String,
    pub(crate) status: &'static str,
    pub(crate) target_commitment: String,
    pub(crate) reached: bool,
    pub(crate) polls: u32,
    pub(crate) elapsed_ms: u128,
    pub(crate) slot: Option<u64>,
    pub(crate) confirmation_status: Option<String>,
    pub(crate) error: Option<Value>,
}

/// Validates a confirmation commitment target. Only `confirmed` and
/// `finalized` are valid landing targets; `processed` is not durable.
pub(crate) fn normalize_confirm_commitment(input: Option<&str>) -> Result<String, String> {
    let value = input
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("confirmed");
    match value.to_ascii_lowercase().as_str() {
        "confirmed" => Ok("confirmed".to_string()),
        "finalized" => Ok("finalized".to_string()),
        _ => Err(format!(
            "targetCommitment must be confirmed or finalized: {value}"
        )),
    }
}

pub(crate) fn commitment_rank(status: &str) -> u8 {
    match status {
        "processed" => 1,
        "confirmed" => 2,
        "finalized" => 3,
        _ => 0,
    }
}

pub(crate) fn record_confirm_outcome(metrics: &Metrics, status: &str) {
    let counter = match status {
        "confirmed" => &metrics.confirmations_confirmed_total,
        "finalized" => &metrics.confirmations_finalized_total,
        "failed" => &metrics.confirmations_failed_total,
        _ => &metrics.confirmations_pending_total,
    };
    counter.fetch_add(1, Ordering::Relaxed);
}

/// Polls `getSignatureStatuses` until the signature reaches the target
/// commitment, fails on-chain, or the bounded timeout elapses.
pub(crate) async fn confirm_signature(
    state: &AppState,
    signature: &str,
    target_commitment: &str,
    timeout_ms: u64,
    poll_interval_ms: u64,
) -> ConfirmOutcome {
    let interval = poll_interval_ms
        .clamp(MIN_CONFIRM_POLL_INTERVAL_MS, MAX_CONFIRM_POLL_INTERVAL_MS)
        .max(1);
    let timeout = timeout_ms.clamp(interval, MAX_CONFIRM_TIMEOUT_MS);
    let max_polls = ((timeout / interval) as u32 + 1).min(MAX_CONFIRM_POLLS);
    let target_rank = commitment_rank(target_commitment);
    let started = Instant::now();

    let mut polls = 0u32;
    let mut last_confirmation_status: Option<String> = None;
    let mut last_slot: Option<u64> = None;

    while polls < max_polls {
        polls += 1;
        let params = json!([[signature], { "searchTransactionHistory": true }]);
        match solana_rpc(state, "getSignatureStatuses", params).await {
            Ok(result) => {
                let entry = result.pointer("/value/0").cloned().unwrap_or(Value::Null);
                if entry.is_object() {
                    last_slot = entry.get("slot").and_then(Value::as_u64).or(last_slot);
                    if let Some(error) = entry.get("err") {
                        if !error.is_null() {
                            let outcome = ConfirmOutcome {
                                signature: signature.to_string(),
                                status: "failed",
                                target_commitment: target_commitment.to_string(),
                                reached: false,
                                polls,
                                elapsed_ms: started.elapsed().as_millis(),
                                slot: last_slot,
                                confirmation_status: entry
                                    .get("confirmationStatus")
                                    .and_then(Value::as_str)
                                    .map(str::to_string),
                                error: Some(error.clone()),
                            };
                            record_confirm_outcome(&state.metrics, "failed");
                            return outcome;
                        }
                    }
                    let confirmation_status = entry
                        .get("confirmationStatus")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    if let Some(status) = &confirmation_status {
                        last_confirmation_status = Some(status.clone());
                        if commitment_rank(status) >= target_rank {
                            let status_label: &'static str = if target_commitment == "finalized" {
                                "finalized"
                            } else {
                                "confirmed"
                            };
                            record_confirm_outcome(&state.metrics, status_label);
                            return ConfirmOutcome {
                                signature: signature.to_string(),
                                status: status_label,
                                target_commitment: target_commitment.to_string(),
                                reached: true,
                                polls,
                                elapsed_ms: started.elapsed().as_millis(),
                                slot: last_slot,
                                confirmation_status,
                                error: None,
                            };
                        }
                    }
                }
            }
            Err(_) => {
                // Transient RPC error is already counted/logged in solana_rpc;
                // keep polling until the bounded budget is exhausted.
            }
        }
        if polls < max_polls {
            tokio::time::sleep(Duration::from_millis(interval)).await;
        }
    }

    record_confirm_outcome(&state.metrics, "pending");
    ConfirmOutcome {
        signature: signature.to_string(),
        status: "pending",
        target_commitment: target_commitment.to_string(),
        reached: false,
        polls,
        elapsed_ms: started.elapsed().as_millis(),
        slot: last_slot,
        confirmation_status: last_confirmation_status,
        error: None,
    }
}

/// Synthetic outcome returned when the service-wide confirmation-poller cap is
/// reached. No RPC is performed; the caller can re-check via `/confirm`.
pub(crate) fn deferred_confirm_outcome(signature: &str, target_commitment: &str) -> ConfirmOutcome {
    ConfirmOutcome {
        signature: signature.to_string(),
        status: "deferred",
        target_commitment: target_commitment.to_string(),
        reached: false,
        polls: 0,
        elapsed_ms: 0,
        slot: None,
        confirmation_status: None,
        error: Some(json!(
            "confirmation deferred: service confirmation capacity reached; re-check via POST /confirm"
        )),
    }
}

/// Runs `confirm_signature` under a service-wide poller slot. When the cap is
/// reached it sheds gracefully (no RPC) rather than amplifying upstream load.
pub(crate) async fn bounded_confirm(
    state: &AppState,
    signature: &str,
    target_commitment: &str,
    timeout_ms: u64,
    poll_interval_ms: u64,
) -> ConfirmOutcome {
    match ConfirmSlot::try_acquire(&state.confirm_in_flight) {
        Some(_slot) => {
            confirm_signature(
                state,
                signature,
                target_commitment,
                timeout_ms,
                poll_interval_ms,
            )
            .await
        }
        None => {
            state
                .metrics
                .confirmations_deferred_total
                .fetch_add(1, Ordering::Relaxed);
            deferred_confirm_outcome(signature, target_commitment)
        }
    }
}
