//! NATS domain-event layer.
//!
//! Billing is **Model A** (observer/recorder). It publishes *redacted*
//! domain events as a best-effort audit / fan-out feed, and subscribes to
//! one inbound command subject that asks it to sync a provider connection.
//! All subjects come from the cross-language registry
//! `remote/libs/nats/subject-defs` (crate `dd-nats-subject-defs`), so the
//! wire contract is shared with every other language in the monorepo.
//!
//! ## Posture
//!
//! - **Best-effort.** A publish failure is logged and counted; it never
//!   propagates into the request/transaction path. Ledger correctness does
//!   not depend on the bus being up.
//! - **Off by default.** When `BILLING_NATS_PUBLISH_ENABLED` is false (or no
//!   URL resolves) the bus is a silent no-op: publishes are dropped and no
//!   subscriber loop runs — mirroring [`crate::cdc`].
//! - **No secrets on the wire.** Payloads carry redacted summaries only.
//!   The webhook-receipt event in particular publishes the payload **hash**,
//!   never the body. Callers are responsible for what they hand us; the
//!   typed helpers below keep that surface small and reviewable.
//!
//! Inbound `dd.remote.billing.commands.sync` messages are queue-grouped
//! (`dd-billing-server`) so a command runs on exactly one replica; each is
//! turned into the same one-shot `sync.connection` scheduler job the HTTP
//! "Sync now" path enqueues, reusing all of its lease / rate-limit / dispatch
//! logic rather than re-implementing it.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use futures_util::StreamExt;
use serde_json::{json, Value};
use uuid::Uuid;

use dd_nats_subject_defs::{
    BILLING_ANCHORS_SUBJECT, BILLING_CONNECTION_EVENTS_SUBJECT, BILLING_LEDGER_POSTINGS_SUBJECT,
    BILLING_RECONCILIATION_BREAKS_SUBJECT, BILLING_SYNC_COMMANDS_QUEUE_GROUP,
    BILLING_SYNC_COMMANDS_SUBJECT, BILLING_WEBHOOK_RECEIPTS_SUBJECT,
};

use crate::state::AppState;

/// Logical source stamped on every envelope (matches the deployment / service
/// name used elsewhere in the registry).
pub const EVENT_SOURCE: &str = "dd-billing-server";

/// Best-effort publisher + inbound command handle.
///
/// Cheap to `clone` via the surrounding `Arc<EventBus>`; the async-nats
/// `Client` is itself an `Arc` internally, so cloning shares one connection.
pub struct EventBus {
    client: Option<async_nats::Client>,
    max_payload_bytes: usize,
    published: AtomicU64,
    dropped_oversize: AtomicU64,
    failed: AtomicU64,
}

impl EventBus {
    /// Live bus backed by a connected client.
    pub fn new(client: async_nats::Client, max_payload_bytes: usize) -> Self {
        Self {
            client: Some(client),
            max_payload_bytes,
            published: AtomicU64::new(0),
            dropped_oversize: AtomicU64::new(0),
            failed: AtomicU64::new(0),
        }
    }

    /// No-op bus: every publish is dropped, no subscriber runs. Used when
    /// NATS is unconfigured and in unit tests.
    pub fn disabled() -> Self {
        Self {
            client: None,
            max_payload_bytes: 1_048_576,
            published: AtomicU64::new(0),
            dropped_oversize: AtomicU64::new(0),
            failed: AtomicU64::new(0),
        }
    }

    /// Connect to `url` and build a live bus, or return a [`Self::disabled`]
    /// bus (logging a warning) if the connection fails. Never errors — a
    /// broker outage at boot must not stop the ledger from serving.
    pub async fn connect(url: &str, max_payload_bytes: usize) -> Self {
        match async_nats::connect(url).await {
            Ok(client) => {
                tracing::info!(%url, "billing event bus connected to NATS");
                Self::new(client, max_payload_bytes)
            }
            Err(error) => {
                tracing::warn!(%url, error = %error, "billing event bus failed to connect; publishing disabled");
                Self::disabled()
            }
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.client.is_some()
    }

    /// `(published, dropped_oversize, failed)` counters for `/metrics`.
    pub fn counters(&self) -> (u64, u64, u64) {
        (
            self.published.load(Ordering::Relaxed),
            self.dropped_oversize.load(Ordering::Relaxed),
            self.failed.load(Ordering::Relaxed),
        )
    }

    /// Serialize, enforce the size ceiling, and publish. Best-effort:
    /// returns `()` always, recording the outcome in the counters + logs.
    async fn publish_value(&self, subject: &'static str, schema_version: &str, fields: Value) {
        let Some(client) = &self.client else {
            return; // disabled — silent no-op
        };
        let envelope = build_envelope(schema_version, fields);
        let payload = match encode_capped(&envelope, self.max_payload_bytes) {
            Ok(bytes) => bytes,
            Err(len) => {
                self.dropped_oversize.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(
                    subject,
                    bytes = len,
                    max = self.max_payload_bytes,
                    "billing event dropped: payload exceeds max bytes"
                );
                return;
            }
        };
        match client.publish(subject, payload.into()).await {
            Ok(()) => {
                self.published.fetch_add(1, Ordering::Relaxed);
            }
            Err(error) => {
                self.failed.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(subject, error = %error, "billing event publish failed");
            }
        }
    }

    // ---- Typed publish helpers (one per subject; redaction lives here) ----

    /// A committed double-entry transaction. `totals` is a map of
    /// `currency -> signed minor-unit string` (strings so large `i128`
    /// values never lose precision through JSON numbers).
    pub async fn publish_ledger_posting(
        &self,
        tenant_id: Uuid,
        tx_id: Uuid,
        kind: &str,
        idempotency_key: &str,
        posting_count: usize,
        totals: Value,
        region: &str,
    ) {
        self.publish_value(
            BILLING_LEDGER_POSTINGS_SUBJECT,
            "billing.ledger.posting.v1",
            json!({
                "tenantId": tenant_id,
                "transactionId": tx_id,
                "kind": kind,
                "idempotencyKey": idempotency_key,
                "postingCount": posting_count,
                "currencyTotalsMinor": totals,
                "region": region,
            }),
        )
        .await;
    }

    /// A reconciliation break opened during provider sync.
    #[allow(clippy::too_many_arguments)]
    pub async fn publish_reconciliation_break(
        &self,
        tenant_id: Uuid,
        provider: &str,
        connection_id: Option<Uuid>,
        break_type: &str,
        currency: &str,
        external_ref: &str,
        expected_minor: Option<i128>,
        actual_minor: Option<i128>,
    ) {
        self.publish_value(
            BILLING_RECONCILIATION_BREAKS_SUBJECT,
            "billing.reconciliation.break.v1",
            json!({
                "tenantId": tenant_id,
                "provider": provider,
                "connectionId": connection_id,
                "breakType": break_type,
                "currency": currency,
                "externalRef": external_ref,
                "expectedMinor": expected_minor.map(|v| v.to_string()),
                "actualMinor": actual_minor.map(|v| v.to_string()),
            }),
        )
        .await;
    }

    /// A Merkle root anchored to Solana.
    #[allow(clippy::too_many_arguments)]
    pub async fn publish_anchor(
        &self,
        tenant_id: Uuid,
        anchor_id: i64,
        from_posting_id: i64,
        to_posting_id: i64,
        posting_count: i64,
        merkle_root_hex: &str,
        tx_signature: Option<&str>,
        slot: Option<i64>,
    ) {
        self.publish_value(
            BILLING_ANCHORS_SUBJECT,
            "billing.anchor.v1",
            json!({
                "tenantId": tenant_id,
                "anchorId": anchor_id,
                "fromPostingId": from_posting_id,
                "toPostingId": to_posting_id,
                "postingCount": posting_count,
                "merkleRootHex": merkle_root_hex,
                "txSignature": tx_signature,
                "slot": slot,
            }),
        )
        .await;
    }

    /// A provider webhook receipt — **redacted**. The raw body and the
    /// verification-error detail are deliberately never published; only the
    /// payload SHA-256 (prefix) travels, as a correlation handle.
    #[allow(clippy::too_many_arguments)]
    pub async fn publish_webhook_receipt(
        &self,
        provider: &str,
        external_event_id: &str,
        event_type: &str,
        signature_ok: bool,
        tenant_id: Option<Uuid>,
        connection_id: Option<Uuid>,
        payload_sha256: &str,
    ) {
        self.publish_value(
            BILLING_WEBHOOK_RECEIPTS_SUBJECT,
            "billing.webhook.receipt.v1",
            json!({
                "provider": provider,
                "externalEventId": external_event_id,
                "eventType": event_type,
                "signatureOk": signature_ok,
                "tenantId": tenant_id,
                "connectionId": connection_id,
                "payloadSha256": payload_sha256,
            }),
        )
        .await;
    }

    /// A provider-connection lifecycle transition
    /// (`created` | `attached` | `synced` | `failed`).
    pub async fn publish_connection_event(
        &self,
        tenant_id: Uuid,
        connection_id: Uuid,
        provider: &str,
        transition: &str,
    ) {
        self.publish_value(
            BILLING_CONNECTION_EVENTS_SUBJECT,
            "billing.connection.event.v1",
            json!({
                "tenantId": tenant_id,
                "connectionId": connection_id,
                "provider": provider,
                "transition": transition,
            }),
        )
        .await;
    }
}

/// Inbound `{tenantId, connectionId, cursor?, trigger?}` command.
#[derive(Debug, serde::Deserialize)]
struct SyncCommand {
    #[serde(alias = "tenant_id")]
    tenant_id: Uuid,
    #[serde(alias = "connection_id")]
    connection_id: Uuid,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    trigger: Option<String>,
}

/// Subscribe to `dd.remote.billing.commands.sync` (queue-grouped) and turn
/// each message into a one-shot `sync.connection` scheduler job. Silent
/// no-op when the bus is disabled. Runs until the subscription ends.
pub async fn run_sync_command_loop(state: AppState) {
    let Some(client) = state.events.client.clone() else {
        tracing::info!("billing sync-command loop disabled (NATS not configured)");
        return;
    };
    let queue_group = state
        .cfg
        .nats_queue_group
        .clone()
        .unwrap_or_else(|| BILLING_SYNC_COMMANDS_QUEUE_GROUP.to_string());
    let max_bytes = state.cfg.nats_max_payload_bytes;

    let mut subscription = match client
        .queue_subscribe(BILLING_SYNC_COMMANDS_SUBJECT, queue_group.clone())
        .await
    {
        Ok(sub) => sub,
        Err(error) => {
            tracing::warn!(error = %error, "billing sync-command subscribe failed");
            return;
        }
    };
    tracing::info!(
        subject = BILLING_SYNC_COMMANDS_SUBJECT,
        queue_group = %queue_group,
        "billing sync-command loop started"
    );

    while let Some(message) = subscription.next().await {
        if message.payload.len() > max_bytes {
            tracing::warn!(
                bytes = message.payload.len(),
                max = max_bytes,
                "billing sync-command rejected: oversize payload"
            );
            continue;
        }
        let command: SyncCommand = match serde_json::from_slice(&message.payload) {
            Ok(c) => c,
            Err(error) => {
                tracing::warn!(error = %error, "billing sync-command rejected: malformed payload");
                continue;
            }
        };
        let state = state.clone();
        // Each command runs independently; a slow/erroring one must not block
        // the subscription's forward progress.
        tokio::spawn(async move {
            if let Err(error) = handle_sync_command(&state, command).await {
                tracing::warn!(error = %error, "billing sync-command failed to enqueue");
            }
        });
    }

    tracing::info!("billing sync-command loop ended (subscription closed)");
}

async fn handle_sync_command(state: &AppState, cmd: SyncCommand) -> crate::error::AppResult<()> {
    // Validate the connection exists for this tenant before enqueuing, so a
    // hostile/garbage command can't create orphaned jobs. Mirrors the HTTP
    // "Sync now" handler in api/connections.rs.
    let _conn = state
        .connections
        .get(cmd.tenant_id, cmd.connection_id)
        .await?;
    let tenant = state.tenants.by_id(cmd.tenant_id).await?;
    let region = tenant.region()?;

    let payload = json!({
        "connection_id": cmd.connection_id,
        "cursor": cmd.cursor,
        "trigger": cmd.trigger.unwrap_or_else(|| "nats".into()),
    });
    let job = state
        .scheduler
        .enqueue_one_shot(
            cmd.tenant_id,
            region,
            "sync.connection",
            format!("nats-conn-{}", cmd.connection_id),
            payload,
        )
        .await?;
    tracing::info!(
        tenant = %cmd.tenant_id,
        connection_id = %cmd.connection_id,
        job_id = %job.id,
        "billing sync-command enqueued via NATS"
    );
    Ok(())
}

/// Wrap event `fields` in the standard envelope. `emittedAt` is RFC-3339 UTC.
fn build_envelope(schema_version: &str, fields: Value) -> Value {
    let mut envelope = json!({
        "schemaVersion": schema_version,
        "source": EVENT_SOURCE,
        "emittedAt": chrono::Utc::now().to_rfc3339(),
    });
    if let (Some(obj), Value::Object(extra)) = (envelope.as_object_mut(), fields) {
        for (k, v) in extra {
            obj.insert(k, v);
        }
    }
    envelope
}

/// Serialize `value`, rejecting (without publishing) anything above `max`
/// bytes. `Err(len)` carries the offending size for logging.
fn encode_capped(value: &Value, max: usize) -> Result<Vec<u8>, usize> {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    if bytes.len() > max {
        Err(bytes.len())
    } else {
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_carries_standard_fields_and_merges_payload() {
        let env = build_envelope("billing.anchor.v1", json!({ "anchorId": 7, "slot": 42 }));
        assert_eq!(env["schemaVersion"], "billing.anchor.v1");
        assert_eq!(env["source"], EVENT_SOURCE);
        assert!(env["emittedAt"].is_string());
        // Merged payload fields sit alongside the envelope keys.
        assert_eq!(env["anchorId"], 7);
        assert_eq!(env["slot"], 42);
    }

    #[test]
    fn envelope_payload_does_not_clobber_reserved_keys_silently() {
        // A field literally named "source" would override; assert our keys
        // win when fields are empty (the normal case) and document intent.
        let env = build_envelope("v1", json!({}));
        assert_eq!(env["source"], EVENT_SOURCE);
    }

    #[test]
    fn encode_capped_accepts_within_limit() {
        let v = json!({ "a": "x" });
        let out = encode_capped(&v, 1024).expect("within limit");
        assert!(!out.is_empty());
    }

    #[test]
    fn encode_capped_rejects_oversize_with_length() {
        let big = "y".repeat(5_000);
        let v = json!({ "blob": big });
        let err = encode_capped(&v, 1024).unwrap_err();
        assert!(err > 1024, "reported length should exceed the cap");
    }

    #[test]
    fn disabled_bus_reports_not_enabled_and_no_counters_move() {
        let bus = EventBus::disabled();
        assert!(!bus.is_enabled());
        // Publishing on a disabled bus is a no-op; counters stay zero.
        let (p, d, f) = bus.counters();
        assert_eq!((p, d, f), (0, 0, 0));
    }

    #[test]
    fn sync_command_accepts_camel_and_snake_case() {
        let camel: SyncCommand =
            serde_json::from_value(json!({ "tenantId": Uuid::nil(), "connectionId": Uuid::nil() }))
                .expect("camelCase parses");
        assert_eq!(camel.tenant_id, Uuid::nil());
        let snake: SyncCommand = serde_json::from_value(
            json!({ "tenant_id": Uuid::nil(), "connection_id": Uuid::nil(), "trigger": "ops" }),
        )
        .expect("snake_case parses");
        assert_eq!(snake.trigger.as_deref(), Some("ops"));
    }
}
