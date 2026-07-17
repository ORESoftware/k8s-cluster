//! Async fiducia.cloud coordination client.
//!
//! Request payloads come from the generated `fiducia-interfaces` crate pinned
//! to `github.com/fiducia-cloud/fiducia-interfaces`. The upstream native Rust
//! transport is intentionally blocking and does not yet support public bearer
//! tokens, so this service keeps Tokio's executor non-blocking with `reqwest`
//! while consuming the canonical shared contract types.

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use fiducia_interfaces::{
    CampaignRequest, ElectionGetResponse, HoldRequest,
    LockAcquireManyRequest as FiduciaLockAcquireManyRequest,
    LockReleaseRequest as FiduciaLockReleaseRequest, RenewRequest as ElectionRenewRequest,
};
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use crate::config::Config;
use crate::error::{AppError, AppResult};

#[derive(Clone)]
pub struct FiduciaCoordinator {
    enabled: bool,
    base_url: Url,
    client: reqwest::Client,
    api_key: Option<String>,
    request_timeout: Duration,
}

impl fmt::Debug for FiduciaCoordinator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let auth = match &self.api_key {
            None => "none",
            Some(_) => "api_key:<redacted>",
        };
        f.debug_struct("FiduciaCoordinator")
            .field("enabled", &self.enabled)
            .field("base_url", &self.base_url)
            .field("credentials", &auth)
            .field("request_timeout", &self.request_timeout)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FiduciaLockGrant {
    pub fencing_token: u64,
    pub lease_expires_ms: i64,
    pub keys: Vec<String>,
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
    leadership: Option<fiducia_interfaces::Leadership>,
}

#[derive(Debug, Deserialize)]
struct RenewOutput {
    renewed: bool,
    #[serde(default)]
    leadership: Option<fiducia_interfaces::Leadership>,
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
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(request_timeout)
            .build()?;
        Ok(Self {
            enabled: cfg.fiducia_enabled,
            base_url: Url::parse(&cfg.fiducia_base_url)?,
            client,
            api_key: cfg.fiducia_api_key.clone(),
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
        let url = self.endpoint(&["healthz"])?;
        let response = self
            .authorized(self.client.get(url))
            .send()
            .await
            .map_err(transport_error)?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(status_error(response.status(), None))
        }
    }

    pub async fn acquire_lock(
        &self,
        keys: Vec<String>,
        holder: &str,
        ttl_ms: u64,
    ) -> AppResult<Option<FiduciaLockGrant>> {
        self.require_enabled()?;
        let request = FiduciaLockAcquireManyRequest {
            keys,
            holder: Some(holder.to_string()),
            ttl_ms: Some(i64::try_from(ttl_ms).map_err(|_| {
                AppError::BadRequest("Fiducia lock TTL exceeds i64 milliseconds".into())
            })?),
            // Callers perform bounded try-lock polling. Fiducia's queued wait
            // is durable, so timing out a queued request could otherwise grant
            // a lock after the caller has already abandoned the operation.
            wait: Some(false),
        };
        let output: LockGrantOutput = self
            .commit(Method::POST, &["v1", "locks", "acquire"], &request)
            .await?;
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
        Ok(Some(FiduciaLockGrant {
            fencing_token,
            lease_expires_ms,
            keys: output.keys,
        }))
    }

    pub async fn release_lock(&self, holder: &str, fencing_token: u64) -> AppResult<bool> {
        self.require_enabled()?;
        let request = FiduciaLockReleaseRequest {
            holder: holder.to_string(),
            fencing_token: i64::try_from(fencing_token)
                .map_err(|_| protocol_error("Fiducia fencing token exceeds i64"))?,
        };
        let output: LockReleaseOutput = self
            .commit(Method::POST, &["v1", "locks", "release"], &request)
            .await?;
        if !output.released {
            tracing::debug!(reason = ?output.reason, "Fiducia lock was already released");
        }
        Ok(output.released)
    }

    pub async fn campaign_lease(
        &self,
        name: &str,
        candidate: &str,
        ttl_ms: u64,
        metadata: BTreeMap<String, String>,
    ) -> AppResult<Option<fiducia_interfaces::Leadership>> {
        self.require_enabled()?;
        let request = CampaignRequest {
            candidate: candidate.to_string(),
            ttl_ms: i64::try_from(ttl_ms)
                .map_err(|_| AppError::BadRequest("lease TTL exceeds i64 milliseconds".into()))?,
            metadata: Some(metadata),
        };
        let output: CampaignOutput = self
            .commit(
                Method::POST,
                &["v1", "elections", name, "campaign"],
                &request,
            )
            .await?;
        if output.won {
            Ok(Some(output.leadership.ok_or_else(|| {
                protocol_error("lease campaign won without leadership details")
            })?))
        } else {
            Ok(None)
        }
    }

    pub async fn get_lease(&self, name: &str) -> AppResult<ElectionGetResponse> {
        self.require_enabled()?;
        self.request_json(
            Method::GET,
            &["v1", "elections", name],
            Option::<&()>::None,
            None,
        )
        .await
    }

    pub async fn renew_lease(
        &self,
        name: &str,
        candidate: &str,
        fencing_token: u64,
        ttl_ms: u64,
    ) -> AppResult<Option<fiducia_interfaces::Leadership>> {
        self.require_enabled()?;
        let request =
            ElectionRenewRequest {
                candidate: candidate.to_string(),
                fencing_token: i64::try_from(fencing_token)
                    .map_err(|_| protocol_error("Fiducia fencing token exceeds i64"))?,
                ttl_ms: Some(i64::try_from(ttl_ms).map_err(|_| {
                    AppError::BadRequest("lease TTL exceeds i64 milliseconds".into())
                })?),
            };
        let output: RenewOutput = self
            .commit(Method::POST, &["v1", "elections", name, "renew"], &request)
            .await?;
        if output.renewed {
            Ok(Some(output.leadership.ok_or_else(|| {
                protocol_error("lease renew succeeded without leadership details")
            })?))
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
        let request = HoldRequest {
            candidate: candidate.to_string(),
            fencing_token: i64::try_from(fencing_token)
                .map_err(|_| protocol_error("Fiducia fencing token exceeds i64"))?,
        };
        let output: ResignOutput = self
            .commit(Method::POST, &["v1", "elections", name, "resign"], &request)
            .await?;
        Ok(output.resigned)
    }

    async fn commit<B, R>(&self, method: Method, segments: &[&str], body: &B) -> AppResult<R>
    where
        B: Serialize + ?Sized,
        R: DeserializeOwned,
    {
        let idempotency_key = format!("billing-server-rs/{}", uuid::Uuid::new_v4());
        let envelope: CommitEnvelope = self
            .request_json(method, segments, Some(body), Some(&idempotency_key))
            .await?;
        if !envelope.committed {
            return Err(protocol_error(&format!(
                "coordination mutation was not committed: {}",
                envelope.error.unwrap_or(Value::Null)
            )));
        }
        let output = envelope
            .result
            .ok_or_else(|| protocol_error("committed response omitted result"))?
            .output;
        serde_json::from_value(output)
            .map_err(|err| protocol_error(&format!("invalid committed output: {err}")))
    }

    async fn request_json<B, R>(
        &self,
        method: Method,
        segments: &[&str],
        body: Option<&B>,
        idempotency_key: Option<&str>,
    ) -> AppResult<R>
    where
        B: Serialize + ?Sized,
        R: DeserializeOwned,
    {
        let url = self.endpoint(segments)?;
        let mut request = self.client.request(method, url);
        if let Some(idempotency_key) = idempotency_key {
            request = request.header("Idempotency-Key", idempotency_key);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = self
            .authorized(request)
            .send()
            .await
            .map_err(transport_error)?;
        let status = response.status();
        let bytes = response.bytes().await.map_err(transport_error)?;
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice::<Value>(&bytes)
                .map_err(|err| protocol_error(&format!("response was not valid JSON: {err}")))?
        };
        if !status.is_success() {
            return Err(status_error(status, Some(value)));
        }
        serde_json::from_value(value)
            .map_err(|err| protocol_error(&format!("response shape mismatch: {err}")))
    }

    fn authorized(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.api_key {
            None => request,
            Some(api_key) => request.bearer_auth(api_key),
        }
    }

    fn endpoint(&self, segments: &[&str]) -> AppResult<Url> {
        let mut url = self.base_url.clone();
        {
            let mut path = url
                .path_segments_mut()
                .map_err(|_| protocol_error("Fiducia base URL cannot hold path segments"))?;
            path.pop_if_empty();
            for segment in segments {
                path.push(segment);
            }
        }
        Ok(url)
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

fn transport_error(err: reqwest::Error) -> AppError {
    AppError::Provider {
        provider: "fiducia.cloud".into(),
        message: format!("transport failure: {err}"),
    }
}

fn protocol_error(message: &str) -> AppError {
    AppError::Provider {
        provider: "fiducia.cloud".into(),
        message: message.to_string(),
    }
}

fn status_error(status: StatusCode, body: Option<Value>) -> AppError {
    let detail = body
        .and_then(|value| {
            value
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    value
                        .get("message")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
        })
        .unwrap_or_else(|| "request rejected".into());
    AppError::Provider {
        provider: "fiducia.cloud".into(),
        message: format!("HTTP {}: {detail}", status.as_u16()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Json;
    use axum::Router;
    use axum::extract::State;
    use axum::http::HeaderMap;
    use axum::routing::post;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn path_segments_encode_slash_safe_lease_names() {
        let coordinator = FiduciaCoordinator::disabled();
        let url = coordinator
            .endpoint(&["v1", "elections", "billing/tenant/a/resource", "renew"])
            .unwrap();
        assert_eq!(
            url.as_str(),
            "http://127.0.0.1:8090/v1/elections/billing%2Ftenant%2Fa%2Fresource/renew"
        );
    }

    #[test]
    fn debug_output_redacts_credentials() {
        let mut cfg = Config::for_tests();
        cfg.fiducia_api_key = Some("fdc_live_secret".into());
        let coordinator = FiduciaCoordinator::from_config(&cfg).unwrap();
        let debug = format!("{coordinator:?}");
        assert!(!debug.contains("fdc_live_secret"));
        assert!(debug.contains("<redacted>"));
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
        assert!(coordinator.release_lock("holder-1", 41).await.unwrap());
        assert_eq!(calls.load(Ordering::Relaxed), 2);
        server.abort();
    }
}
