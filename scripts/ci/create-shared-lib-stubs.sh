#!/usr/bin/env bash
# Create minimal compile-time source stand-ins for the three monorepo path
# dependencies used by Quaestor. A clean GitHub checkout does not contain
# ../../libs, so the old CI never compiled the service at all.
#
# IMPORTANT: each generated Cargo.toml mirrors the canonical manifest in
# ORESoftware/k8s-libs-and-shared-defs. Cargo.lock records path-package
# dependency edges; a simplified manifest would make `cargo --locked` request a
# lockfile rewrite and hide real dependency drift. Only the Rust implementation
# is minimized here.
#
# The script never overwrites a real shared crate. Developers and production
# workspace builds therefore continue to compile against the canonical code.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
workspace_root="$(cd "$repo_root/../.." && pwd)"
libs_root="$workspace_root/libs"

create_if_missing() {
  local crate_dir="$1"
  if [[ -e "$crate_dir/Cargo.toml" ]]; then
    printf 'using existing shared crate: %s\n' "$crate_dir"
    return 1
  fi
  mkdir -p "$crate_dir/src"
  return 0
}

telemetry_dir="$libs_root/telemetry-rs"
if create_if_missing "$telemetry_dir"; then
  cat >"$telemetry_dir/Cargo.toml" <<'TOML'
[package]
name = "dd-telemetry"
version = "0.1.0"
edition = "2021"
description = "Shared OpenTelemetry tracing + structured-logging init for the Rust services in k8s-cluster/remote. Call dd_telemetry::init(\"dd-foo\") at the top of main and add dd_telemetry::http_trace_layer() to your axum Router. Explicit instrumentation only — no monkey-patching."

[lib]
path = "src/lib.rs"

[dependencies]
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json", "fmt"] }
tracing-opentelemetry = "0.27"
opentelemetry = "0.26"
opentelemetry_sdk = { version = "0.26", features = ["rt-tokio"] }
opentelemetry-otlp = { version = "0.26", default-features = false, features = [
    "trace",
    "http-proto",
    "reqwest-client",
    "reqwest-rustls",
] }
opentelemetry-http = "0.26"
opentelemetry-semantic-conventions = "0.16"
tower-http = { version = "0.6", features = ["trace"] }
http = "1"
tokio = { version = "1", features = ["rt"] }
TOML
  cat >"$telemetry_dir/src/lib.rs" <<'RS'
#[must_use]
pub struct OtelGuard;

pub fn init(_service_name: &str) -> OtelGuard {
    OtelGuard
}

pub fn http_trace_layer() -> tower_http::trace::TraceLayer<
    tower_http::classify::SharedClassifier<tower_http::classify::ServerErrorsAsFailures>,
> {
    tower_http::trace::TraceLayer::new_for_http()
}
RS
fi

wal_dir="$libs_root/wal-consumer-rs"
if create_if_missing "$wal_dir"; then
  cat >"$wal_dir/Cargo.toml" <<'TOML'
[package]
name = "dd-wal-consumer"
version = "0.1.0"
edition = "2021"

[dependencies]
async-nats = "=0.38.0"
futures-util = "0.3"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "time", "sync"] }
tracing = { version = "0.1", optional = true }

[features]
default = ["tracing"]
tracing = ["dep:tracing"]
TOML
  cat >"$wal_dir/src/lib.rs" <<'RS'
use std::future::Future;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeOp {
    Insert,
    Update,
    Delete,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RowChange {
    pub schema: String,
    pub table: String,
    pub op: ChangeOp,
    pub lsn: String,
    pub xid: Option<i64>,
    pub ts_ms: u64,
}

#[derive(Debug)]
pub struct Error;

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("stub WAL consumer error")
    }
}

impl std::error::Error for Error {}

#[derive(Default)]
pub struct SubscriptionBuilder;

impl SubscriptionBuilder {
    pub fn stream(self, _stream: impl Into<String>) -> Self {
        self
    }

    pub fn durable_name(self, _name: impl Into<String>) -> Self {
        self
    }

    pub fn filter_subject(self, _subject: impl Into<String>) -> Self {
        self
    }

    pub async fn start<F, Fut>(
        self,
        _jetstream: &async_nats::jetstream::Context,
        _handler: F,
    ) -> Result<tokio::task::JoinHandle<()>, Error>
    where
        F: Fn(RowChange) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        Ok(tokio::spawn(async {
            std::future::pending::<()>().await;
        }))
    }
}

pub struct Subscription;

impl Subscription {
    pub fn builder() -> SubscriptionBuilder {
        SubscriptionBuilder
    }
}
RS
fi

subjects_dir="$libs_root/nats/subject-defs/generated/rust"
if create_if_missing "$subjects_dir"; then
  cat >"$subjects_dir/Cargo.toml" <<'TOML'
[package]
name = "dd-nats-subject-defs"
version = "0.1.0"
edition = "2021"
description = "Generated Rust constants, formatters and parsers for dd NATS subject conventions. Do not edit by hand."

[lib]
path = "src/lib.rs"

[dependencies]
TOML
  cat >"$subjects_dir/src/lib.rs" <<'RS'
pub const BILLING_ANCHORS_SUBJECT: &str = "dd.remote.billing.anchors";
pub const BILLING_CONNECTION_EVENTS_SUBJECT: &str = "dd.remote.billing.connections.events";
pub const BILLING_LEDGER_POSTINGS_SUBJECT: &str = "dd.remote.billing.ledger.postings";
pub const BILLING_RECONCILIATION_BREAKS_SUBJECT: &str = "dd.remote.billing.reconciliation.breaks";
pub const BILLING_SYNC_COMMANDS_QUEUE_GROUP: &str = "dd-billing-server";
pub const BILLING_SYNC_COMMANDS_SUBJECT: &str = "dd.remote.billing.commands.sync";
pub const BILLING_WEBHOOK_RECEIPTS_SUBJECT: &str = "dd.remote.billing.webhook.receipts";
RS
fi
