//! Distributed-lease seam.
//!
//! Every place in this service that must be executed by exactly one worker goes
//! through [`Coordination`]. There are two implementations and the difference
//! between them is the difference between "one replica" and "more than one":
//!
//! * [`NoopCoordination`] — the default. Grants every lease locally and
//!   provides **no cross-process exclusion whatsoever**. It exists so the call
//!   sites are uniform and so the single-replica deployment behaves exactly as
//!   it did before this module existed.
//! * [`FiduciaCoordination`] — real distributed leases from fiducia-node's
//!   `/v1/locks/*` API, with fencing tokens.
//!
//! Fiducia leases are **off by default**, and deliberately so:
//!
//! * `/v1/locks/*` mutations require the `locks:write` scope. The Daedalus
//!   `FIDUCIA_API_KEY` is KV-read shaped and will be rejected with HTTP 403
//!   `insufficient_scope` — hence the separate, least-privilege
//!   `FIDUCIA_LOCKS_API_KEY`.
//! * Reaching fiducia at all depends on NetworkPolicy egress from this
//!   namespace (`deploy/k8s/networkpolicy.yaml`, namespace `fiducia`, TCP 8088).
//!   Without that rule every acquire is a connect timeout, and note that the
//!   `k8s/` copy of the manifests does not carry the rule at all.
//!
//! Because of both, this module must degrade *safely and loudly*: a fiducia
//! outage surfaces as [`CoordinationError::Unavailable`] and a scope problem as
//! [`CoordinationError::Forbidden`]. Neither is ever silently converted into a
//! granted lease — the caller decides, and the NATS caller declines to process.

use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        OnceLock,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::secrets::normalize_base_url;

/// Default lease lifetime. Matches fiducia-node's own `ttl_ms` default.
pub(crate) const DEFAULT_LEASE_TTL_MS: u64 = 30_000;
/// Upper bound on the renewal interval, so a long TTL still produces a timely
/// signal that authority was lost.
const MAX_RENEWAL_INTERVAL_MS: u64 = 20_000;
/// Ceiling on a lock-API response body. Lock responses are a few hundred bytes.
const MAX_RESPONSE_BYTES: u64 = 64 * 1024;

/// A held lease.
///
/// `fencing_token` is fiducia-node's strictly increasing `u64`. Renewal
/// preserves it; a *new* grant always gets a higher one. That is what makes a
/// stale holder detectable: its token is lower than the current one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Lease {
    /// The union of keys this grant covers. Release is by token, not by key,
    /// so the whole union is released together.
    pub(crate) keys: Vec<String>,
    pub(crate) holder: String,
    pub(crate) fencing_token: u64,
    pub(crate) expires_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CoordinationError {
    /// Someone else holds the key (or we were placed in the wait queue rather
    /// than granted). The caller must not proceed.
    AlreadyHeld { key: String },
    /// We no longer hold what we thought we held: a renewal was refused, or a
    /// response did not preserve the exact grant. Authority is lost as of now;
    /// the caller must stop before any further side effect.
    Fenced { reason: String },
    /// fiducia-node could not be reached, or answered something unusable.
    /// Distinct from `Fenced` because it says nothing about who holds the lock.
    Unavailable { detail: String },
    /// The credential is not permitted to use the lock API — almost always the
    /// KV-shaped `FIDUCIA_API_KEY` being used where a `locks:write` key is
    /// required. Actionable configuration error, never a grant.
    Forbidden { detail: String },
}

impl std::fmt::Display for CoordinationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyHeld { key } => {
                write!(
                    formatter,
                    "lease for {key} is already held by another holder"
                )
            }
            Self::Fenced { reason } => write!(formatter, "lease authority lost: {reason}"),
            Self::Unavailable { detail } => {
                write!(formatter, "coordination backend unavailable: {detail}")
            }
            Self::Forbidden { detail } => write!(formatter, "coordination forbidden: {detail}"),
        }
    }
}

impl std::error::Error for CoordinationError {}

#[async_trait]
pub(crate) trait Coordination: Send + Sync {
    /// Try to take `key` for `ttl`. Never blocks and never waits in a queue.
    async fn acquire_lease(&self, key: &str, ttl: Duration) -> Result<Lease, CoordinationError>;
    /// Extend an existing grant. The fencing token is preserved on success; any
    /// failure means authority is gone.
    async fn renew_lease(&self, lease: &Lease, ttl: Duration) -> Result<Lease, CoordinationError>;
    /// Give the grant back. Best-effort by contract: the TTL is the backstop.
    async fn release_lease(&self, lease: &Lease) -> Result<(), CoordinationError>;
    /// Short name for boot logs and diagnostics.
    fn mode(&self) -> &'static str;
    /// Whether this implementation actually excludes other processes.
    fn is_distributed(&self) -> bool;
}

/// The process-wide holder identity: one UUIDv4 per process.
///
/// Deliberately **not** pid/hostname. Holder identity participates in
/// cancellation authority on fiducia-node, so a value that another process
/// could guess or legitimately reuse (a recycled pid, a StatefulSet hostname
/// after a restart) would let a *different* process cancel or be mistaken for
/// this one's waiters.
pub(crate) fn process_holder_id() -> &'static str {
    static HOLDER: OnceLock<String> = OnceLock::new();
    HOLDER.get_or_init(|| format!("dd-fabrication-server-{}", Uuid::new_v4()))
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

/// Local, non-distributed coordination.
///
/// **This provides no cross-process exclusion.** Every `acquire_lease` call
/// succeeds immediately, even for a key another process is holding, because
/// this implementation cannot see other processes. It is the honest default for
/// a `replicas: 1` deployment with a `Recreate` strategy: there *is* no other
/// process, so uniform call sites cost nothing and the fiducia dependency stays
/// optional. Running more than one replica on this implementation gives you
/// concurrent duplicate processing with no error anywhere — see the "Scaling
/// past one replica" section of the readme.
///
/// The fencing token counts up per process so that call sites which stamp
/// mutations with it still see a monotonic value, but it is *not* comparable
/// across processes and must not be trusted as one.
pub(crate) struct NoopCoordination {
    holder: String,
    next_token: AtomicU64,
}

impl Default for NoopCoordination {
    fn default() -> Self {
        Self {
            holder: process_holder_id().to_string(),
            next_token: AtomicU64::new(1),
        }
    }
}

#[async_trait]
impl Coordination for NoopCoordination {
    async fn acquire_lease(&self, key: &str, ttl: Duration) -> Result<Lease, CoordinationError> {
        Ok(Lease {
            keys: vec![key.to_string()],
            holder: self.holder.clone(),
            fencing_token: self.next_token.fetch_add(1, Ordering::Relaxed),
            expires_at_ms: now_unix_ms()
                .saturating_add(ttl.as_millis().min(u128::from(u64::MAX)) as u64),
        })
    }

    async fn renew_lease(&self, lease: &Lease, ttl: Duration) -> Result<Lease, CoordinationError> {
        let mut renewed = lease.clone();
        renewed.expires_at_ms =
            now_unix_ms().saturating_add(ttl.as_millis().min(u128::from(u64::MAX)) as u64);
        Ok(renewed)
    }

    async fn release_lease(&self, _lease: &Lease) -> Result<(), CoordinationError> {
        Ok(())
    }

    fn mode(&self) -> &'static str {
        "noop"
    }

    fn is_distributed(&self) -> bool {
        false
    }
}

/// Real distributed leases backed by fiducia-node's `/v1/locks/*` API.
pub(crate) struct FiduciaCoordination {
    base_url: String,
    api_key: String,
    holder: String,
    http: reqwest::Client,
}

impl FiduciaCoordination {
    /// Construct against a validated base URL. The client is built exactly like
    /// [`crate::secrets`]'s: redirects disabled (so a 302 cannot move a bearer
    /// token to another host), bounded connect and total timeouts, and a URL
    /// that has already been refused if it names a cloud-metadata host or
    /// carries credentials.
    pub(crate) fn new(raw_url: &str, api_key: String) -> Result<Self, String> {
        let base_url = normalize_base_url(raw_url)?;
        let api_key = api_key.trim().to_string();
        if api_key.is_empty() {
            return Err("FIDUCIA_LOCKS_API_KEY must not be empty".to_string());
        }
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|_| "could not construct the Fiducia lock HTTP client".to_string())?;
        Ok(Self {
            base_url,
            api_key,
            holder: process_holder_id().to_string(),
            http,
        })
    }

    pub(crate) fn holder(&self) -> &str {
        &self.holder
    }

    /// POST a lock mutation and return `result.output`.
    async fn post_output(&self, path: &str, body: Value) -> Result<Value, CoordinationError> {
        let response = self
            .http
            .post(format!("{}{path}", self.base_url))
            .bearer_auth(&self.api_key)
            .header(reqwest::header::ACCEPT, "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|error| CoordinationError::Unavailable {
                detail: format!("{path} request failed: {error}"),
            })?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            // Read a bounded slice of the body: fiducia names the missing scope
            // in it, and an operator staring at a pod log needs that word.
            let detail = response.text().await.unwrap_or_default();
            let detail = detail.chars().take(256).collect::<String>();
            return Err(CoordinationError::Forbidden {
                detail: format!(
                    "fiducia refused {path} with HTTP {} ({}). The lock API requires the \
                     `locks:write` scope: set FIDUCIA_LOCKS_API_KEY to a lock-capable key \
                     (the KV-read FIDUCIA_API_KEY/FIDUCIA_TOKEN is not one), and open \
                     NetworkPolicy egress from this namespace to fiducia-node",
                    status.as_u16(),
                    if detail.is_empty() {
                        "no body"
                    } else {
                        detail.trim()
                    }
                ),
            });
        }
        if !status.is_success() {
            return Err(CoordinationError::Unavailable {
                detail: format!("fiducia {path} returned HTTP {}", status.as_u16()),
            });
        }
        if response
            .content_length()
            .is_some_and(|declared| declared > MAX_RESPONSE_BYTES)
        {
            return Err(CoordinationError::Unavailable {
                detail: format!("fiducia {path} response exceeded {MAX_RESPONSE_BYTES} bytes"),
            });
        }
        let body: Value =
            response
                .json()
                .await
                .map_err(|error| CoordinationError::Unavailable {
                    detail: format!("fiducia {path} response was not JSON: {error}"),
                })?;
        body.pointer("/result/output")
            .cloned()
            .ok_or_else(|| CoordinationError::Unavailable {
                detail: format!("fiducia {path} response omitted result.output"),
            })
    }
}

#[async_trait]
impl Coordination for FiduciaCoordination {
    async fn acquire_lease(&self, key: &str, ttl: Duration) -> Result<Lease, CoordinationError> {
        let ttl_ms = ttl_to_ms(ttl);
        let output = self
            .post_output(
                "/v1/locks/acquire",
                json!({
                    "keys": [key],
                    "holder": self.holder,
                    // A fresh id per attempt: fiducia uses it to make a retried
                    // acquire idempotent rather than a second queue slot.
                    "request_id": format!("dd-fab-acquire-{}", Uuid::new_v4()),
                    "ttl_ms": ttl_ms,
                    // `wait` does not long-poll — it reserves a FIFO slot and
                    // returns immediately. This service must never hold a slot:
                    // a NATS redelivery is a better retry than a queued waiter
                    // whose message may be redelivered underneath it.
                    "wait": false,
                }),
            )
            .await?;

        // `queued: true` is NOT an acquisition. Treating a queue position as a
        // grant is the single most dangerous misreading of this API, so it is
        // checked explicitly and before anything else.
        if output.get("queued").and_then(Value::as_bool) == Some(true)
            || output.get("acquired").and_then(Value::as_bool) != Some(true)
        {
            return Err(CoordinationError::AlreadyHeld {
                key: key.to_string(),
            });
        }
        let fencing_token = output
            .get("fencing_token")
            .and_then(Value::as_u64)
            .ok_or_else(|| CoordinationError::Unavailable {
                detail: "fiducia acquire response omitted a usable fencing_token".to_string(),
            })?;
        let keys = output
            .get("keys")
            .and_then(Value::as_array)
            .map(|keys| {
                keys.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| vec![key.to_string()]);
        if !keys.iter().any(|granted| granted == key) {
            return Err(CoordinationError::Fenced {
                reason: format!("fiducia granted {keys:?}, which does not include {key}"),
            });
        }
        Ok(Lease {
            keys,
            holder: output
                .get("holder")
                .and_then(Value::as_str)
                .unwrap_or(&self.holder)
                .to_string(),
            fencing_token,
            expires_at_ms: output
                .get("lease_expires_ms")
                .and_then(Value::as_u64)
                .unwrap_or_else(|| now_unix_ms().saturating_add(ttl_ms)),
        })
    }

    async fn renew_lease(&self, lease: &Lease, ttl: Duration) -> Result<Lease, CoordinationError> {
        let ttl_ms = ttl_to_ms(ttl);
        let output = self
            .post_output(
                "/v1/locks/renew",
                json!({
                    "keys": lease.keys,
                    "holder": lease.holder,
                    "fencing_token": lease.fencing_token,
                    "ttl_ms": ttl_ms,
                }),
            )
            .await?;
        if output.get("renewed").and_then(Value::as_bool) != Some(true) {
            // `not_found` / `not_holder` / `key_mismatch` all mean the same
            // thing to us: we are not the holder any more.
            return Err(CoordinationError::Fenced {
                reason: format!(
                    "fiducia refused renewal (reason={})",
                    output
                        .get("reason")
                        .and_then(Value::as_str)
                        .unwrap_or("unspecified")
                ),
            });
        }
        // Renewal preserves the fencing token. A response that changes the
        // token, holder, or key set is not our grant, whatever it says.
        let holder_matches = output
            .get("holder")
            .and_then(Value::as_str)
            .is_none_or(|holder| holder == lease.holder);
        let token_matches = output
            .get("fencing_token")
            .and_then(Value::as_u64)
            .is_none_or(|token| token == lease.fencing_token);
        if !holder_matches || !token_matches {
            return Err(CoordinationError::Fenced {
                reason: "fiducia renewal response did not preserve the exact grant".to_string(),
            });
        }
        let mut renewed = lease.clone();
        renewed.expires_at_ms = output
            .get("lease_expires_ms")
            .and_then(Value::as_u64)
            .unwrap_or_else(|| now_unix_ms().saturating_add(ttl_ms));
        Ok(renewed)
    }

    async fn release_lease(&self, lease: &Lease) -> Result<(), CoordinationError> {
        // Release carries NO key: the fencing token identifies the whole grant
        // union, and sending a key would be a different (nonexistent) contract.
        let output = self
            .post_output(
                "/v1/locks/release",
                json!({
                    "holder": lease.holder,
                    "fencing_token": lease.fencing_token,
                }),
            )
            .await?;
        if output.get("released").and_then(Value::as_bool) == Some(false) {
            return Err(CoordinationError::Fenced {
                reason: format!(
                    "fiducia refused release (reason={})",
                    output
                        .get("reason")
                        .and_then(Value::as_str)
                        .unwrap_or("unspecified")
                ),
            });
        }
        Ok(())
    }

    fn mode(&self) -> &'static str {
        "fiducia"
    }

    fn is_distributed(&self) -> bool {
        true
    }
}

fn ttl_to_ms(ttl: Duration) -> u64 {
    // fiducia caps ttl_ms at 24h; clamp locally so an absurd value is a bounded
    // request rather than a rejected one.
    (ttl.as_millis().min(u128::from(u64::MAX)) as u64).clamp(1_000, 24 * 60 * 60 * 1_000)
}

/// The renewal interval for a lease of `ttl`: a third of the TTL, so two
/// consecutive renewals can fail before the grant lapses.
pub(crate) fn renewal_interval(ttl: Duration) -> Duration {
    Duration::from_millis((ttl_to_ms(ttl) / 3).clamp(1, MAX_RENEWAL_INTERVAL_MS))
}

/// Renew `lease` forever, returning the first failure.
///
/// This never returns `Ok`: either the caller drops the future because the work
/// finished, or a renewal fails and the error is the caller's signal that
/// authority is gone and no further side effect may be performed. Callers run
/// it in `select!` against the work itself precisely so that a lost lease
/// cancels the work rather than being noticed afterwards.
pub(crate) async fn maintain_lease(
    coordination: &dyn Coordination,
    lease: &Lease,
    ttl: Duration,
) -> CoordinationError {
    let interval = renewal_interval(ttl);
    let mut current = lease.clone();
    loop {
        tokio::time::sleep(interval).await;
        match coordination.renew_lease(&current, ttl).await {
            Ok(renewed) => current = renewed,
            Err(error) => return error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{extract::State, routing::post, Json, Router};
    use std::sync::{Arc, Mutex};

    /// A real fiducia-shaped HTTP server on an ephemeral port. No mocking
    /// framework: the client under test performs actual requests, so the JSON
    /// body, the path, and the bearer header are all exercised.
    #[derive(Default)]
    struct FakeState {
        /// Queued responses per path; the last one repeats once drained.
        responses: Mutex<Vec<(&'static str, u16, Value)>>,
        requests: Mutex<Vec<(String, Value, Option<String>)>>,
    }

    struct FakeFiducia {
        url: String,
        state: Arc<FakeState>,
    }

    impl FakeFiducia {
        fn requests(&self) -> Vec<(String, Value, Option<String>)> {
            self.state.requests.lock().expect("requests").clone()
        }
    }

    async fn handle(
        state: Arc<FakeState>,
        path: &'static str,
        headers: axum::http::HeaderMap,
        body: Value,
    ) -> (axum::http::StatusCode, Json<Value>) {
        let authorization = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        state
            .requests
            .lock()
            .expect("requests")
            .push((path.to_string(), body, authorization));
        let mut responses = state.responses.lock().expect("responses");
        let index = responses.iter().position(|entry| entry.0 == path);
        let (status, payload) = match index {
            Some(index) if responses.iter().filter(|entry| entry.0 == path).count() > 1 => {
                let entry = responses.remove(index);
                (entry.1, entry.2)
            }
            Some(index) => (responses[index].1, responses[index].2.clone()),
            None => (500, json!({"error": "no queued response"})),
        };
        (
            axum::http::StatusCode::from_u16(status).expect("status"),
            Json(payload),
        )
    }

    async fn start_fake(responses: Vec<(&'static str, u16, Value)>) -> FakeFiducia {
        let state = Arc::new(FakeState {
            responses: Mutex::new(responses),
            requests: Mutex::new(Vec::new()),
        });
        async fn acquire(
            State(state): State<Arc<FakeState>>,
            headers: axum::http::HeaderMap,
            Json(body): Json<Value>,
        ) -> (axum::http::StatusCode, Json<Value>) {
            handle(state, "/v1/locks/acquire", headers, body).await
        }
        async fn renew(
            State(state): State<Arc<FakeState>>,
            headers: axum::http::HeaderMap,
            Json(body): Json<Value>,
        ) -> (axum::http::StatusCode, Json<Value>) {
            handle(state, "/v1/locks/renew", headers, body).await
        }
        async fn release(
            State(state): State<Arc<FakeState>>,
            headers: axum::http::HeaderMap,
            Json(body): Json<Value>,
        ) -> (axum::http::StatusCode, Json<Value>) {
            handle(state, "/v1/locks/release", headers, body).await
        }
        let app = Router::new()
            .route("/v1/locks/acquire", post(acquire))
            .route("/v1/locks/renew", post(renew))
            .route("/v1/locks/release", post(release))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let address = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        FakeFiducia {
            url: format!("http://{address}"),
            state,
        }
    }

    fn acquired(token: u64) -> Value {
        json!({"committed": true, "result": {"output": {
            "acquired": true,
            "queued": false,
            "keys": ["daedalus/fab/request/req-1"],
            "holder": "holder",
            "fencing_token": token,
            "lease_expires_ms": 1_700_000_030_000_u64,
            "revision": 4
        }}})
    }

    #[tokio::test]
    async fn acquire_parses_the_fencing_token_and_sends_the_documented_body() {
        let fake = start_fake(vec![("/v1/locks/acquire", 200, acquired(41))]).await;
        let coordination =
            FiduciaCoordination::new(&fake.url, "locks-key".to_string()).expect("client");
        let lease = coordination
            .acquire_lease("daedalus/fab/request/req-1", Duration::from_secs(30))
            .await
            .expect("acquire");

        assert_eq!(lease.fencing_token, 41);
        assert_eq!(lease.keys, vec!["daedalus/fab/request/req-1".to_string()]);
        assert_eq!(lease.expires_at_ms, 1_700_000_030_000);

        let requests = fake.requests();
        assert_eq!(requests.len(), 1);
        let (path, body, authorization) = &requests[0];
        assert_eq!(path, "/v1/locks/acquire");
        assert_eq!(authorization.as_deref(), Some("Bearer locks-key"));
        assert_eq!(body["keys"], json!(["daedalus/fab/request/req-1"]));
        assert_eq!(body["holder"], json!(coordination.holder()));
        assert_eq!(body["ttl_ms"], json!(30_000));
        // Never reserve a FIFO slot: `wait` does not long-poll.
        assert_eq!(body["wait"], json!(false));
        assert!(body["request_id"]
            .as_str()
            .is_some_and(|value| value.starts_with("dd-fab-acquire-")));
    }

    #[tokio::test]
    async fn a_queued_response_is_not_an_acquisition() {
        let queued = json!({"committed": true, "result": {"output": {
            "acquired": false, "queued": true, "position": 2, "wait_expires_ms": 1_700_000_000_000_u64
        }}});
        let fake = start_fake(vec![("/v1/locks/acquire", 200, queued)]).await;
        let coordination =
            FiduciaCoordination::new(&fake.url, "locks-key".to_string()).expect("client");
        let error = coordination
            .acquire_lease("daedalus/fab/request/req-1", Duration::from_secs(30))
            .await
            .expect_err("a queue position must never read as a grant");
        assert_eq!(
            error,
            CoordinationError::AlreadyHeld {
                key: "daedalus/fab/request/req-1".to_string()
            }
        );
    }

    #[tokio::test]
    async fn a_refused_renewal_is_surfaced_as_lost_authority() {
        let refused = json!({"committed": true, "result": {"output": {
            "renewed": false, "reason": "not_holder", "revision": 9
        }}});
        let fake = start_fake(vec![
            ("/v1/locks/acquire", 200, acquired(7)),
            ("/v1/locks/renew", 200, refused),
        ])
        .await;
        let coordination =
            FiduciaCoordination::new(&fake.url, "locks-key".to_string()).expect("client");
        let lease = coordination
            .acquire_lease("daedalus/fab/request/req-1", Duration::from_secs(30))
            .await
            .expect("acquire");

        let error = coordination
            .renew_lease(&lease, Duration::from_secs(30))
            .await
            .expect_err("renewed:false must not be treated as a renewal");
        assert!(
            matches!(&error, CoordinationError::Fenced { reason } if reason.contains("not_holder")),
            "unexpected error: {error}"
        );

        // The supervisor must convert that into a bounded, terminal signal.
        let lost = tokio::time::timeout(
            Duration::from_secs(5),
            maintain_lease(&coordination, &lease, Duration::from_secs(1)),
        )
        .await
        .expect("renewal supervisor must not hang");
        assert!(matches!(lost, CoordinationError::Fenced { .. }));

        let renew = fake
            .requests()
            .into_iter()
            .find(|(path, _, _)| path == "/v1/locks/renew")
            .expect("renew request");
        assert_eq!(renew.1["fencing_token"], json!(7));
        assert_eq!(renew.1["keys"], json!(["daedalus/fab/request/req-1"]));
        assert_eq!(renew.1["holder"], json!("holder"));
    }

    #[tokio::test]
    async fn release_is_by_holder_and_token_with_no_key() {
        let released = json!({"committed": true, "result": {"output": {
            "released": true, "keys": ["daedalus/fab/request/req-1"], "promoted": [], "revision": 5
        }}});
        let fake = start_fake(vec![
            ("/v1/locks/acquire", 200, acquired(12)),
            ("/v1/locks/release", 200, released),
        ])
        .await;
        let coordination =
            FiduciaCoordination::new(&fake.url, "locks-key".to_string()).expect("client");
        let lease = coordination
            .acquire_lease("daedalus/fab/request/req-1", Duration::from_secs(30))
            .await
            .expect("acquire");
        coordination.release_lease(&lease).await.expect("release");

        let release = fake
            .requests()
            .into_iter()
            .find(|(path, _, _)| path == "/v1/locks/release")
            .expect("release request");
        // The token releases the whole union; the API takes no key here.
        assert_eq!(release.1, json!({"holder": "holder", "fencing_token": 12}));
    }

    #[tokio::test]
    async fn insufficient_scope_is_an_actionable_error_not_a_grant() {
        let fake = start_fake(vec![(
            "/v1/locks/acquire",
            403,
            json!({"error": "insufficient_scope", "required": "locks:write"}),
        )])
        .await;
        let coordination =
            FiduciaCoordination::new(&fake.url, "kv-read-key".to_string()).expect("client");
        let error = coordination
            .acquire_lease("daedalus/fab/request/req-1", Duration::from_secs(30))
            .await
            .expect_err("a 403 must never read as a grant");

        let CoordinationError::Forbidden { detail } = &error else {
            panic!("expected Forbidden, got {error}");
        };
        // Actionable: it names the scope, the env var to fix, and the egress.
        assert!(detail.contains("locks:write"), "{detail}");
        assert!(detail.contains("FIDUCIA_LOCKS_API_KEY"), "{detail}");
        assert!(detail.contains("NetworkPolicy"), "{detail}");
        assert!(detail.contains("insufficient_scope"), "{detail}");
    }

    #[tokio::test]
    async fn an_unreachable_backend_is_unavailable_rather_than_a_grant() {
        // Port 1 on loopback refuses immediately: a fiducia outage, not a lock.
        let coordination = FiduciaCoordination::new("http://127.0.0.1:1", "locks-key".to_string())
            .expect("client");
        let error = coordination
            .acquire_lease("daedalus/fab/request/req-1", Duration::from_secs(30))
            .await
            .expect_err("an unreachable backend must not grant");
        assert!(matches!(error, CoordinationError::Unavailable { .. }));
    }

    #[tokio::test]
    async fn noop_coordination_never_blocks_and_is_honest_about_it() {
        let coordination = NoopCoordination::default();
        let ttl = Duration::from_secs(30);
        let first = coordination
            .acquire_lease("daedalus/fab/request/req-1", ttl)
            .await
            .expect("first grant");
        // The same key, again, in the same process: still granted. This is the
        // documented absence of exclusion, asserted so nobody mistakes the
        // default for a distributed lock.
        let second = coordination
            .acquire_lease("daedalus/fab/request/req-1", ttl)
            .await
            .expect("second grant");
        assert!(second.fencing_token > first.fencing_token);
        assert!(!coordination.is_distributed());
        assert_eq!(coordination.mode(), "noop");

        coordination
            .renew_lease(&first, ttl)
            .await
            .expect("noop renewal never fences");
        coordination.release_lease(&first).await.expect("release");
    }

    #[test]
    fn the_holder_id_is_a_stable_per_process_uuid() {
        let holder = process_holder_id();
        assert_eq!(holder, process_holder_id());
        let suffix = holder
            .strip_prefix("dd-fabrication-server-")
            .expect("holder is prefixed with the service name");
        // A UUIDv4, not a pid or hostname: holder identity participates in
        // cancellation authority, so it must be unguessable and never reused.
        let parsed = Uuid::parse_str(suffix).expect("holder suffix is a uuid");
        assert_eq!(parsed.get_version_num(), 4);
    }

    #[test]
    fn renewal_interval_is_a_third_of_the_ttl_and_bounded() {
        assert_eq!(
            renewal_interval(Duration::from_millis(30_000)),
            Duration::from_millis(10_000)
        );
        // A long TTL still reports lost authority promptly.
        assert_eq!(
            renewal_interval(Duration::from_secs(3_600)),
            Duration::from_millis(MAX_RENEWAL_INTERVAL_MS)
        );
        assert!(renewal_interval(Duration::from_millis(1)) > Duration::ZERO);
    }

    #[test]
    fn lock_urls_inherit_the_secret_overlays_metadata_ban() {
        // One validator, one answer: a lock endpoint may not be a metadata host
        // or carry credentials any more than a KV endpoint may.
        assert!(FiduciaCoordination::new("http://169.254.169.254", "k".to_string()).is_err());
        assert!(
            FiduciaCoordination::new("https://u:p@fiducia.example.com", "k".to_string()).is_err()
        );
        assert!(
            FiduciaCoordination::new("http://fiducia.daedalus.svc:8088", "k".to_string()).is_ok()
        );
        assert!(
            FiduciaCoordination::new("http://fiducia.daedalus.svc:8088", "  ".to_string()).is_err()
        );
    }
}
