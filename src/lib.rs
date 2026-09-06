//! `dd-rust-network-mutex` — networked mutex broker and clients.
//!
//! This crate is a Rust port of the Node.js `live-mutex` library. It exposes
//! both a server (broker) and clients over TCP, Unix domain sockets, and
//! HTTP, plus first-class support for:
//!
//! * **Reader-writer locks** alongside the regular exclusive `Client`.
//! * **Fencing tokens** — a per-key monotonically increasing counter is
//!   handed back with every successful grant. Callers should pass the token
//!   to whatever resource they're protecting, so a stale leaseholder's
//!   eventual write can be detected and rejected.
//! * **Composite (multi-key) locking** — atomic acquisition of up to five
//!   keys in one request, deadlock-free via global key sorting. See
//!   <https://github.com/ORESoftware/live-mutex/issues/105>.
//! * **TLS** — optional, behind the `tls` cargo feature. In production, a
//!   load balancer or service mesh is usually a more capable terminator.
//!
//! ## Public API surface
//!
//! - [`Broker`] / [`BrokerConfig`] — in-process broker. Used by tests and the
//!   `main.rs` binary.
//! - [`server::run`] / [`server::ServerConfig`] — bind TCP/UDS/HTTP listeners
//!   on top of a `Broker`.
//! - [`Client`] / [`ClientConfig`] — exclusive lock client (single or
//!   composite key).
//! - [`RwClient`] — reader-writer lock client.
//! - [`protocol::Request`] / [`protocol::Response`] — serializable wire
//!   format. Useful for code-gen / cross-runtime clients.

pub mod broker;
pub mod broker_raft;
pub mod cli_flags;
pub mod client;
pub mod config;
pub mod metrics;
pub mod protocol;
pub mod queue;
pub mod routine;
pub mod server;
pub mod sim;
pub mod sockopt;
pub mod status;

pub use broker::{Broker, BrokerConfig, BrokerMetrics};
pub use broker_raft::{
    BrokerRaft, BrokerRaftConfig, BrokerRaftError, RaftCommand, RaftCompactionReport, RaftLogEntry,
    RaftLogStore, RaftMembership, RaftPeerConfig, RaftSnapshotMetadata,
};
pub use cli_flags::{load_broker_cli_config, BrokerCliConfig, BrokerCliEnv, CliFlagError};
pub use client::{Client, ClientConfig, ClientError, LockGuard, LockInfo, RwClient};
pub use config::{load_runtime_config, ConfigError, RuntimeConfig};
pub use protocol::{Request, Response, MAX_COMPOSITE_KEYS, PROTOCOL_VERSION};
pub use routine::{
    current_log_level, init_tracing, is_otel_enabled, set_log_level, set_otel_enabled,
    shutdown_tracing,
};
pub use server::{run as run_server, ServerConfig};
pub use sim::{RaftSim, RaftSimConfig, RaftSimError, RaftSimLock};

#[cfg(feature = "tls")]
pub use server::TlsConfig;
