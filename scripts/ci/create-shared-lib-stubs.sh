#!/usr/bin/env bash
# Create minimal compile-time stand-ins for the three monorepo path dependencies
# used by Quaestor. A clean GitHub checkout does not contain ../../libs, so the
# old CI never compiled the service at all. These stubs mirror only the public
# interfaces Quaestor consumes; production builds in the k8s-cluster workspace
# continue to use the real shared crates.
#
# The script never overwrites a real shared crate. Interface drift in Quaestor
# therefore breaks CI, while developers with the monorepo checkout still compile
# against the canonical implementations.
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

[dependencies]
tower = "0.5"
TOML
  cat >"$telemetry_dir/src/lib.rs" <<'RS'
#[must_use]
pub struct OtelGuard;

pub fn init(_service_name: &str) -> OtelGuard {
    OtelGuard
}

pub fn http_trace_layer() -> tower::layer::util::Identity {
    tower::layer::util::Identity::new()
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
serde = { version = "1", features = ["derive"] }
tokio = { version = "1", features = ["rt", "macros"] }

[features]
default = ["tracing"]
tracing = []
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
TOML
  cat >"$subjects_dir/src/lib.rs" <<'RS'
pub const BILLING_ANCHORS_SUBJECT: &str = "dd.remote.billing.anchors";
pub const BILLING_CONNECTION_EVENTS_SUBJECT: &str = "dd.remote.billing.connections.events";
pub const BILLING_LEDGER_POSTINGS_SUBJECT: &str = "dd.remote.billing.ledger.postings";
pub const BILLING_RECONCILIATION_BREAKS_SUBJECT: &str =
    "dd.remote.billing.reconciliation.breaks";
pub const BILLING_SYNC_COMMANDS_QUEUE_GROUP: &str = "dd-billing-server";
pub const BILLING_SYNC_COMMANDS_SUBJECT: &str = "dd.remote.billing.commands.sync";
pub const BILLING_WEBHOOK_RECEIPTS_SUBJECT: &str = "dd.remote.billing.webhook.receipts";
RS
fi
