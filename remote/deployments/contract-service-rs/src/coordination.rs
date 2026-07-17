//! Cross-replica broadcast coordination.
//!
//! Solana broadcasts are protected by two independent, fail-closed fences:
//! a transaction-scoped Postgres advisory lock and Fiducia's durable
//! idempotency lease. The key is a SHA-256 digest of the already-signed
//! transaction, so every route that relays the same transaction converges on
//! the same fence without exposing transaction bytes to either coordinator.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use reqwest::{Client, Url};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{postgres::PgPoolOptions, PgPool, Postgres, Transaction};

const DEFAULT_LEASE_MS: u64 = 5 * 60 * 1_000;
const DEFAULT_RETENTION_MS: u64 = 7 * 24 * 60 * 60 * 1_000;
const MAX_COORDINATION_RESPONSE_BYTES: u64 = 1024 * 1024;

#[derive(Clone)]
pub(crate) struct CoordinationState {
    inner: Option<Arc<CoordinationInner>>,
    required: bool,
    metrics: Arc<CoordinationMetrics>,
}

struct CoordinationInner {
    pool: PgPool,
    client: Client,
    fiducia_url: String,
    fiducia_api_key: Option<String>,
    owner: String,
    lease_ms: u64,
    retention_ms: u64,
}

#[derive(Default)]
struct CoordinationMetrics {
    acquired_total: AtomicU64,
    replayed_total: AtomicU64,
    contended_total: AtomicU64,
    errors_total: AtomicU64,
    completed_total: AtomicU64,
}

pub(crate) enum BeginOutcome {
    Acquired(CoordinationLease),
    Replay(Value),
}

pub(crate) struct CoordinationLease {
    transaction: Option<Transaction<'static, Postgres>>,
    inner: Arc<CoordinationInner>,
    metrics: Arc<CoordinationMetrics>,
    key: String,
    owner: String,
    fencing_token: u64,
}

impl CoordinationState {
    pub(crate) fn from_env(client: Client) -> Result<Self, String> {
        let enabled = crate::env_bool("CONTRACT_COORDINATION_ENABLED", false);
        let required = crate::env_bool("CONTRACT_COORDINATION_REQUIRED", false);
        let metrics = Arc::new(CoordinationMetrics::default());
        if !enabled {
            if required {
                return Err(
                    "CONTRACT_COORDINATION_REQUIRED=true requires CONTRACT_COORDINATION_ENABLED=true"
                        .to_string(),
                );
            }
            return Ok(Self {
                inner: None,
                required,
                metrics,
            });
        }

        let database_url = crate::env_secret("CONTRACT_DATABASE_URL")
            .or_else(|| crate::env_secret("RDS_DATABASE_URL"))
            .ok_or_else(|| {
                "broadcast coordination requires CONTRACT_DATABASE_URL or RDS_DATABASE_URL"
                    .to_string()
            })?;
        let fiducia_url = validate_fiducia_url(&crate::env_value(
            "FIDUCIA_LOCK_URL",
            "http://fiducia-load-balance.fiducia.svc.cluster.local:8088",
        ))?;
        let pool_size = crate::env_u64("CONTRACT_COORDINATION_PG_POOL_SIZE", 4).clamp(1, 16) as u32;
        let pool = PgPoolOptions::new()
            .max_connections(pool_size)
            .acquire_timeout(std::time::Duration::from_secs(5))
            .connect_lazy(&database_url)
            .map_err(|error| format!("invalid coordination database URL: {error}"))?;
        let owner = crate::env_value(
            "CONTRACT_COORDINATION_HOLDER",
            &crate::env_value("HOSTNAME", "dd-contract-service"),
        );
        let lease_ms = crate::env_u64("FIDUCIA_IDEMPOTENCY_LEASE_MS", DEFAULT_LEASE_MS)
            .clamp(30_000, 15 * 60 * 1_000);
        let retention_ms = crate::env_u64("FIDUCIA_IDEMPOTENCY_RETENTION_MS", DEFAULT_RETENTION_MS)
            .clamp(lease_ms, 30 * 24 * 60 * 60 * 1_000);

        Ok(Self {
            inner: Some(Arc::new(CoordinationInner {
                pool,
                client,
                fiducia_url,
                fiducia_api_key: crate::env_secret("FIDUCIA_API_KEY"),
                owner,
                lease_ms,
                retention_ms,
            })),
            required,
            metrics,
        })
    }

    pub(crate) fn enabled(&self) -> bool {
        self.inner.is_some()
    }

    pub(crate) fn required(&self) -> bool {
        self.required
    }

    pub(crate) async fn readiness(&self) -> Result<(), String> {
        let Some(inner) = self.inner.clone() else {
            return if self.required {
                Err("broadcast coordination is required but not configured".to_string())
            } else {
                Ok(())
            };
        };

        let postgres = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            sqlx::query_scalar::<_, i32>("select 1").fetch_one(&inner.pool),
        );
        let mut fiducia_request = inner.client.get(format!("{}/readyz", inner.fiducia_url));
        if let Some(api_key) = &inner.fiducia_api_key {
            fiducia_request = fiducia_request.bearer_auth(api_key);
        }
        let fiducia =
            tokio::time::timeout(std::time::Duration::from_secs(2), fiducia_request.send());
        let (postgres, fiducia) = tokio::join!(postgres, fiducia);

        match postgres {
            Ok(Ok(1)) => {}
            Ok(Ok(_)) => {
                return Err("postgres coordination readiness returned unexpected value".to_string())
            }
            Ok(Err(error)) => return Err(format!("postgres coordination unavailable: {error}")),
            Err(_) => return Err("postgres coordination readiness timed out".to_string()),
        }
        match fiducia {
            Ok(Ok(response)) if response.status().is_success() => Ok(()),
            Ok(Ok(response)) => Err(format!(
                "Fiducia coordination readiness returned HTTP {}",
                response.status()
            )),
            Ok(Err(error)) => Err(format!("Fiducia coordination unavailable: {error}")),
            Err(_) => Err("Fiducia coordination readiness timed out".to_string()),
        }
    }

    #[cfg(test)]
    pub(crate) fn disabled_for_tests() -> Self {
        Self {
            inner: None,
            required: false,
            metrics: Arc::new(CoordinationMetrics::default()),
        }
    }

    pub(crate) async fn begin_broadcast(
        &self,
        signed_transaction: &[u8],
    ) -> Result<BeginOutcome, String> {
        let Some(inner) = self.inner.clone() else {
            if self.required {
                self.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                return Err("broadcast coordination is required but not configured".to_string());
            }
            return Err("broadcast coordination is disabled".to_string());
        };

        let digest = Sha256::digest(signed_transaction);
        let digest_hex = hex::encode(digest);
        let advisory_key = i64::from_be_bytes(digest[..8].try_into().expect("sha256 prefix"));
        let key = format!("solana/broadcast/{digest_hex}");
        let owner = format!("{}:{}", inner.owner, &digest_hex[..16]);

        let mut transaction = inner.pool.begin().await.map_err(|error| {
            self.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            format!("postgres coordination unavailable: {error}")
        })?;
        let acquired: bool = sqlx::query_scalar("select pg_try_advisory_xact_lock($1)")
            .bind(advisory_key)
            .fetch_one(&mut *transaction)
            .await
            .map_err(|error| {
                self.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                format!("postgres advisory lock failed: {error}")
            })?;
        if !acquired {
            self.metrics.contended_total.fetch_add(1, Ordering::Relaxed);
            let _ = transaction.rollback().await;
            return Err("broadcast is already in progress for this signed transaction".to_string());
        }

        let claim = fiducia_post(
            &inner,
            "/v1/idempotency/claim",
            json!({
                "key": key,
                "owner": owner,
                "ttl_ms": inner.lease_ms,
                "retention_ms": inner.retention_ms,
                "metadata": {
                    "service": crate::SERVICE_NAME,
                    "operation": "solana.sendTransaction",
                    "transactionDigest": digest_hex,
                }
            }),
        )
        .await;

        let claim = match claim {
            Ok(value) => proposal_output(&value),
            Err(error) => {
                self.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                let _ = transaction.rollback().await;
                return Err(error);
            }
        };
        if claim.get("claimed").and_then(Value::as_bool) == Some(true) {
            let Some(fencing_token) = claim.get("fencing_token").and_then(Value::as_u64) else {
                self.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                let _ = transaction.rollback().await;
                return Err("Fiducia claim omitted fencing_token".to_string());
            };
            self.metrics.acquired_total.fetch_add(1, Ordering::Relaxed);
            return Ok(BeginOutcome::Acquired(CoordinationLease {
                transaction: Some(transaction),
                inner,
                metrics: self.metrics.clone(),
                key,
                owner,
                fencing_token,
            }));
        }

        let _ = transaction.rollback().await;
        if claim.get("duplicate").and_then(Value::as_bool) == Some(true) {
            if let Some(result) = claim.pointer("/record/result/rpcResult").cloned() {
                self.metrics.replayed_total.fetch_add(1, Ordering::Relaxed);
                return Ok(BeginOutcome::Replay(result));
            }
            self.metrics.contended_total.fetch_add(1, Ordering::Relaxed);
            return Err("broadcast idempotency lease is already held".to_string());
        }

        self.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
        Err(format!(
            "Fiducia refused broadcast coordination: {}",
            claim
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("unknown reason")
        ))
    }

    pub(crate) fn render_metrics(&self, out: &mut String) {
        let m = &self.metrics;
        out.push_str("# HELP dd_contract_service_coordination_total Cross-replica Solana broadcast coordination outcomes.\n# TYPE dd_contract_service_coordination_total counter\n");
        for (outcome, value) in [
            ("acquired", m.acquired_total.load(Ordering::Relaxed)),
            ("replayed", m.replayed_total.load(Ordering::Relaxed)),
            ("contended", m.contended_total.load(Ordering::Relaxed)),
            ("completed", m.completed_total.load(Ordering::Relaxed)),
            ("error", m.errors_total.load(Ordering::Relaxed)),
        ] {
            out.push_str(&format!(
                "dd_contract_service_coordination_total{{outcome=\"{outcome}\"}} {value}\n"
            ));
        }
    }
}

impl CoordinationLease {
    pub(crate) async fn complete(mut self, rpc_result: &Value) -> Result<(), String> {
        let completed = fiducia_post(
            &self.inner,
            "/v1/idempotency/complete",
            json!({
                "key": self.key,
                "owner": self.owner,
                "fencing_token": self.fencing_token,
                "result": { "rpcResult": rpc_result }
            }),
        )
        .await
        .map(|value| proposal_output(&value));
        let committed = match completed {
            Ok(value)
                if value.get("completed").and_then(Value::as_bool) == Some(true)
                    || value.pointer("/record/status").and_then(Value::as_str)
                        == Some("completed") =>
            {
                Ok(())
            }
            Ok(value) => Err(format!(
                "Fiducia did not complete idempotency record: {}",
                value
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown reason")
            )),
            Err(error) => Err(error),
        };

        if let Some(transaction) = self.transaction.take() {
            let _ = transaction.rollback().await;
        }
        match committed {
            Ok(()) => {
                self.metrics.completed_total.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(error) => {
                self.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                Err(error)
            }
        }
    }

    pub(crate) async fn abandon(mut self) {
        let _ = fiducia_post(
            &self.inner,
            "/v1/idempotency/abandon",
            json!({
                "key": self.key,
                "owner": self.owner,
                "fencing_token": self.fencing_token,
            }),
        )
        .await;
        if let Some(transaction) = self.transaction.take() {
            let _ = transaction.rollback().await;
        }
    }
}

fn validate_fiducia_url(raw: &str) -> Result<String, String> {
    let parsed =
        Url::parse(raw).map_err(|error| format!("FIDUCIA_LOCK_URL is invalid: {error}"))?;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("FIDUCIA_LOCK_URL must not include credentials".to_string());
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| "FIDUCIA_LOCK_URL must include a host".to_string())?;
    let internal_http = parsed.scheme() == "http"
        && (host == "localhost" || host == "127.0.0.1" || host.ends_with(".svc.cluster.local"));
    if parsed.scheme() != "https" && !internal_http {
        return Err(
            "FIDUCIA_LOCK_URL must use https or an in-cluster .svc.cluster.local http URL"
                .to_string(),
        );
    }
    Ok(raw.trim_end_matches('/').to_string())
}

async fn fiducia_post(
    inner: &CoordinationInner,
    path: &str,
    payload: Value,
) -> Result<Value, String> {
    let mut request = inner
        .client
        .post(format!("{}{path}", inner.fiducia_url))
        .json(&payload);
    if let Some(api_key) = &inner.fiducia_api_key {
        request = request.bearer_auth(api_key);
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("Fiducia coordination request failed: {error}"))?;
    let status = response.status();
    if response.content_length().unwrap_or(0) > MAX_COORDINATION_RESPONSE_BYTES {
        return Err("Fiducia coordination response exceeded size limit".to_string());
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("Fiducia coordination response failed: {error}"))?;
    if bytes.len() as u64 > MAX_COORDINATION_RESPONSE_BYTES {
        return Err("Fiducia coordination response exceeded size limit".to_string());
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Fiducia coordination response was not JSON: {error}"))?;
    if !status.is_success() {
        return Err(format!("Fiducia coordination returned HTTP {status}"));
    }
    Ok(value)
}

fn proposal_output(value: &Value) -> Value {
    value
        .pointer("/result/output")
        .cloned()
        .unwrap_or_else(|| value.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fiducia_url_policy_allows_cluster_service_and_https() {
        assert!(
            validate_fiducia_url("http://fiducia-load-balance.fiducia.svc.cluster.local:8088")
                .is_ok()
        );
        assert!(validate_fiducia_url("https://api.fiducia.cloud").is_ok());
        assert!(validate_fiducia_url("http://api.fiducia.cloud").is_err());
        assert!(validate_fiducia_url("https://user:pass@api.fiducia.cloud").is_err());
    }

    #[test]
    fn proposal_output_accepts_direct_and_consensus_envelopes() {
        let direct = json!({ "claimed": true, "fencing_token": 4 });
        assert_eq!(proposal_output(&direct), direct);
        let wrapped = json!({ "result": { "output": { "claimed": true } } });
        assert_eq!(proposal_output(&wrapped), json!({ "claimed": true }));
    }
}
