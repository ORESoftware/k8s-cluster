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
use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream::{
    consumer::{pull::Config as PullConfig, AckPolicy, Consumer, DeliverPolicy},
    Context,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::task::JoinHandle;

pub const SCHEMA_VERSION: &str = "cdc.row.v1";

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

/// Build the JetStream subject the gateway publishes to for a given table
/// and operation.
pub fn subject_for(schema: &str, table: &str, op: ChangeOp) -> String {
    format!("cdc.{schema}.{table}.{}", op.as_str())
}

/// Build the wildcard subject for "all ops on this table".
pub fn subject_for_table(schema: &str, table: &str) -> String {
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
            Error::Decode(e, raw) => write!(
                f,
                "row envelope decode error: {e}; first 200 bytes={}",
                String::from_utf8_lossy(&raw[..raw.len().min(200)])
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
    pub fn ack_wait(mut self, d: Duration) -> Self {
        self.ack_wait = d;
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
        let handler = Arc::new(handler);
        let label = self.durable_name.clone();
        let join = tokio::spawn(async move {
            run_pull_loop(consumer, label, handler).await;
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

async fn run_pull_loop<F, Fut>(consumer: Consumer<PullConfig>, label: String, handler: Arc<F>)
where
    F: Fn(RowChange) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
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
            match serde_json::from_slice::<RowChange>(&msg.payload) {
                Ok(change) => {
                    if change.schema_version == SCHEMA_VERSION {
                        (handler)(change).await;
                    } else {
                        log_warn(&format!(
                            "wal-consumer[{log_label}] unsupported schemaVersion={}",
                            change.schema_version
                        ));
                    }
                }
                Err(error) => {
                    log_warn(&format!(
                        "wal-consumer[{log_label}] decode failed: {error}; payload len={}",
                        msg.payload.len()
                    ));
                }
            }
            if let Err(error) = msg.ack().await {
                log_warn(&format!("wal-consumer[{log_label}] ack failed: {error}"));
            }
        }
        // The stream ended (server closed it, e.g. a deploy of the NATS
        // box). Reconnect by re-creating the messages stream.
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
