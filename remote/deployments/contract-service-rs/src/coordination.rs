//! Cross-replica broadcast coordination.
//!
//! Solana broadcasts are protected by two independent, fail-closed fences:
//! a transaction-scoped Postgres advisory lock and Fiducia's durable
//! idempotency lease. The key is a SHA-256 digest of the already-signed
//! transaction, so every route that relays the same transaction converges on
//! the same fence without exposing transaction bytes to either coordinator.
//!
//! PostgreSQL access goes through SeaORM. The advisory-lock statements remain
//! explicit because they are PostgreSQL coordination primitives rather than
//! application-table CRUD. Their values are still bound parameters; no SQL is
//! assembled from caller data and no migration runs from this service.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::time::Duration;

use reqwest::{Client, Url};
use sea_orm::{
    ConnectOptions, Database, DatabaseConnection, DatabaseTransaction, DbBackend,
    FromQueryResult, Statement, TransactionTrait,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const DEFAULT_LEASE_MS: u64 = 5 * 60 * 1_000;
const DEFAULT_RETENTION_MS: u64 = 7 * 24 * 60 * 60 * 1_000;
const MAX_COORDINATION_RESPONSE_BYTES: u64 = 1024 * 1024;
const POSTGRES_TIMEOUT: Duration = Duration::from_secs(5);
const READINESS_TIMEOUT: Duration = Duration::from_secs(2);
const READINESS_SQL: &str = "select 1 as value";
const ADVISORY_LOCK_SQL: &str =
    "select pg_try_advisory_xact_lock($1) as acquired";

#[derive(Clone)]
pub(crate) struct CoordinationState {
    inner: Option<Arc<CoordinationInner>>,
    required: bool,
    metrics: Arc<CoordinationMetrics>,
}

struct CoordinationInner {
    database: DatabaseConnection,
    client: Client,
    fiducia_url: String,
    fiducia_api_key: String,
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

#[derive(Debug, FromQueryResult)]
struct ReadinessRow {
    value: i32,
}

#[derive(Debug, FromQueryResult)]
struct AdvisoryLockRow {
    acquired: bool,
}

pub(crate) enum BeginOutcome {
    Acquired(CoordinationLease),
    Replay(Value),
}

pub(crate) struct CoordinationLease {
    transaction: Option<DatabaseTransaction>,
    inner: Arc<CoordinationInner>,
    metrics: Arc<CoordinationMetrics>,
    key: String,
    owner: String,
    fencing_token: u64,
}

impl CoordinationState {
    pub(crate) async fn from_env(client: Client) -> Result<Self, String> {
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
        let fiducia_api_key = crate::env_secret("FIDUCIA_API_KEY").ok_or_else(|| {
            "CONTRACT_COORDINATION_ENABLED=true requires a requests:write-scoped FIDUCIA_API_KEY"
                .to_string()
        })?;
        let pool_size = crate::env_u64("CONTRACT_COORDINATION_PG_POOL_SIZE", 4).clamp(1, 16) as u32;
        let mut options = ConnectOptions::new(database_url);
        options
            .max_connections(pool_size)
            .min_connections(0)
            .connect_timeout(POSTGRES_TIMEOUT)
            .acquire_timeout(POSTGRES_TIMEOUT)
            .sqlx_logging(false);
        let database = Database::connect(options)
            .await
            .map_err(|error| format!("postgres coordination connection failed: {error}"))?;
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
                database,
                client,
                fiducia_url,
                fiducia_api_key,
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
            READINESS_TIMEOUT,
            ReadinessRow::find_by_statement(readiness_statement()).one(&inner.database),
        );
        // OPTIONS on the write-only claim route is non-mutating, but the Fiducia
        // edge still applies the route's requests:write/admin:write scope check
        // before forwarding it. Axum commonly answers 405 after authorization;
        // that status therefore proves the credential reached the protected
        // route without creating an idempotency record.
        let fiducia_request = inner
            .client
            .request(
                reqwest::Method::OPTIONS,
                format!("{}/v1/idempotency/claim", inner.fiducia_url),
            )
            .bearer_auth(&inner.fiducia_api_key);
        let fiducia = tokio::time::timeout(READINESS_TIMEOUT, fiducia_request.send());
        let (postgres, fiducia) = tokio::join!(postgres, fiducia);

        match postgres {
            Ok(Ok(Some(ReadinessRow { value: 1 }))) => {}
            Ok(Ok(Some(_))) | Ok(Ok(None)) => {
                return Err("postgres coordination readiness returned unexpected value".to_string())
            }
            Ok(Err(error)) => return Err(format!("postgres coordination unavailable: {error}")),
            Err(_) => return Err("postgres coordination readiness timed out".to_string()),
        }
        match fiducia {
            Ok(Ok(response))
                if response.status().is_success()
                    || response.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED =>
            {
                Ok(())
            }
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

        let transaction = inner.database.begin().await.map_err(|error| {
            self.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
            format!("postgres coordination unavailable: {error}")
        })?;
        let acquired = AdvisoryLockRow::find_by_statement(advisory_lock_statement(advisory_key))
            .one(&transaction)
            .await
            .map_err(|error| {
                self.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                format!("postgres advisory lock failed: {error}")
            })?
            .map(|row| row.acquired)
            .ok_or_else(|| {
                self.metrics.errors_total.fetch_add(1, Ordering::Relaxed);
                "postgres advisory lock returned no row".to_string()
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

fn readiness_statement() -> Statement {
    Statement::from_string(DbBackend::Postgres, READINESS_SQL.to_owned())
}

fn advisory_lock_statement(advisory_key: i64) -> Statement {
    Statement::from_sql_and_values(
        DbBackend::Postgres,
        ADVISORY_LOCK_SQL,
        [advisory_key.into()],
    )
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
    let request = inner
        .client
        .post(format!("{}{path}", inner.fiducia_url))
        .bearer_auth(&inner.fiducia_api_key)
        .json(&payload);
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
    fn statements_preserve_parameterized_postgres_semantics() {
        let readiness = readiness_statement();
        assert_eq!(readiness.sql, READINESS_SQL);
        assert!(readiness.values.is_none());

        let advisory = advisory_lock_statement(42);
        assert_eq!(advisory.sql, ADVISORY_LOCK_SQL);
        assert_eq!(advisory.db_backend, DbBackend::Postgres);
        let values = format!("{:?}", advisory.values);
        assert!(values.contains("42"), "advisory lock key must remain bound");
    }

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
