//! Async adapter over the canonical blocking `fiducia-clients` SDK.
//!
//! The SDK owns route encoding, redirect refusal, bearer handling, retry safety,
//! and shared generated contracts. This service runs SDK calls on Tokio's
//! blocking pool so coordination cannot stall the async executor.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use fiducia_client::{
    Error as FiduciaClientError, FiduciaClient, RequestControl,
    types::{ElectionGetResponse, Leadership},
};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use url::Url;

use crate::config::Config;
use crate::error::{AppError, AppResult};

#[derive(Clone)]
pub struct FiduciaCoordinator {
    enabled: bool,
    base_url: Url,
    client: Arc<FiduciaClient>,
    request_timeout: Duration,
}

impl fmt::Debug for FiduciaCoordinator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FiduciaCoordinator")
            .field("enabled", &self.enabled)
            .field("base_url", &self.base_url)
            .field("client", &self.client)
            .field("request_timeout", &self.request_timeout)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FiduciaLockGrant {
    /// Scalar token. For a single-key grant this is authoritative; for a
    /// multi-key (union) grant the node only guarantees it mirrors *one* of
    /// the per-key tokens, so it must not be applied to the other keys.
    pub fencing_token: u64,
    pub lease_expires_ms: i64,
    pub keys: Vec<String>,
    /// Per-key tokens for a union grant. Empty for a single-key grant, where
    /// the scalar `fencing_token` is the key's token.
    pub fencing_tokens: BTreeMap<String, u64>,
}

impl FiduciaLockGrant {
    /// The fencing token that actually guards `key`. A union grant carries one
    /// token per key; using the scalar for every key (as this code previously
    /// did) both misreports the fence and releases the other keys with a
    /// mismatched token, stranding them until the lease TTL expires.
    pub fn token_for(&self, key: &str) -> Option<u64> {
        if self.fencing_tokens.is_empty() {
            // Single-key grant: the scalar is this key's token, but only if the
            // grant really covers the key we were asked about.
            return self
                .keys
                .iter()
                .any(|k| k == key)
                .then_some(self.fencing_token);
        }
        self.fencing_tokens.get(key).copied()
    }

    /// Every (key, token) pair this grant holds, in deterministic key order.
    pub fn per_key_tokens(&self) -> Vec<(String, u64)> {
        if self.fencing_tokens.is_empty() {
            return self
                .keys
                .iter()
                .map(|key| (key.clone(), self.fencing_token))
                .collect();
        }
        self.fencing_tokens
            .iter()
            .map(|(key, token)| (key.clone(), *token))
            .collect()
    }
}

#[derive(Debug, Deserialize)]
struct LockGrantOutput {
    acquired: bool,
    #[serde(default)]
    fencing_token: Option<u64>,
    #[serde(default)]
    lease_expires_ms: Option<i64>,
    #[serde(default)]
    keys: Vec<String>,
    /// Per-key tokens for a union grant. Absent for single-key grants.
    #[serde(default)]
    fencing_tokens: BTreeMap<String, u64>,
    #[serde(default)]
    conflicts: Vec<String>,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LockReleaseOutput {
    released: bool,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CampaignOutput {
    won: bool,
    #[serde(default)]
    leadership: Option<Leadership>,
}

#[derive(Debug, Deserialize)]
struct RenewOutput {
    renewed: bool,
    #[serde(default)]
    leadership: Option<Leadership>,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ResignOutput {
    resigned: bool,
}

#[derive(Debug, Deserialize)]
struct CommitEnvelope {
    committed: bool,
    #[serde(default)]
    result: Option<CommitResult>,
    #[serde(default)]
    error: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct CommitResult {
    output: Value,
}

impl FiduciaCoordinator {
    pub fn from_config(cfg: &Config) -> anyhow::Result<Self> {
        let request_timeout = Duration::from_millis(cfg.fiducia_request_timeout_ms);
        let base_url = Url::parse(&cfg.fiducia_base_url)?;
        let mut client = match cfg.fiducia_api_key.as_deref() {
            Some(api_key) => FiduciaClient::bearer(base_url.as_str(), api_key),
            None => FiduciaClient::new(base_url.as_str()),
        };
        client.request_timeout = Some(request_timeout);
        client.lock_request_timeout = Some(request_timeout);
        Ok(Self {
            enabled: cfg.fiducia_enabled,
            base_url,
            client: Arc::new(client),
            request_timeout,
        })
    }

    #[cfg(test)]
    pub fn disabled() -> Self {
        let cfg = Config::for_tests();
        Self::from_config(&cfg).expect("test config is valid")
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn base_url(&self) -> &str {
        self.base_url.as_str()
    }

    pub fn request_timeout(&self) -> Duration {
        self.request_timeout
    }

    pub async fn health(&self) -> AppResult<()> {
        if !self.enabled {
            return Ok(());
        }
        self.call(FiduciaClient::health).await.map(|_| ())
    }

    pub async fn acquire_lock(
        &self,
        keys: Vec<String>,
        holder: &str,
        ttl_ms: u64,
    ) -> AppResult<Option<FiduciaLockGrant>> {
        self.require_enabled()?;
        i64::try_from(ttl_ms).map_err(|_| {
            AppError::BadRequest("Fiducia lock TTL exceeds i64 milliseconds".into())
        })?;
        let holder = holder.to_string();
        let control = self.mutation_control();
        let response = self
            .call(move |client| {
                let key_refs = keys.iter().map(String::as_str).collect::<Vec<_>>();
                // Callers perform bounded try-lock polling. Fiducia's durable
                // wait queue must not grant this request after our deadline.
                client.lock_acquire_many_with_options(
                    &key_refs,
                    Some(&holder),
                    Some(ttl_ms),
                    false,
                    control,
                )
            })
            .await?;
        let output: LockGrantOutput = committed_output(response)?;
        if !output.acquired {
            tracing::debug!(
                conflicts = ?output.conflicts,
                reason = ?output.reason,
                "Fiducia lock not acquired"
            );
            return Ok(None);
        }
        let fencing_token = output.fencing_token.ok_or_else(|| {
            protocol_error("lock acquire committed with acquired=true but no fencing_token")
        })?;
        let lease_expires_ms = output.lease_expires_ms.ok_or_else(|| {
            protocol_error("lock acquire committed with acquired=true but no lease_expires_ms")
        })?;
        // A union grant must carry a token for every key it claims to hold.
        // Silently accepting a partial map would fence some keys and leave the
        // rest unguarded, which is worse than refusing the grant outright.
        if !output.fencing_tokens.is_empty() {
            if let Some(missing) = output
                .keys
                .iter()
                .find(|key| !output.fencing_tokens.contains_key(*key))
            {
                return Err(protocol_error(&format!(
                    "lock acquire returned per-key fencing tokens but omitted key '{missing}'"
                )));
            }
            if output.fencing_tokens.values().any(|token| *token == 0) {
                return Err(protocol_error(
                    "lock acquire returned a zero per-key fencing token",
                ));
            }
        }
        if fencing_token == 0 {
            return Err(protocol_error(
                "lock acquire committed with acquired=true but a zero fencing_token",
            ));
        }
        Ok(Some(FiduciaLockGrant {
            fencing_token,
            lease_expires_ms,
            keys: output.keys,
            fencing_tokens: output.fencing_tokens,
        }))
    }

    pub async fn release_lock(&self, holder: &str, fencing_token: u64) -> AppResult<bool> {
        self.require_enabled()?;
        i64::try_from(fencing_token)
            .map_err(|_| protocol_error("Fiducia fencing token exceeds i64"))?;
        let holder = holder.to_string();
        let control = self.mutation_control();
        let response = self
            .call(move |client| {
                client.lock_release_with_options("", &holder, fencing_token, control)
            })
            .await?;
        let output: LockReleaseOutput = committed_output(response)?;
        if !output.released {
            tracing::debug!(reason = ?output.reason, "Fiducia lock was already released");
        }
        Ok(output.released)
    }

    /// Release every token a grant holds. A union grant has one token per key
    /// and the release payload is `{holder, fencing_token}`, so each distinct
    /// token needs its own call — releasing only the scalar leaves the other
    /// keys held until the lease TTL expires.
    ///
    /// Every token is attempted even if an earlier one fails, so one bad key
    /// cannot strand the rest; the first error is returned afterwards.
    pub async fn release_grant(&self, holder: &str, grant: &FiduciaLockGrant) -> AppResult<bool> {
        let mut tokens: Vec<u64> = grant
            .per_key_tokens()
            .into_iter()
            .map(|(_, token)| token)
            .collect();
        tokens.sort_unstable();
        tokens.dedup();
        if tokens.is_empty() {
            tokens.push(grant.fencing_token);
        }

        let mut released_all = true;
        let mut first_error = None;
        for token in tokens {
            match self.release_lock(holder, token).await {
                Ok(released) => released_all &= released,
                Err(err) => {
                    tracing::error!(
                        error = %err,
                        holder,
                        fencing_token = token,
                        "failed to release a Fiducia union-lock member key"
                    );
                    if first_error.is_none() {
                        first_error = Some(err);
                    }
                    released_all = false;
                }
            }
        }
        match first_error {
            Some(err) => Err(err),
            None => Ok(released_all),
        }
    }

    pub async fn campaign_lease(
        &self,
        name: &str,
        candidate: &str,
        ttl_ms: u64,
        metadata: BTreeMap<String, String>,
    ) -> AppResult<Option<Leadership>> {
        self.require_enabled()?;
        i64::try_from(ttl_ms)
            .map_err(|_| AppError::BadRequest("lease TTL exceeds i64 milliseconds".into()))?;
        let name = name.to_string();
        let expected_candidate = candidate.to_string();
        let request_candidate = expected_candidate.clone();
        let control = self.mutation_control();
        let response = self
            .call(move |client| {
                client.election_campaign_with_options(
                    &name,
                    &request_candidate,
                    ttl_ms,
                    Some(json!(metadata)),
                    control,
                )
            })
            .await?;
        let output: CampaignOutput = committed_output(response)?;
        if output.won {
            let leadership = output
                .leadership
                .ok_or_else(|| protocol_error("lease campaign won without leadership details"))?;
            Ok(Some(validate_leadership(
                leadership,
                &expected_candidate,
                None,
            )?))
        } else {
            Ok(None)
        }
    }

    pub async fn get_lease(&self, name: &str) -> AppResult<ElectionGetResponse> {
        self.require_enabled()?;
        let name = name.to_string();
        let response = self.call(move |client| client.election_get(&name)).await?;
        deserialize_response(response, "lease lookup response")
    }

    pub async fn renew_lease(
        &self,
        name: &str,
        candidate: &str,
        fencing_token: u64,
        ttl_ms: u64,
    ) -> AppResult<Option<Leadership>> {
        self.require_enabled()?;
        i64::try_from(fencing_token)
            .map_err(|_| protocol_error("Fiducia fencing token exceeds i64"))?;
        i64::try_from(ttl_ms)
            .map_err(|_| AppError::BadRequest("lease TTL exceeds i64 milliseconds".into()))?;
        let name = name.to_string();
        let expected_candidate = candidate.to_string();
        let request_candidate = expected_candidate.clone();
        let control = self.mutation_control();
        let response = self
            .call(move |client| {
                client.election_renew_with_options(
                    &name,
                    &request_candidate,
                    fencing_token,
                    Some(ttl_ms),
                    control,
                )
            })
            .await?;
        let output: RenewOutput = committed_output(response)?;
        if output.renewed {
            let leadership = output.leadership.ok_or_else(|| {
                protocol_error("lease renew succeeded without leadership details")
            })?;
            Ok(Some(validate_leadership(
                leadership,
                &expected_candidate,
                Some(fencing_token),
            )?))
        } else {
            tracing::debug!(reason = ?output.reason, "Fiducia lease was not renewed");
            Ok(None)
        }
    }

    pub async fn resign_lease(
        &self,
        name: &str,
        candidate: &str,
        fencing_token: u64,
    ) -> AppResult<bool> {
        self.require_enabled()?;
        i64::try_from(fencing_token)
            .map_err(|_| protocol_error("Fiducia fencing token exceeds i64"))?;
        let name = name.to_string();
        let candidate = candidate.to_string();
        let control = self.mutation_control();
        let response = self
            .call(move |client| {
                client.election_resign_with_options(&name, &candidate, fencing_token, control)
            })
            .await?;
        let output: ResignOutput = committed_output(response)?;
        Ok(output.resigned)
    }

    fn mutation_control(&self) -> RequestControl {
        RequestControl {
            timeout: Some(self.request_timeout),
            lock_request_timeout: Some(self.request_timeout),
            max_retries: 1,
            retry_delay: Duration::from_millis(25),
            idempotency_key: Some(format!("billing-server-rs/{}", uuid::Uuid::new_v4())),
        }
    }

    async fn call<F>(&self, operation: F) -> AppResult<Value>
    where
        F: FnOnce(&FiduciaClient) -> Result<Value, FiduciaClientError> + Send + 'static,
    {
        let client = Arc::clone(&self.client);
        tokio::task::spawn_blocking(move || operation(client.as_ref()))
            .await
            .map_err(|err| protocol_error(&format!("Fiducia client task failed: {err}")))?
            .map_err(client_error)
    }

    fn require_enabled(&self) -> AppResult<()> {
        if self.enabled {
            Ok(())
        } else {
            Err(AppError::Provider {
                provider: "fiducia.cloud".into(),
                message: "coordination is disabled".into(),
            })
        }
    }
}

fn committed_output<R: DeserializeOwned>(response: Value) -> AppResult<R> {
    let envelope: CommitEnvelope = deserialize_response(response, "commit response")?;
    if !envelope.committed {
        let detail = response_detail(envelope.error).unwrap_or_else(|| "request rejected".into());
        return Err(protocol_error(&format!(
            "coordination mutation was not committed: {detail}"
        )));
    }
    let output = envelope
        .result
        .ok_or_else(|| protocol_error("committed response omitted result"))?
        .output;
    deserialize_response(output, "committed output")
}

fn deserialize_response<R: DeserializeOwned>(value: Value, context: &str) -> AppResult<R> {
    serde_json::from_value(value)
        .map_err(|err| protocol_error(&format!("invalid {context}: {err}")))
}

fn client_error(error: FiduciaClientError) -> AppError {
    match error {
        FiduciaClientError::Http { status, body } => status_error(status, body),
        FiduciaClientError::Transport(message) => AppError::Provider {
            provider: "fiducia.cloud".into(),
            message: format!("transport failure: {message}"),
        },
    }
}

fn protocol_error(message: &str) -> AppError {
    AppError::Provider {
        provider: "fiducia.cloud".into(),
        message: message.to_string(),
    }
}

/// Accept a leadership response only when it proves the caller still owns the
/// same fenced term. A stale, malformed, or cross-tenant response must never be
/// converted into a valid local billing lease.
fn validate_leadership(
    leadership: Leadership,
    candidate: &str,
    expected_fencing_token: Option<u64>,
) -> AppResult<Leadership> {
    if leadership.leader != candidate {
        return Err(protocol_error(
            "Fiducia leadership response names a different candidate",
        ));
    }
    let fencing_token = u64::try_from(leadership.fencing_token)
        .map_err(|_| protocol_error("Fiducia leadership response has a negative fencing token"))?;
    if fencing_token == 0 {
        return Err(protocol_error(
            "Fiducia leadership response has a zero fencing token",
        ));
    }
    if let Some(expected) = expected_fencing_token {
        if fencing_token != expected {
            return Err(protocol_error(
                "Fiducia lease renew changed the fencing token",
            ));
        }
    }
    Ok(leadership)
}

fn status_error(status: u16, body: Option<Value>) -> AppError {
    let detail = response_detail(body).unwrap_or_else(|| "request rejected".into());
    AppError::Provider {
        provider: "fiducia.cloud".into(),
        message: format!("HTTP {status}: {detail}"),
    }
}

fn response_detail(body: Option<Value>) -> Option<String> {
    let value = body?;
    let detail = value
        .get("error")
        .and_then(Value::as_str)
        .or_else(|| value.get("message").and_then(Value::as_str))?;
    Some(detail.chars().take(256).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Json;
    use axum::Router;
    use axum::extract::{Path, State};
    use axum::http::HeaderMap;
    use axum::routing::post;
    use fiducia_client::types::{
        CampaignRequest, HoldRequest, LockAcquireManyRequest as FiduciaLockAcquireManyRequest,
        LockReleaseRequest as FiduciaLockReleaseRequest, RenewRequest,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn debug_output_redacts_credentials() {
        let mut cfg = Config::for_tests();
        cfg.fiducia_api_key = Some("fdc_live_secret".into());
        let coordinator = FiduciaCoordinator::from_config(&cfg).unwrap();
        let debug = format!("{coordinator:?}");
        assert!(!debug.contains("fdc_live_secret"));
        assert!(debug.contains("<redacted>"));
    }

    #[test]
    fn leadership_validation_requires_the_expected_candidate_and_term() {
        let leadership = Leadership {
            leader: "billing-worker-a".into(),
            fencing_token: 41,
            lease_expires_ms: 1_900_000_000_000,
            ttl_ms: 60_000,
            metadata: BTreeMap::new(),
        };
        assert!(validate_leadership(leadership.clone(), "billing-worker-a", Some(41)).is_ok());
        assert!(validate_leadership(leadership.clone(), "billing-worker-b", None).is_err());
        assert!(validate_leadership(leadership, "billing-worker-a", Some(42)).is_err());
    }

    #[test]
    fn leadership_validation_rejects_invalid_fencing_tokens() {
        let mut leadership = Leadership {
            leader: "billing-worker-a".into(),
            fencing_token: 0,
            lease_expires_ms: 1_900_000_000_000,
            ttl_ms: 60_000,
            metadata: BTreeMap::new(),
        };
        assert!(validate_leadership(leadership.clone(), "billing-worker-a", None).is_err());
        leadership.fencing_token = -1;
        assert!(validate_leadership(leadership, "billing-worker-a", None).is_err());
    }

    #[tokio::test]
    async fn lock_calls_use_bearer_idempotency_and_canonical_payloads() {
        let calls = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route(
                "/v1/locks/acquire",
                post(
                    |State(calls): State<Arc<AtomicUsize>>,
                     headers: HeaderMap,
                     Json(request): Json<FiduciaLockAcquireManyRequest>| async move {
                        assert_eq!(headers.get("authorization").unwrap(), "Bearer fdc_test.key");
                        assert!(
                            headers
                                .get("idempotency-key")
                                .unwrap()
                                .to_str()
                                .unwrap()
                                .starts_with("billing-server-rs/")
                        );
                        assert_eq!(request.keys, vec!["billing:customer:t:c"]);
                        assert_eq!(request.holder.as_deref(), Some("holder-1"));
                        assert_eq!(request.ttl_ms, Some(60_000));
                        assert_eq!(request.wait, Some(false));
                        calls.fetch_add(1, Ordering::Relaxed);
                        Json(serde_json::json!({
                            "committed": true,
                            "result": {"output": {
                                "acquired": true,
                                "fencing_token": 41,
                                "lease_expires_ms": 1_900_000_000_000_i64,
                                "keys": request.keys
                            }}
                        }))
                    },
                ),
            )
            .route(
                "/v1/locks/release",
                post(
                    |State(calls): State<Arc<AtomicUsize>>,
                     headers: HeaderMap,
                     Json(request): Json<FiduciaLockReleaseRequest>| async move {
                        assert_eq!(headers.get("authorization").unwrap(), "Bearer fdc_test.key");
                        assert!(headers.contains_key("idempotency-key"));
                        assert_eq!(request.holder, "holder-1");
                        assert_eq!(request.fencing_token, 41);
                        calls.fetch_add(1, Ordering::Relaxed);
                        Json(serde_json::json!({
                            "committed": true,
                            "result": {"output": {"released": true}}
                        }))
                    },
                ),
            )
            .with_state(calls.clone());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let mut cfg = Config::for_tests();
        cfg.fiducia_enabled = true;
        cfg.fiducia_base_url = format!("http://{address}");
        cfg.fiducia_api_key = Some("fdc_test.key".into());
        let coordinator = FiduciaCoordinator::from_config(&cfg).unwrap();
        let grant = coordinator
            .acquire_lock(vec!["billing:customer:t:c".into()], "holder-1", 60_000)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(grant.fencing_token, 41);
        let renewed = coordinator
            .acquire_lock(vec!["billing:customer:t:c".into()], "holder-1", 60_000)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(renewed.fencing_token, grant.fencing_token);
        assert_eq!(renewed.keys, grant.keys);
        assert!(coordinator.release_lock("holder-1", 41).await.unwrap());
        assert_eq!(calls.load(Ordering::Relaxed), 3);
        server.abort();
    }

    #[tokio::test]
    async fn lease_calls_use_sdk_encoding_idempotency_and_canonical_payloads() {
        let calls = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route(
                "/v1/elections/{name}/campaign",
                post(
                    |State(calls): State<Arc<AtomicUsize>>,
                     Path(name): Path<String>,
                     headers: HeaderMap,
                     Json(request): Json<CampaignRequest>| async move {
                        assert_eq!(name, "billing/tenant/a/resource");
                        assert_eq!(headers.get("authorization").unwrap(), "Bearer fdc_test.key");
                        assert!(headers.contains_key("idempotency-key"));
                        assert_eq!(request.candidate, "replica-a");
                        assert_eq!(request.ttl_ms, 60_000);
                        assert_eq!(
                            request.metadata.unwrap().get("region").map(String::as_str),
                            Some("us-east")
                        );
                        calls.fetch_add(1, Ordering::Relaxed);
                        Json(serde_json::json!({
                            "committed": true,
                            "result": {"output": {
                                "won": true,
                                "leadership": {
                                    "name": name,
                                    "leader": "replica-a",
                                    "fencing_token": 73,
                                    "lease_expires_ms": 1_900_000_000_000_i64,
                                    "ttl_ms": 60_000,
                                    "metadata": {"region": "us-east"}
                                }
                            }}
                        }))
                    },
                ),
            )
            .route(
                "/v1/elections/{name}/renew",
                post(
                    |State(calls): State<Arc<AtomicUsize>>,
                     Path(name): Path<String>,
                     headers: HeaderMap,
                     Json(request): Json<RenewRequest>| async move {
                        assert_eq!(name, "billing/tenant/a/resource");
                        assert!(headers.contains_key("idempotency-key"));
                        assert_eq!(request.candidate, "replica-a");
                        assert_eq!(request.fencing_token, 73);
                        assert_eq!(request.ttl_ms, Some(60_000));
                        calls.fetch_add(1, Ordering::Relaxed);
                        Json(serde_json::json!({
                            "committed": true,
                            "result": {"output": {
                                "renewed": true,
                                "leadership": {
                                    "name": name,
                                    "leader": "replica-a",
                                    "fencing_token": 73,
                                    "lease_expires_ms": 1_900_000_060_000_i64,
                                    "ttl_ms": 60_000,
                                    "metadata": {"region": "us-east"}
                                }
                            }}
                        }))
                    },
                ),
            )
            .route(
                "/v1/elections/{name}/resign",
                post(
                    |State(calls): State<Arc<AtomicUsize>>,
                     Path(name): Path<String>,
                     headers: HeaderMap,
                     Json(request): Json<HoldRequest>| async move {
                        assert_eq!(name, "billing/tenant/a/resource");
                        assert!(headers.contains_key("idempotency-key"));
                        assert_eq!(request.candidate, "replica-a");
                        assert_eq!(request.fencing_token, 73);
                        calls.fetch_add(1, Ordering::Relaxed);
                        Json(serde_json::json!({
                            "committed": true,
                            "result": {"output": {"resigned": true}}
                        }))
                    },
                ),
            )
            .with_state(calls.clone());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut cfg = Config::for_tests();
        cfg.fiducia_enabled = true;
        cfg.fiducia_base_url = format!("http://{address}");
        cfg.fiducia_api_key = Some("fdc_test.key".into());
        let coordinator = FiduciaCoordinator::from_config(&cfg).unwrap();
        let name = "billing/tenant/a/resource";
        let leadership = coordinator
            .campaign_lease(
                name,
                "replica-a",
                60_000,
                BTreeMap::from([("region".into(), "us-east".into())]),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(leadership.fencing_token, 73);
        assert!(
            coordinator
                .renew_lease(name, "replica-a", 73, 60_000)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            coordinator
                .resign_lease(name, "replica-a", 73)
                .await
                .unwrap()
        );
        assert_eq!(calls.load(Ordering::Relaxed), 3);
        server.abort();
    }

    fn union_grant() -> FiduciaLockGrant {
        FiduciaLockGrant {
            fencing_token: 7,
            lease_expires_ms: 0,
            keys: vec!["a".into(), "b".into()],
            fencing_tokens: BTreeMap::from([("a".to_string(), 7), ("b".to_string(), 9)]),
        }
    }

    #[test]
    fn union_grant_fences_each_key_with_its_own_token() {
        let grant = union_grant();
        // The scalar mirrors only one member; "b" must NOT inherit it.
        assert_eq!(grant.token_for("a"), Some(7));
        assert_eq!(grant.token_for("b"), Some(9));
        assert_eq!(grant.token_for("missing"), None);
        assert_eq!(
            grant.per_key_tokens(),
            vec![("a".to_string(), 7), ("b".to_string(), 9)]
        );
    }

    #[test]
    fn single_key_grant_uses_the_scalar_token_only_for_keys_it_holds() {
        let grant = FiduciaLockGrant {
            fencing_token: 4,
            lease_expires_ms: 0,
            keys: vec!["solo".into()],
            fencing_tokens: BTreeMap::new(),
        };
        assert_eq!(grant.token_for("solo"), Some(4));
        // A key the grant never covered must never borrow the scalar.
        assert_eq!(grant.token_for("other"), None);
        assert_eq!(grant.per_key_tokens(), vec![("solo".to_string(), 4)]);
    }

    #[test]
    fn union_grant_release_covers_every_distinct_token() {
        // Releasing only the scalar (7) would leave "b" (9) held until TTL.
        let tokens: Vec<u64> = union_grant()
            .per_key_tokens()
            .into_iter()
            .map(|(_, token)| token)
            .collect();
        assert!(tokens.contains(&7) && tokens.contains(&9));
    }

    #[test]
    fn partial_per_key_token_map_is_rejected() {
        // A union grant claiming a key with no token would leave that key
        // unfenced; the acquire path must refuse the whole grant.
        let output = json!({
            "acquired": true,
            "fencing_token": 7,
            "lease_expires_ms": 1,
            "keys": ["a", "b"],
            "fencing_tokens": { "a": 7 }
        });
        let parsed: LockGrantOutput = serde_json::from_value(output).unwrap();
        let missing = parsed
            .keys
            .iter()
            .find(|key| !parsed.fencing_tokens.contains_key(*key));
        assert_eq!(missing, Some(&"b".to_string()));
    }
}
