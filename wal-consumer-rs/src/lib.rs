//! NATS JetStream client for the `dd-wal-gateway` CDC stream.
//!
//! ## Wire format
//!
//! The gateway publishes one message per row change to subjects shaped
//! `cdc.<schema>.<table>.<op>` where `<op>` is `insert | update | delete`.
//! The payload is JSON with the [`RowChange`] envelope:
//!
//! ```jsonc
//! {
//!   "schemaVersion": "cdc.row.v1",
//!   "schema": "public",
//!   "table": "app_config",
//!   "op": "update",
//!   "lsn": "0/1A3B5C0",
//!   "xid": 12345,
//!   "tsMs": 1736000000000,
//!   "primaryKey": ["id"],
//!   "row":         { "id": "...", "scope": "...", ... },
//!   "previousRow": { "id": "..." }   // null for inserts
//! }
//! ```
//!
//! ## Subscribing
//!
//! ```no_run
//! use dd_wal_consumer::{Subscription, RowChange, ChangeOp};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let nats = async_nats::connect("nats://localhost:4222").await?;
//! let jetstream = async_nats::jetstream::new(nats);
//!
//! Subscription::builder()
//!     .stream("CDC")
//!     .durable_name("trading-server-app-config")
//!     .filter_subject("cdc.public.app_config.>")
//!     .start(&jetstream, move |change: RowChange| async move {
//!         if matches!(change.op, ChangeOp::Update | ChangeOp::Insert) {
//!             println!("app_config changed: {}", change.row);
//!         }
//!     })
//!     .await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Delivery semantics
//!
//! * **At-least-once**: the gateway commits the slot only after a JetStream
//!   ack, but JetStream itself can redeliver if a consumer dies before
//!   acking. Handlers must be idempotent — keying off `(table, primary_key,
//!   lsn)` is usually enough.
//! * **Per-consumer position**: the durable name persists the consumer's
//!   position. Reusing the same durable name across restarts resumes;
//!   using a fresh one starts from the stream's earliest still-retained
//!   message.
//! * **Ordering**: JetStream preserves per-subject order. Cross-subject
//!   order is not guaranteed (though in practice the gateway publishes
//!   in commit order so most cross-table interleavings hold).

use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream::{
    consumer::{pull::Config as PullConfig, AckPolicy, Consumer, DeliverPolicy},
    AckKind, Context, Message,
};
use futures_util::{FutureExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::task::JoinHandle;

pub const SCHEMA_VERSION: &str = "cdc.row.v1";

/// Counters for a running subscription, so the host service can surface CDC
/// health on its Prometheus `/metrics` endpoint (and therefore Grafana).
///
/// This crate deliberately does NOT own an HTTP endpoint or initialize
/// OpenTelemetry — that is the host service's job (`dd-telemetry`). It only
/// counts, and emits `tracing` spans/events that the host's subscriber ships
/// to Loki (as `dd.log.v1` stdout) and OTLP.
#[derive(Debug, Default)]
pub struct ConsumerMetrics {
    /// Envelopes received from JetStream (before decode).
    pub received: AtomicU64,
    /// Envelopes decoded and dispatched to the handler.
    pub handled: AtomicU64,
    /// Envelopes that failed to decode (acked and dropped).
    pub decode_errors: AtomicU64,
    /// Envelopes skipped because `schemaVersion` did not match.
    pub schema_mismatch: AtomicU64,
    /// Handler panics caught (the subscription survives).
    pub handler_panics: AtomicU64,
    /// `AckKind::Progress` heartbeats sent during slow handlers.
    pub ack_progress: AtomicU64,
    /// Ack failures reported by the server.
    pub ack_errors: AtomicU64,
    /// Times the message stream ended and the loop reconnected.
    pub reconnects: AtomicU64,
}

impl ConsumerMetrics {
    /// Render these counters in Prometheus text format. Append the result to
    /// the host service's `/metrics` body. `durable` labels the series so
    /// several subscriptions in one process stay distinguishable.
    pub fn prometheus_text(&self, durable: &str) -> String {
        let label = durable.replace('\\', "\\\\").replace('"', "\\\"");
        let rows = [
            ("received_total", "CDC envelopes received from JetStream.", &self.received),
            ("handled_total", "CDC envelopes dispatched to the handler.", &self.handled),
            ("decode_errors_total", "CDC envelopes that failed to decode.", &self.decode_errors),
            ("schema_mismatch_total", "CDC envelopes skipped for schemaVersion mismatch.", &self.schema_mismatch),
            ("handler_panics_total", "Handler panics caught by the consumer.", &self.handler_panics),
            ("ack_progress_total", "Ack progress heartbeats sent during slow handlers.", &self.ack_progress),
            ("ack_errors_total", "Ack failures reported by the server.", &self.ack_errors),
            ("reconnects_total", "Times the CDC message stream ended and reconnected.", &self.reconnects),
        ];
        let mut out = String::new();
        for (name, help, counter) in rows {
            out.push_str(&format!(
                "# HELP dd_wal_consumer_{name} {help}\n# TYPE dd_wal_consumer_{name} counter\ndd_wal_consumer_{name}{{durable=\"{label}\"}} {}\n",
                counter.load(Ordering::Relaxed)
            ));
        }
        out
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChangeOp {
    Insert,
    Update,
    Delete,
}

impl ChangeOp {
    pub fn as_str(self) -> &'static str {
        match self {
            ChangeOp::Insert => "insert",
            ChangeOp::Update => "update",
            ChangeOp::Delete => "delete",
        }
    }
}

/// One row change. Mirrors the gateway's wire envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RowChange {
    #[serde(default = "default_schema_version")]
    pub schema_version: String,
    pub schema: String,
    pub table: String,
    pub op: ChangeOp,
    pub lsn: String,
    #[serde(default)]
    pub xid: Option<i64>,
    #[serde(default)]
    pub ts_ms: u64,
    #[serde(default)]
    pub source_timestamp: Option<String>,
    #[serde(default)]
    pub primary_key: Vec<String>,
    pub row: Value,
    #[serde(default)]
    pub previous_row: Option<Value>,
}

fn default_schema_version() -> String {
    SCHEMA_VERSION.to_string()
}

impl RowChange {
    /// Convenience: look up the named column from the current `row` field.
    /// For DELETE this returns the identity (primary key) value.
    pub fn column(&self, name: &str) -> Option<&Value> {
        self.row.get(name)
    }

    /// True if this change matches the given fully qualified table.
    pub fn is_table(&self, schema: &str, table: &str) -> bool {
        self.schema == schema && self.table == table
    }
}

/// A subject token must be a bare NATS token: dots would add subject levels
/// and `*`/`>` would widen the subscription — either silently changes what a
/// consumer receives. Panics (these are compile-time-known identifiers in
/// practice; failing fast at startup beats subscribing to the wrong data).
fn assert_subject_token(kind: &str, value: &str) {
    let ok = !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    assert!(
        ok,
        "invalid {kind} {value:?} for a CDC subject: must be non-empty and \
         contain only [A-Za-z0-9_-] (no '.', '*', '>' or whitespace)"
    );
}

/// Build the JetStream subject the gateway publishes to for a given table
/// and operation.
pub fn subject_for(schema: &str, table: &str, op: ChangeOp) -> String {
    assert_subject_token("schema", schema);
    assert_subject_token("table", table);
    format!("cdc.{schema}.{table}.{}", op.as_str())
}

/// Build the wildcard subject for "all ops on this table".
pub fn subject_for_table(schema: &str, table: &str) -> String {
    assert_subject_token("schema", schema);
    assert_subject_token("table", table);
    format!("cdc.{schema}.{table}.>")
}

#[derive(Debug)]
pub enum Error {
    Jetstream(String),
    Decode(serde_json::Error, Vec<u8>),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Jetstream(s) => write!(f, "jetstream error: {s}"),
            // Deliberately length-only: CDC payloads carry row data, which
            // must not leak into logs/error chains via a Display impl.
            Error::Decode(e, raw) => write!(
                f,
                "row envelope decode error: {e}; payload_len={}",
                raw.len()
            ),
        }
    }
}

impl std::error::Error for Error {}

pub struct SubscriptionBuilder {
    stream: String,
    durable_name: String,
    filter_subject: String,
    deliver_policy: DeliverPolicy,
    max_inflight: u32,
    ack_wait: Duration,
    metrics: Option<Arc<ConsumerMetrics>>,
}

impl Default for SubscriptionBuilder {
    fn default() -> Self {
        Self {
            stream: "CDC".to_string(),
            durable_name: String::new(),
            filter_subject: "cdc.>".to_string(),
            deliver_policy: DeliverPolicy::New,
            max_inflight: 256,
            ack_wait: Duration::from_secs(30),
            metrics: None,
        }
    }
}

impl SubscriptionBuilder {
    pub fn stream(mut self, stream: impl Into<String>) -> Self {
        self.stream = stream.into();
        self
    }
    /// Durable name — must be stable across restarts so JetStream remembers
    /// the consumer's position. Convention: `<service>-<purpose>`, e.g.
    /// `trading-server-app-config`.
    pub fn durable_name(mut self, name: impl Into<String>) -> Self {
        self.durable_name = name.into();
        self
    }
    /// JetStream subject filter (supports wildcards).
    pub fn filter_subject(mut self, subject: impl Into<String>) -> Self {
        self.filter_subject = subject.into();
        self
    }
    /// Start from a particular policy. Default: `DeliverPolicy::New`
    /// (only deliver messages published AFTER the consumer is created).
    /// Use `DeliverPolicy::All` for replay-on-boot semantics.
    pub fn deliver_policy(mut self, policy: DeliverPolicy) -> Self {
        self.deliver_policy = policy;
        self
    }
    /// Cap on un-acked messages. Default 256; raise for high-throughput
    /// tables or lower if the handler is slow / memory-sensitive.
    pub fn max_inflight(mut self, n: u32) -> Self {
        self.max_inflight = n;
        self
    }
    /// How long JetStream waits for an ack before redelivering. Default 30s.
    ///
    /// Note this only takes effect when the durable consumer is *created*.
    /// JetStream does not reconfigure an existing durable on `get_or_create`,
    /// so if the consumer already exists with different settings the server's
    /// values win — `start` logs a warning when it detects that drift.
    pub fn ack_wait(mut self, d: Duration) -> Self {
        self.ack_wait = d;
        self
    }

    /// Share a metrics handle so the host service can render CDC counters on
    /// its own Prometheus `/metrics` endpoint. When unset the subscription
    /// still counts internally, the values are just not reachable outside.
    pub fn metrics(mut self, metrics: Arc<ConsumerMetrics>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Start the subscription as a background task. Returns a JoinHandle
    /// that yields if the subscription task ever exits (it normally runs
    /// forever and is dropped at process shutdown).
    ///
    /// The handler receives every successfully-decoded `RowChange`. The
    /// future it returns is awaited before ack — so synchronous handlers
    /// "just work" by returning `async {}` and async handlers naturally
    /// support backpressure.
    ///
    /// Decode errors are logged (via eprintln; or `tracing` if the feature
    /// is enabled) and the message is ack'd anyway — there's nothing the
    /// consumer can do about an undecodable envelope, and not acking would
    /// just redeliver the same poison message.
    pub async fn start<F, Fut>(
        self,
        jetstream: &Context,
        handler: F,
    ) -> Result<JoinHandle<()>, Error>
    where
        F: Fn(RowChange) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        if self.durable_name.is_empty() {
            return Err(Error::Jetstream(
                "Subscription requires a non-empty durable_name".into(),
            ));
        }
        let stream = jetstream
            .get_stream(&self.stream)
            .await
            .map_err(|e| Error::Jetstream(format!("get_stream({}): {e}", self.stream)))?;
        let consumer: Consumer<PullConfig> = stream
            .get_or_create_consumer(
                &self.durable_name,
                PullConfig {
                    durable_name: Some(self.durable_name.clone()),
                    filter_subject: self.filter_subject.clone(),
                    ack_policy: AckPolicy::Explicit,
                    deliver_policy: self.deliver_policy,
                    max_ack_pending: self.max_inflight as i64,
                    ack_wait: self.ack_wait,
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| Error::Jetstream(format!("create_consumer: {e}")))?;
        // `get_or_create_consumer` returns a pre-existing durable unchanged, so
        // builder settings are silently ignored when the consumer already
        // exists with different values. Surface that instead of letting an
        // operator believe a tuning change took effect.
        let effective_ack_wait = consumer.cached_info().config.ack_wait;
        if effective_ack_wait != self.ack_wait {
            log_warn(&format!(
                "wal-consumer[{}] consumer already exists with ack_wait={:?}, ignoring requested {:?}; \
                 delete/recreate the durable to change it",
                self.durable_name, effective_ack_wait, self.ack_wait
            ));
        }
        let metrics = self
            .metrics
            .clone()
            .unwrap_or_else(|| Arc::new(ConsumerMetrics::default()));
        let handler = Arc::new(handler);
        let label = self.durable_name.clone();
        let join = tokio::spawn(async move {
            run_pull_loop(consumer, label, handler, effective_ack_wait, metrics).await;
        });
        Ok(join)
    }

    /// Convenience constructor that returns `Self::default()`.
    pub fn builder() -> Self {
        Self::default()
    }
}

/// Alias so the doc example reads more naturally.
pub struct Subscription;
impl Subscription {
    pub fn builder() -> SubscriptionBuilder {
        SubscriptionBuilder::default()
    }
}

/// Await the handler while extending the JetStream ack deadline with
/// `AckKind::Progress` every `interval`, and contain any panic it throws.
///
/// Without the heartbeat a handler slower than `ack_wait` (default 30s) is
/// treated as stalled and the row is redelivered *while it is still being
/// processed* — duplicate work on every slow handler. Without the panic guard a
/// single bad row would unwind out of the loop and kill the subscription for
/// the life of the process. Returns `Err(())` if the handler panicked.
async fn run_handler_guarded<Fut>(
    msg: &Message,
    interval: Duration,
    metrics: &ConsumerMetrics,
    handler_fut: Fut,
) -> Result<(), ()>
where
    Fut: Future<Output = ()>,
{
    let guarded = std::panic::AssertUnwindSafe(handler_fut).catch_unwind();
    tokio::pin!(guarded);
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await; // the first tick completes immediately; skip it
    loop {
        tokio::select! {
            biased;
            outcome = &mut guarded => return outcome.map_err(|_| ()),
            _ = ticker.tick() => {
                if msg.ack_with(AckKind::Progress).await.is_ok() {
                    metrics.ack_progress.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
}

async fn run_pull_loop<F, Fut>(
    consumer: Consumer<PullConfig>,
    label: String,
    handler: Arc<F>,
    ack_wait: Duration,
    metrics: Arc<ConsumerMetrics>,
) where
    F: Fn(RowChange) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    // Heartbeat at a third of the ack deadline (floored at 1s) so several
    // heartbeats land inside each window even if one is delayed.
    let progress_every = (ack_wait / 3).max(Duration::from_secs(1));
    let log_label = label.clone();
    loop {
        let messages = match consumer.messages().await {
            Ok(stream) => stream,
            Err(error) => {
                log_warn(&format!(
                    "wal-consumer[{log_label}] messages() failed: {error}; retrying"
                ));
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
        };
        tokio::pin!(messages);
        while let Some(next) = messages.next().await {
            let msg = match next {
                Ok(m) => m,
                Err(error) => {
                    log_warn(&format!("wal-consumer[{log_label}] stream error: {error}"));
                    break;
                }
            };
            metrics.received.fetch_add(1, Ordering::Relaxed);
            match serde_json::from_slice::<RowChange>(&msg.payload) {
                Ok(change) => {
                    if change.schema_version == SCHEMA_VERSION {
                        // Only low-cardinality routing fields are recorded — row
                        // data is CDC payload and must never reach logs/traces.
                        log_row_event(&log_label, &change);
                        metrics.handled.fetch_add(1, Ordering::Relaxed);
                        if run_handler_guarded(
                            &msg,
                            progress_every,
                            &metrics,
                            (handler)(change),
                        )
                        .await
                        .is_err()
                        {
                            // The row is still acked below: a panicking handler
                            // will panic again on redelivery, so retrying it
                            // would wedge the subscription on one poison row.
                            metrics.handler_panics.fetch_add(1, Ordering::Relaxed);
                            log_warn(&format!(
                                "wal-consumer[{log_label}] handler panicked; row acked and skipped"
                            ));
                        }
                    } else {
                        metrics.schema_mismatch.fetch_add(1, Ordering::Relaxed);
                        log_warn(&format!(
                            "wal-consumer[{log_label}] unsupported schemaVersion={}",
                            change.schema_version
                        ));
                    }
                }
                Err(error) => {
                    metrics.decode_errors.fetch_add(1, Ordering::Relaxed);
                    log_warn(&format!(
                        "wal-consumer[{log_label}] decode failed: {error}; payload len={}",
                        msg.payload.len()
                    ));
                }
            }
            if let Err(error) = msg.ack().await {
                metrics.ack_errors.fetch_add(1, Ordering::Relaxed);
                log_warn(&format!("wal-consumer[{log_label}] ack failed: {error}"));
            }
        }
        // The stream ended (server closed it, e.g. a deploy of the NATS
        // box). Reconnect by re-creating the messages stream.
        metrics.reconnects.fetch_add(1, Ordering::Relaxed);
        log_warn(&format!(
            "wal-consumer[{log_label}] message stream ended; reconnecting"
        ));
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

#[cfg(feature = "tracing")]
fn log_warn(msg: &str) {
    tracing::warn!("{msg}");
}

#[cfg(not(feature = "tracing"))]
fn log_warn(msg: &str) {
    eprintln!("{msg}");
}

/// Per-row structured event. The host service's `tracing` subscriber
/// (`dd-telemetry`) turns this into a `dd.log.v1` stdout line for Loki and,
/// when a span is active, correlates it with the OTLP trace.
///
/// Only low-cardinality routing fields are emitted — never `row`/`previousRow`,
/// which carry actual table data (same reasoning as the redacted `Display` for
/// `Error::Decode`).
#[cfg(feature = "tracing")]
fn log_row_event(label: &str, change: &RowChange) {
    tracing::debug!(
        durable = label,
        schema = %change.schema,
        table = %change.table,
        op = change.op.as_str(),
        "cdc row change received"
    );
}

#[cfg(not(feature = "tracing"))]
fn log_row_event(_label: &str, _change: &RowChange) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_canonical_envelope() {
        let json = r#"{
          "schemaVersion": "cdc.row.v1",
          "schema": "public",
          "table": "app_config",
          "op": "update",
          "lsn": "0/1A3B5C0",
          "xid": 12345,
          "tsMs": 1736000000000,
          "primaryKey": ["id"],
          "row": {"id":"00000000-0000-0000-0000-000000000001","scope":"default","key":"trading.platforms.v1"},
          "previousRow": {"id":"00000000-0000-0000-0000-000000000001"}
        }"#;
        let parsed: RowChange = serde_json::from_str(json).expect("decode");
        assert_eq!(parsed.op, ChangeOp::Update);
        assert_eq!(parsed.table, "app_config");
        assert_eq!(
            parsed.column("scope").and_then(Value::as_str),
            Some("default")
        );
        assert!(parsed.is_table("public", "app_config"));
    }

    #[test]
    fn metrics_render_prometheus_exposition() {
        let metrics = ConsumerMetrics::default();
        metrics.received.fetch_add(7, Ordering::Relaxed);
        metrics.handler_panics.fetch_add(2, Ordering::Relaxed);
        let text = metrics.prometheus_text("trading-server-app-config");

        assert!(text.contains("# TYPE dd_wal_consumer_received_total counter"));
        assert!(text
            .contains("dd_wal_consumer_received_total{durable=\"trading-server-app-config\"} 7"));
        assert!(text
            .contains("dd_wal_consumer_handler_panics_total{durable=\"trading-server-app-config\"} 2"));
        // Untouched counters still render (so a dashboard panel never gaps).
        assert!(text.contains("dd_wal_consumer_ack_progress_total"));
        assert!(text.contains("dd_wal_consumer_reconnects_total"));
    }

    #[test]
    fn metrics_label_is_escaped() {
        // A durable name with a quote must not break the exposition format.
        let metrics = ConsumerMetrics::default();
        let text = metrics.prometheus_text("we\"ird");
        assert!(text.contains(r#"durable="we\"ird""#));
    }

    #[test]
    fn subject_helpers() {
        assert_eq!(
            subject_for("public", "app_config", ChangeOp::Insert),
            "cdc.public.app_config.insert"
        );
        assert_eq!(
            subject_for_table("public", "lambda_functions"),
            "cdc.public.lambda_functions.>"
        );
    }
}
