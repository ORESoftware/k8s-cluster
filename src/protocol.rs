//! Wire protocol for `dd-rust-network-mutex`.
//!
//! TCP and Unix-domain-socket clients exchange newline-delimited JSON with the
//! broker. Every frame is one JSON object terminated by `\n`. The HTTP surface
//! reuses the same value enums but is request/response oriented (one body per
//! HTTP exchange), with optional long-poll for queued lock acquisitions.
//!
//! Each request carries a client-generated `uuid` correlation ID; the broker
//! echoes it on the matching response. Lock-acquired responses include a
//! distinct `lock_uuid` plus a monotonically increasing per-key `fencing_token`
//! that callers can attach to downstream writes (see
//! <https://martin.kleppmann.com/2016/02/08/how-to-do-distributed-locking.html>).
//!
//! The protocol is intentionally close to upstream Node.js `live-mutex`
//! semantics so existing operators can reason about it, but not byte-for-byte
//! compatible: we expose composite (multi-key) locks and fencing tokens as
//! first-class top-level fields rather than ad-hoc extensions.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: &str = "0.1.0";
pub const MAX_COMPOSITE_KEYS: usize = 5;

/// Hard ceiling on the per-key concurrency level (semaphore-style locks
/// — see upstream `live-mutex`'s `max` field). Every concurrent holder
/// occupies a row in the per-key holder map and its own deadline entry,
/// so the broker's per-key cost is proportional to `max`. 1_000 was
/// picked as the default because at that scale the holder/deadline
/// overhead is still trivial (~tens of KiB per fully-saturated key) but
/// the cap is high enough that ordinary "rate-limit at N parallel jobs"
/// workloads don't trip over it and don't need operator tuning.
///
/// The cap is enforced by the broker via clamping (a per-request `max`
/// above the cap is silently lowered and counted in
/// `dd_rust_network_mutex_concurrency_cap_clamps_total`). It can be
/// raised or lowered at startup with `LMX_MAX_CONCURRENCY_CAP`;
/// cross-runtime clients read the *current* effective cap from
/// `/metrics` rather than trusting a baked-in constant.
pub const DEFAULT_MAX_CONCURRENCY_CAP: u32 = 1_000;

/// Envelope for client → broker requests over TCP/UDS.
///
/// The on-the-wire `type` discriminator and every inline struct field are
/// camelCase. Cross-runtime clients (TypeScript / Go / Dart / Gleam) MUST
/// mirror this exactly. The Rust source uses snake_case identifiers; serde
/// does the rename automatically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Request {
    /// Initial protocol handshake. The broker rejects clients whose major
    /// version disagrees.
    Version { uuid: String, value: String },
    /// Optional auth handshake when `LMX_AUTH_TOKEN` is configured.
    Auth { uuid: String, token: String },
    /// Acquire an exclusive lock on a single `key`, OR a composite lock on up
    /// to `MAX_COMPOSITE_KEYS` keys via `keys` (mutually exclusive with `key`).
    Lock {
        uuid: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        keys: Option<Vec<String>>,
        #[serde(default)]
        pid: Option<i64>,
        #[serde(default)]
        ttl: Option<u64>,
        #[serde(default)]
        max: Option<u32>,
        #[serde(default)]
        force: bool,
        #[serde(default)]
        retry_count: u32,
        #[serde(default)]
        keep_locks_after_death: bool,
        /// Whether the broker should queue this request and block until the
        /// lock can be granted (`true`/absent, the default), or fail fast and
        /// return `acquired:false` immediately when the key(s) are contended
        /// (`false`). No-wait requests are never enqueued, so they cannot leak
        /// a later grant. Applies to both single-key and composite locks.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wait: Option<bool>,
    },
    /// Release a previously held lock. `lockUuid` must match the one returned
    /// by the broker on acquisition (or `force` must be true).
    Unlock {
        uuid: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        keys: Option<Vec<String>>,
        #[serde(default)]
        lock_uuid: Option<String>,
        #[serde(default)]
        force: bool,
    },
    /// Reader-writer: register a reader, blocking until any active writer is
    /// done. On grant the reader counter is incremented atomically.
    RegisterRead { uuid: String, key: String },
    /// Reader-writer: register a writer, blocking until readers and other
    /// writers are zero. On grant `writer_flag` is set true.
    RegisterWrite { uuid: String, key: String },
    /// Reader-writer: end a read (decrement reader count).
    EndRead { uuid: String, key: String },
    /// Reader-writer: end a write (clear writer flag and broadcast).
    EndWrite { uuid: String, key: String },
    /// Inspect the broker's current locks (debug / admin).
    LockInfo { uuid: String, key: String },
    /// List all known lock keys.
    Ls { uuid: String },
    /// Heartbeat for HTTP long-poll continuation; not used over TCP/UDS.
    Heartbeat { uuid: String },
}

/// Envelope for broker → client responses. camelCase wire format; see
/// [`Request`] for the cross-runtime contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Response {
    Version {
        uuid: String,
        broker_version: String,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    Auth {
        uuid: String,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Single-key lock result.
    Lock {
        uuid: String,
        key: String,
        acquired: bool,
        lock_request_count: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lock_uuid: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fencing_token: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        readers_count: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Composite (multi-key) lock result. `acquired:true` means every key was
    /// granted atomically; `lock_uuid` is the single token that releases all
    /// of them.
    CompositeLock {
        uuid: String,
        keys: Vec<String>,
        acquired: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lock_uuid: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fencing_tokens: Option<std::collections::BTreeMap<String, u64>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    Unlock {
        uuid: String,
        keys: Vec<String>,
        unlocked: bool,
        lock_request_count: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    RegisterReadResult {
        uuid: String,
        key: String,
        readers_count: u32,
        writer_flag: bool,
        granted: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lock_uuid: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fencing_token: Option<u64>,
    },
    RegisterWriteResult {
        uuid: String,
        key: String,
        readers_count: u32,
        writer_flag: bool,
        granted: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lock_uuid: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fencing_token: Option<u64>,
    },
    EndReadResult {
        uuid: String,
        key: String,
        readers_count: u32,
    },
    EndWriteResult {
        uuid: String,
        key: String,
        readers_count: u32,
        writer_flag: bool,
    },
    LockInfo {
        uuid: String,
        key: String,
        is_locked: bool,
        lockholder_uuids: Vec<String>,
        lock_request_count: usize,
        readers_count: u32,
        writer_flag: bool,
    },
    LsResult {
        uuid: String,
        keys: Vec<String>,
    },
    /// Server-initiated re-election notice to the next candidate in the queue.
    /// Ignorable on the client; it usually arrives just before a `Lock` grant.
    Reelection {
        uuid: String,
        key: String,
    },
    Error {
        uuid: String,
        error: String,
    },
    Ok {
        uuid: String,
    },
}

impl Response {
    /// Convenience for routers and tests.
    pub fn correlation_uuid(&self) -> &str {
        crate::routine_id!("ddl-routine-d7PH3GeHXLkM2Nb0I0");
        match self {
            Response::Version { uuid, .. }
            | Response::Auth { uuid, .. }
            | Response::Lock { uuid, .. }
            | Response::CompositeLock { uuid, .. }
            | Response::Unlock { uuid, .. }
            | Response::RegisterReadResult { uuid, .. }
            | Response::RegisterWriteResult { uuid, .. }
            | Response::EndReadResult { uuid, .. }
            | Response::EndWriteResult { uuid, .. }
            | Response::LockInfo { uuid, .. }
            | Response::LsResult { uuid, .. }
            | Response::Reelection { uuid, .. }
            | Response::Error { uuid, .. }
            | Response::Ok { uuid } => uuid,
        }
    }
}

/// HTTP body shapes (JSON). The broker also accepts the TCP `Request` enum
/// directly at `POST /v1/raw` for clients that want to share a transport.
pub mod http {
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct AcquireRequest {
        /// Optional client-provided idempotency/correlation key. BrokerRaft
        /// uses this to replay a recent response instead of appending a
        /// duplicate command when callers retry the same HTTP operation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub request_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub key: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub keys: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub ttl_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub max: Option<u32>,
        /// Long-poll wait window. `0` (or unset) returns immediately.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub wait_ms: Option<u64>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct AcquireResponse {
        pub acquired: bool,
        pub keys: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub lock_uuid: Option<String>,
        #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
        pub fencing_tokens: std::collections::BTreeMap<String, u64>,
        pub queue_depth: usize,
        /// Broker-reported reason for an unacquired lock (validation
        /// errors, oversized composites, etc.). Empty when `acquired`
        /// is true.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub error: Option<String>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ReleaseRequest {
        /// Optional client-provided idempotency/correlation key. BrokerRaft
        /// uses this to replay a recent response instead of appending a
        /// duplicate command when callers retry the same HTTP operation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub request_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub key: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub keys: Option<Vec<String>>,
        /// Optional so that operator-initiated `force: true` releases
        /// don't have to invent a fake UUID. The broker rejects a
        /// missing `lockUuid` unless `force` is true.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub lock_uuid: Option<String>,
        #[serde(default)]
        pub force: bool,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ReleaseResponse {
        pub unlocked: bool,
        pub keys: Vec<String>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct RwAcquireRequest {
        pub key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub wait_ms: Option<u64>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct RwAcquireResponse {
        pub granted: bool,
        pub key: String,
        pub readers_count: u32,
        pub writer_flag: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub lock_uuid: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub fencing_token: Option<u64>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct RwReleaseRequest {
        pub key: String,
        pub lock_uuid: String,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct RwReleaseResponse {
        pub key: String,
        pub readers_count: u32,
        pub writer_flag: bool,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct LockInfoResponse {
        pub key: String,
        pub is_locked: bool,
        pub lockholder_uuids: Vec<String>,
        pub lock_request_count: usize,
        pub readers_count: u32,
        pub writer_flag: bool,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_request_round_trips() {
        crate::routine_id!("ddl-routine-c8BV5Smf3UYt1BM_R8");
        let req = Request::Lock {
            uuid: "u-1".into(),
            key: Some("k1".into()),
            keys: None,
            pid: Some(123),
            ttl: Some(4000),
            max: Some(1),
            force: false,
            retry_count: 0,
            keep_locks_after_death: false,
            wait: Some(false),
        };
        let s = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&s).unwrap();
        assert!(matches!(back, Request::Lock { .. }));
    }

    #[test]
    fn composite_response_serialises() {
        crate::routine_id!("ddl-routine-eiBiIPhsWPZ3wqZixX");
        let mut tokens = std::collections::BTreeMap::new();
        tokens.insert("a".to_string(), 1u64);
        tokens.insert("b".to_string(), 1u64);
        let resp = Response::CompositeLock {
            uuid: "u-1".into(),
            keys: vec!["a".into(), "b".into()],
            acquired: true,
            lock_uuid: Some("L-1".into()),
            fencing_tokens: Some(tokens),
            error: None,
        };
        let s = serde_json::to_string(&resp).unwrap();
        assert!(s.contains("\"type\":\"compositeLock\""));
        assert!(s.contains("\"acquired\":true"));
        assert!(s.contains("\"lockUuid\":\"L-1\""));
        assert!(s.contains("\"fencingTokens\""));
    }

    #[test]
    fn lock_response_uses_camel_case_wire_fields() {
        crate::routine_id!("ddl-routine-q6cNU91QOmjsFO1XuR");
        let resp = Response::Lock {
            uuid: "u".into(),
            key: "k".into(),
            acquired: true,
            lock_request_count: 0,
            lock_uuid: Some("L".into()),
            fencing_token: Some(7),
            readers_count: Some(0),
            error: None,
        };
        let s = serde_json::to_string(&resp).unwrap();
        for needle in [
            "\"type\":\"lock\"",
            "\"lockRequestCount\":0",
            "\"lockUuid\":\"L\"",
            "\"fencingToken\":7",
            "\"readersCount\":0",
        ] {
            assert!(s.contains(needle), "{needle} missing from {s}");
        }
    }

    #[test]
    fn register_read_request_uses_camel_case_tag() {
        crate::routine_id!("ddl-routine-UJlTT7ESgL_BMCOGdl");
        let req = Request::RegisterRead {
            uuid: "u".into(),
            key: "k".into(),
        };
        let s = serde_json::to_string(&req).unwrap();
        assert!(s.contains("\"type\":\"registerRead\""), "{s}");
    }
}
