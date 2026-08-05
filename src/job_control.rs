//! Durable fabrication job control.
//!
//! PostgreSQL is the canonical state machine and recovery ledger. NATS
//! JetStream is used only for durable delivery/wakeups. Long-running workers
//! acquire a Fiducia lease and every PostgreSQL mutation is additionally
//! serialized by a transaction-scoped advisory lock and checked against the
//! persisted Fiducia fencing token.
//!
//! This module intentionally never executes DDL. The reviewed contract lives
//! in `doc/database/durable-job-control.sql` until it is promoted into the
//! canonical `k8s-libs-and-shared-defs/pg-defs/schema/schema.sql`.

use std::{
    env,
    error::Error,
    fmt, io,
    path::Path,
    time::{Duration, Instant},
};

use async_nats::jetstream;
use reqwest::StatusCode;
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, DbErr,
    QueryResult, Statement, TransactionTrait, Value,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use uuid::Uuid;

pub type JobControlResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

pub const EXECUTIONS_TABLE: &str = "daedalus.fabrication_job_executions";
pub const OUTBOX_TABLE: &str = "daedalus.fabrication_job_outbox";
pub const REQUEST_SUBJECT: &str = dd_nats_subject_defs::FABRICATION_REQUESTS_SUBJECT;
pub const RESULT_SUBJECT: &str = dd_nats_subject_defs::FABRICATION_RESULTS_SUBJECT;

const DEFAULT_DB_MAX_CONNECTIONS: u32 = 8;
const DEFAULT_OUTBOX_CLAIM_SECS: u64 = 60;
const DEFAULT_JOB_LEASE_SECS: u64 = 120;
const DEFAULT_FIDUCIA_WAIT_SECS: u64 = 5;

fn invalid(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(io::Error::new(io::ErrorKind::InvalidInput, message.into()))
}

fn failed_precondition(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(io::Error::new(io::ErrorKind::Other, message.into()))
}

fn postgres(sql: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

fn require_nonempty(name: &str, value: &str) -> JobControlResult<()> {
    if value.trim().is_empty() {
        return Err(invalid(format!("{name} must not be empty")));
    }
    Ok(())
}

fn env_truthy(name: &str, default: bool) -> bool {
    env::var(name)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(default)
}

fn env_u64(name: &str, default: u64) -> JobControlResult<u64> {
    match env::var(name) {
        Ok(value) => value
            .trim()
            .parse::<u64>()
            .map_err(|error| invalid(format!("{name} must be an unsigned integer: {error}"))),
        Err(_) => Ok(default),
    }
}

fn database_url_from_env() -> JobControlResult<String> {
    for name in [
        "FABRICATION_DATABASE_URL",
        "RDS_DATABASE_URL",
        "DATABASE_URL",
    ] {
        if let Ok(value) = env::var(name) {
            if !value.trim().is_empty() {
                return Ok(value);
            }
        }
    }
    Err(invalid(
        "set FABRICATION_DATABASE_URL, RDS_DATABASE_URL, or DATABASE_URL",
    ))
}

#[derive(Clone)]
pub struct JobStore {
    database: DatabaseConnection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnqueueRequest {
    pub tenant_id: String,
    pub request_id: String,
    pub idempotency_key: String,
    pub kind: String,
    pub request_payload: JsonValue,
    pub max_attempts: i32,
    pub priority: i16,
    pub subject: String,
}

impl EnqueueRequest {
    pub fn validate(&self) -> JobControlResult<()> {
        require_nonempty("tenant_id", &self.tenant_id)?;
        require_nonempty("request_id", &self.request_id)?;
        require_nonempty("idempotency_key", &self.idempotency_key)?;
        require_nonempty("kind", &self.kind)?;
        require_nonempty("subject", &self.subject)?;
        if self.max_attempts < 1 || self.max_attempts > 100 {
            return Err(invalid("max_attempts must be between 1 and 100"));
        }
        if !self.subject.starts_with("dd.remote.fabrication.") {
            return Err(invalid("subject must remain under dd.remote.fabrication.>"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobSnapshot {
    pub job_id: String,
    pub tenant_id: String,
    pub request_id: String,
    pub idempotency_key: String,
    pub kind: String,
    pub state: String,
    pub current_stage: String,
    pub checkpoint_version: i64,
    pub checkpoint: JsonValue,
    pub request_payload: JsonValue,
    pub result_payload: Option<JsonValue>,
    pub attempt_count: i32,
    pub max_attempts: i32,
    pub priority: i16,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<String>,
    pub fiducia_fencing_token: Option<i64>,
    pub next_attempt_at: String,
    pub last_error_code: Option<String>,
    pub last_error_message: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxEvent {
    pub event_id: String,
    pub job_id: String,
    pub subject: String,
    pub event_type: String,
    pub message_id: String,
    pub payload: JsonValue,
    pub publish_attempts: i32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReapDisposition {
    Requeued,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReapOutcome {
    pub job_id: String,
    pub disposition: ReapDisposition,
    pub attempt_count: i32,
    pub checkpoint_version: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatchReport {
    pub claimed: usize,
    pub published: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReapReport {
    pub examined: usize,
    pub requeued: usize,
    pub failed: usize,
    pub skipped: usize,
    pub outcomes: Vec<ReapOutcome>,
}

impl JobStore {
    pub async fn connect_from_env() -> JobControlResult<Self> {
        let database_url = database_url_from_env()?;
        Self::connect(&database_url).await
    }

    pub async fn connect(database_url: &str) -> JobControlResult<Self> {
        require_nonempty("database_url", database_url)?;
        let mut options = ConnectOptions::new(database_url.to_owned());
        options
            .max_connections(DEFAULT_DB_MAX_CONNECTIONS)
            .min_connections(0)
            .connect_timeout(Duration::from_secs(10))
            .acquire_timeout(Duration::from_secs(10))
            .idle_timeout(Duration::from_secs(120))
            .sqlx_logging(false);
        let database = Database::connect(options).await?;
        Ok(Self { database })
    }

    pub async fn ensure_schema(&self) -> JobControlResult<()> {
        let row = self
            .database
            .query_one(postgres(
                r#"
SELECT
    to_regclass('daedalus.fabrication_job_executions')::text AS executions,
    to_regclass('daedalus.fabrication_job_outbox')::text AS outbox
"#,
                vec![],
            ))
            .await?
            .ok_or_else(|| failed_precondition("schema verification returned no row"))?;
        let executions: Option<String> = row.try_get("", "executions")?;
        let outbox: Option<String> = row.try_get("", "outbox")?;
        if executions.is_none() || outbox.is_none() {
            return Err(failed_precondition(
                "durable job-control tables are absent; apply the canonical shared-defs schema first",
            ));
        }
        Ok(())
    }

    pub async fn enqueue(&self, request: &EnqueueRequest) -> JobControlResult<JobSnapshot> {
        request.validate()?;
        let candidate_job_id = Uuid::new_v4().to_string();
        let transaction = self.database.begin().await?;

        let row = transaction
            .query_one(postgres(
                ENQUEUE_JOB_SQL,
                vec![
                    candidate_job_id.into(),
                    request.tenant_id.clone().into(),
                    request.request_id.clone().into(),
                    request.idempotency_key.clone().into(),
                    request.kind.clone().into(),
                    request.request_payload.clone().into(),
                    request.max_attempts.into(),
                    request.priority.into(),
                ],
            ))
            .await?
            .ok_or_else(|| failed_precondition("enqueue did not return a job"))?;
        let job = decode_job(&row)?;

        let message_id = queue_message_id(&job.job_id, job.checkpoint_version, "queued");
        let payload = queue_payload(&job, "queued");
        self.insert_outbox_in(
            &transaction,
            &job.job_id,
            &request.subject,
            "fabrication.job.queued.v1",
            &message_id,
            &payload,
            0,
        )
        .await?;

        transaction.commit().await?;
        Ok(job)
    }

    pub async fn get_job(
        &self,
        tenant_id: &str,
        job_id: &str,
    ) -> JobControlResult<Option<JobSnapshot>> {
        require_nonempty("tenant_id", tenant_id)?;
        validate_uuid("job_id", job_id)?;
        let row = self
            .database
            .query_one(postgres(
                &format!(
                    "{} WHERE tenant_id = $1 AND job_id = $2::uuid",
                    JOB_SELECT_SQL
                ),
                vec![tenant_id.to_owned().into(), job_id.to_owned().into()],
            ))
            .await?;
        row.as_ref().map(decode_job).transpose()
    }

    pub async fn claim_job(
        &self,
        tenant_id: &str,
        job_id: &str,
        owner: &str,
        fencing_token: i64,
        lease_secs: u64,
    ) -> JobControlResult<JobSnapshot> {
        validate_claim_inputs(tenant_id, job_id, owner, fencing_token, lease_secs)?;
        let transaction = self.database.begin().await?;
        advisory_xact_lock(&transaction, tenant_id, job_id).await?;

        let row = transaction
            .query_one(postgres(
                CLAIM_JOB_SQL,
                vec![
                    tenant_id.to_owned().into(),
                    job_id.to_owned().into(),
                    owner.to_owned().into(),
                    fencing_token.into(),
                    u64_to_i64("lease_secs", lease_secs)?.into(),
                ],
            ))
            .await?
            .ok_or_else(|| {
                failed_precondition(
                    "job is not claimable (active lease, terminal state, retry delay, attempts exhausted, or stale fencing token)",
                )
            })?;
        let job = decode_job(&row)?;
        transaction.commit().await?;
        Ok(job)
    }

    pub async fn renew_job_lease(
        &self,
        tenant_id: &str,
        job_id: &str,
        owner: &str,
        fencing_token: i64,
        lease_secs: u64,
    ) -> JobControlResult<JobSnapshot> {
        validate_claim_inputs(tenant_id, job_id, owner, fencing_token, lease_secs)?;
        let transaction = self.database.begin().await?;
        advisory_xact_lock(&transaction, tenant_id, job_id).await?;
        let row = transaction
            .query_one(postgres(
                RENEW_JOB_LEASE_SQL,
                vec![
                    tenant_id.to_owned().into(),
                    job_id.to_owned().into(),
                    owner.to_owned().into(),
                    fencing_token.into(),
                    u64_to_i64("lease_secs", lease_secs)?.into(),
                ],
            ))
            .await?
            .ok_or_else(|| {
                failed_precondition(
                    "job lease renewal rejected because ownership or fencing changed",
                )
            })?;
        let job = decode_job(&row)?;
        transaction.commit().await?;
        Ok(job)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn checkpoint(
        &self,
        tenant_id: &str,
        job_id: &str,
        owner: &str,
        fencing_token: i64,
        expected_version: i64,
        stage: &str,
        checkpoint: &JsonValue,
        lease_secs: u64,
    ) -> JobControlResult<JobSnapshot> {
        validate_claim_inputs(tenant_id, job_id, owner, fencing_token, lease_secs)?;
        if expected_version < 0 {
            return Err(invalid("expected_version must be non-negative"));
        }
        require_nonempty("stage", stage)?;
        let transaction = self.database.begin().await?;
        advisory_xact_lock(&transaction, tenant_id, job_id).await?;

        let row = transaction
            .query_one(postgres(
                CHECKPOINT_SQL,
                vec![
                    tenant_id.to_owned().into(),
                    job_id.to_owned().into(),
                    owner.to_owned().into(),
                    fencing_token.into(),
                    expected_version.into(),
                    stage.to_owned().into(),
                    checkpoint.clone().into(),
                    u64_to_i64("lease_secs", lease_secs)?.into(),
                ],
            ))
            .await?
            .ok_or_else(|| {
                failed_precondition(
                    "checkpoint rejected because ownership, fencing token, or checkpoint version changed",
                )
            })?;
        let job = decode_job(&row)?;
        transaction.commit().await?;
        Ok(job)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn complete(
        &self,
        tenant_id: &str,
        job_id: &str,
        owner: &str,
        fencing_token: i64,
        expected_version: i64,
        result_payload: &JsonValue,
        result_subject: &str,
    ) -> JobControlResult<JobSnapshot> {
        validate_terminal_inputs(
            tenant_id,
            job_id,
            owner,
            fencing_token,
            expected_version,
            result_subject,
        )?;
        let transaction = self.database.begin().await?;
        advisory_xact_lock(&transaction, tenant_id, job_id).await?;

        let row = transaction
            .query_one(postgres(
                COMPLETE_SQL,
                vec![
                    tenant_id.to_owned().into(),
                    job_id.to_owned().into(),
                    owner.to_owned().into(),
                    fencing_token.into(),
                    expected_version.into(),
                    result_payload.clone().into(),
                ],
            ))
            .await?
            .ok_or_else(|| {
                failed_precondition(
                    "completion rejected because ownership, fencing token, or checkpoint version changed",
                )
            })?;
        let job = decode_job(&row)?;
        let message_id = result_message_id(&job.job_id, job.checkpoint_version, "succeeded");
        self.insert_outbox_in(
            &transaction,
            &job.job_id,
            result_subject,
            "fabrication.job.succeeded.v1",
            &message_id,
            &terminal_payload(&job),
            0,
        )
        .await?;
        transaction.commit().await?;
        Ok(job)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn fail(
        &self,
        tenant_id: &str,
        job_id: &str,
        owner: &str,
        fencing_token: i64,
        expected_version: i64,
        error_code: &str,
        error_message: &str,
        retryable: bool,
        request_subject: &str,
        result_subject: &str,
    ) -> JobControlResult<JobSnapshot> {
        validate_terminal_inputs(
            tenant_id,
            job_id,
            owner,
            fencing_token,
            expected_version,
            result_subject,
        )?;
        require_nonempty("error_code", error_code)?;
        require_nonempty("error_message", error_message)?;
        require_nonempty("request_subject", request_subject)?;
        let transaction = self.database.begin().await?;
        advisory_xact_lock(&transaction, tenant_id, job_id).await?;

        let row = transaction
            .query_one(postgres(
                FAIL_SQL,
                vec![
                    tenant_id.to_owned().into(),
                    job_id.to_owned().into(),
                    owner.to_owned().into(),
                    fencing_token.into(),
                    expected_version.into(),
                    error_code.to_owned().into(),
                    error_message.to_owned().into(),
                    retryable.into(),
                ],
            ))
            .await?
            .ok_or_else(|| {
                failed_precondition(
                    "failure transition rejected because ownership, fencing token, or checkpoint version changed",
                )
            })?;
        let job = decode_job(&row)?;

        if job.state == "retry_wait" {
            let message_id = queue_message_id(&job.job_id, job.checkpoint_version, "retry");
            self.insert_outbox_in(
                &transaction,
                &job.job_id,
                request_subject,
                "fabrication.job.retry.v1",
                &message_id,
                &queue_payload(&job, "retry"),
                retry_delay_secs(job.attempt_count),
            )
            .await?;
        } else {
            let message_id = result_message_id(&job.job_id, job.checkpoint_version, "failed");
            self.insert_outbox_in(
                &transaction,
                &job.job_id,
                result_subject,
                "fabrication.job.failed.v1",
                &message_id,
                &terminal_payload(&job),
                0,
            )
            .await?;
        }
        transaction.commit().await?;
        Ok(job)
    }

    pub async fn claim_outbox_batch(
        &self,
        owner: &str,
        limit: u64,
        claim_secs: u64,
    ) -> JobControlResult<Vec<OutboxEvent>> {
        require_nonempty("owner", owner)?;
        if limit == 0 || limit > 1_000 {
            return Err(invalid("outbox batch limit must be between 1 and 1000"));
        }
        if claim_secs == 0 || claim_secs > 3_600 {
            return Err(invalid("outbox claim_secs must be between 1 and 3600"));
        }
        let rows = self
            .database
            .query_all(postgres(
                CLAIM_OUTBOX_SQL,
                vec![
                    owner.to_owned().into(),
                    u64_to_i64("limit", limit)?.into(),
                    u64_to_i64("claim_secs", claim_secs)?.into(),
                ],
            ))
            .await?;
        rows.iter().map(decode_outbox).collect()
    }

    pub async fn mark_outbox_published(&self, event_id: &str, owner: &str) -> JobControlResult<()> {
        validate_uuid("event_id", event_id)?;
        require_nonempty("owner", owner)?;
        let affected = self
            .database
            .execute(postgres(
                r#"
UPDATE daedalus.fabrication_job_outbox
SET published_at = clock_timestamp(),
    claim_owner = NULL,
    claim_expires_at = NULL,
    last_error = NULL,
    updated_at = clock_timestamp()
WHERE event_id = $1::uuid
  AND claim_owner = $2
  AND published_at IS NULL
"#,
                vec![event_id.to_owned().into(), owner.to_owned().into()],
            ))
            .await?
            .rows_affected();
        if affected != 1 {
            return Err(failed_precondition(
                "outbox publish acknowledgment lost ownership",
            ));
        }
        Ok(())
    }

    pub async fn release_outbox_claim(
        &self,
        event_id: &str,
        owner: &str,
        error_message: &str,
        delay_secs: u64,
    ) -> JobControlResult<()> {
        validate_uuid("event_id", event_id)?;
        require_nonempty("owner", owner)?;
        require_nonempty("error_message", error_message)?;
        let affected = self
            .database
            .execute(postgres(
                r#"
UPDATE daedalus.fabrication_job_outbox
SET available_at = clock_timestamp() + make_interval(secs => $4::double precision),
    claim_owner = NULL,
    claim_expires_at = NULL,
    last_error = left($3, 4000),
    updated_at = clock_timestamp()
WHERE event_id = $1::uuid
  AND claim_owner = $2
  AND published_at IS NULL
"#,
                vec![
                    event_id.to_owned().into(),
                    owner.to_owned().into(),
                    error_message.to_owned().into(),
                    u64_to_i64("delay_secs", delay_secs)?.into(),
                ],
            ))
            .await?
            .rows_affected();
        if affected != 1 {
            return Err(failed_precondition("outbox failure update lost ownership"));
        }
        Ok(())
    }

    pub async fn expired_job_candidates(
        &self,
        limit: u64,
    ) -> JobControlResult<Vec<(String, String)>> {
        if limit == 0 || limit > 1_000 {
            return Err(invalid("reaper limit must be between 1 and 1000"));
        }
        let rows = self
            .database
            .query_all(postgres(
                r#"
SELECT tenant_id, job_id::text AS job_id
FROM daedalus.fabrication_job_executions
WHERE state = 'running'
  AND lease_expires_at IS NOT NULL
  AND lease_expires_at <= clock_timestamp()
ORDER BY lease_expires_at, created_at
LIMIT $1
"#,
                vec![u64_to_i64("limit", limit)?.into()],
            ))
            .await?;
        rows.iter()
            .map(|row| Ok((row.try_get("", "tenant_id")?, row.try_get("", "job_id")?)))
            .collect::<Result<Vec<_>, DbErr>>()
            .map_err(Into::into)
    }

    pub async fn reap_expired_job(
        &self,
        tenant_id: &str,
        job_id: &str,
        request_subject: &str,
        result_subject: &str,
    ) -> JobControlResult<ReapOutcome> {
        require_nonempty("tenant_id", tenant_id)?;
        validate_uuid("job_id", job_id)?;
        require_nonempty("request_subject", request_subject)?;
        require_nonempty("result_subject", result_subject)?;

        let transaction = self.database.begin().await?;
        if !try_advisory_xact_lock(&transaction, tenant_id, job_id).await? {
            transaction.rollback().await?;
            return Ok(ReapOutcome {
                job_id: job_id.to_owned(),
                disposition: ReapDisposition::Skipped,
                attempt_count: 0,
                checkpoint_version: 0,
            });
        }

        let row = transaction
            .query_one(postgres(
                REAP_EXPIRED_SQL,
                vec![tenant_id.to_owned().into(), job_id.to_owned().into()],
            ))
            .await?;
        let Some(row) = row else {
            transaction.rollback().await?;
            return Ok(ReapOutcome {
                job_id: job_id.to_owned(),
                disposition: ReapDisposition::Skipped,
                attempt_count: 0,
                checkpoint_version: 0,
            });
        };
        let job = decode_job(&row)?;
        let disposition = if job.state == "retry_wait" {
            let message_id = queue_message_id(&job.job_id, job.checkpoint_version, "recovery");
            self.insert_outbox_in(
                &transaction,
                &job.job_id,
                request_subject,
                "fabrication.job.recovered.v1",
                &message_id,
                &queue_payload(&job, "lease_expired"),
                retry_delay_secs(job.attempt_count),
            )
            .await?;
            ReapDisposition::Requeued
        } else {
            let message_id = result_message_id(&job.job_id, job.checkpoint_version, "failed");
            self.insert_outbox_in(
                &transaction,
                &job.job_id,
                result_subject,
                "fabrication.job.failed.v1",
                &message_id,
                &terminal_payload(&job),
                0,
            )
            .await?;
            ReapDisposition::Failed
        };
        transaction.commit().await?;
        Ok(ReapOutcome {
            job_id: job.job_id,
            disposition,
            attempt_count: job.attempt_count,
            checkpoint_version: job.checkpoint_version,
        })
    }

    async fn insert_outbox_in<C: ConnectionTrait>(
        &self,
        connection: &C,
        job_id: &str,
        subject: &str,
        event_type: &str,
        message_id: &str,
        payload: &JsonValue,
        delay_secs: u64,
    ) -> JobControlResult<()> {
        validate_uuid("job_id", job_id)?;
        require_nonempty("subject", subject)?;
        require_nonempty("event_type", event_type)?;
        require_nonempty("message_id", message_id)?;
        let event_id = Uuid::new_v4().to_string();
        connection
            .execute(postgres(
                INSERT_OUTBOX_SQL,
                vec![
                    event_id.into(),
                    job_id.to_owned().into(),
                    subject.to_owned().into(),
                    event_type.to_owned().into(),
                    message_id.to_owned().into(),
                    payload.clone().into(),
                    u64_to_i64("delay_secs", delay_secs)?.into(),
                ],
            ))
            .await?;
        Ok(())
    }
}

pub struct NatsPublisher {
    context: jetstream::Context,
}

impl NatsPublisher {
    pub async fn connect_from_env(service_name: &str) -> JobControlResult<Self> {
        require_nonempty("service_name", service_name)?;
        let nats_url = env::var("NATS_URL")
            .map_err(|_| invalid("NATS_URL is required for outbox dispatch"))?;
        let mut options = async_nats::ConnectOptions::new()
            .name(service_name)
            .retry_on_initial_connect()
            .ping_interval(Duration::from_secs(15))
            .connection_timeout(Duration::from_secs(10));
        if env_truthy("NATS_REQUIRE_TLS", false) {
            options = options.require_tls(true);
        }
        if let Ok(path) = env::var("NATS_CREDENTIALS_FILE") {
            if !path.trim().is_empty() {
                options = options.credentials_file(path).await?;
            }
        } else if let Ok(token) = env::var("NATS_TOKEN") {
            if !token.trim().is_empty() {
                options = options.token(token);
            }
        } else if let Ok(seed) = env::var("NATS_NKEY") {
            if !seed.trim().is_empty() {
                options = options.nkey(seed);
            }
        }
        let client = options.connect(nats_url).await?;
        Ok(Self {
            context: jetstream::new(client),
        })
    }

    pub async fn publish(&self, event: &OutboxEvent) -> JobControlResult<()> {
        let mut headers = async_nats::HeaderMap::new();
        headers.append("Nats-Msg-Id", event.message_id.as_str());
        headers.append("Dd-Event-Type", event.event_type.as_str());
        headers.append("Dd-Job-Id", event.job_id.as_str());
        self.context
            .publish_with_headers(
                event.subject.clone(),
                headers,
                serde_json::to_vec(&event.payload)?.into(),
            )
            .await?
            .await?;
        Ok(())
    }
}

pub async fn dispatch_once(
    store: &JobStore,
    publisher: &NatsPublisher,
    owner: &str,
    limit: u64,
) -> JobControlResult<DispatchReport> {
    let claim_secs = env_u64("FABRICATION_OUTBOX_CLAIM_SECS", DEFAULT_OUTBOX_CLAIM_SECS)?;
    let events = store.claim_outbox_batch(owner, limit, claim_secs).await?;
    let mut report = DispatchReport {
        claimed: events.len(),
        ..DispatchReport::default()
    };
    for event in events {
        match publisher.publish(&event).await {
            Ok(()) => {
                store.mark_outbox_published(&event.event_id, owner).await?;
                report.published += 1;
            }
            Err(error) => {
                let delay = publish_retry_delay_secs(event.publish_attempts);
                store
                    .release_outbox_claim(&event.event_id, owner, &error.to_string(), delay)
                    .await?;
                report.failed += 1;
            }
        }
    }
    Ok(report)
}

pub async fn reap_once(
    store: &JobStore,
    limit: u64,
    request_subject: &str,
    result_subject: &str,
) -> JobControlResult<ReapReport> {
    let candidates = store.expired_job_candidates(limit).await?;
    let mut report = ReapReport {
        examined: candidates.len(),
        ..ReapReport::default()
    };
    for (tenant_id, job_id) in candidates {
        let outcome = store
            .reap_expired_job(&tenant_id, &job_id, request_subject, result_subject)
            .await?;
        match outcome.disposition {
            ReapDisposition::Requeued => report.requeued += 1,
            ReapDisposition::Failed => report.failed += 1,
            ReapDisposition::Skipped => report.skipped += 1,
        }
        report.outcomes.push(outcome);
    }
    Ok(report)
}

#[derive(Clone)]
pub struct FiduciaClient {
    http: reqwest::Client,
    base_url: String,
    auth_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FiduciaLeaseGrant {
    pub lease_id: String,
    pub lock_key: String,
    pub owner: String,
    pub fencing_token: i64,
    pub lease_expiry_epoch_ms: Option<i64>,
}

#[derive(Debug, Serialize)]
struct FiduciaAcquireRequest<'a> {
    lock_key: &'a str,
    owner: &'a str,
    lease_secs: u64,
    wait_secs: u64,
}

#[derive(Debug, Deserialize)]
struct FiduciaAcquireResponse {
    acquired: bool,
    lease_id: Option<String>,
    fencing_token: Option<i64>,
    lease_expiry_epoch_ms: Option<i64>,
}

#[derive(Debug, Serialize)]
struct FiduciaHeartbeatRequest<'a> {
    lease_id: &'a str,
    lease_secs: u64,
}

#[derive(Debug, Serialize)]
struct FiduciaReleaseRequest<'a> {
    lease_id: &'a str,
}

impl FiduciaClient {
    pub fn from_env() -> JobControlResult<Self> {
        let base_url = env::var("FIDUCIA_BASE_URL")
            .map_err(|_| invalid("FIDUCIA_BASE_URL is required for worker claims"))?;
        require_nonempty("FIDUCIA_BASE_URL", &base_url)?;

        let mut builder = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15));
        if let Ok(pem) = env::var("FIDUCIA_TLS_CA_PEM") {
            if !pem.trim().is_empty() {
                builder =
                    builder.add_root_certificate(reqwest::Certificate::from_pem(pem.as_bytes())?);
            }
        } else if let Ok(path) = env::var("FIDUCIA_TLS_CA_PATH") {
            if !path.trim().is_empty() {
                let bytes = std::fs::read(Path::new(&path))?;
                builder = builder.add_root_certificate(reqwest::Certificate::from_pem(&bytes)?);
            }
        }
        let http = builder.build()?;
        let auth_token = env::var("FIDUCIA_AUTH_TOKEN")
            .ok()
            .filter(|value| !value.trim().is_empty());
        Ok(Self {
            http,
            base_url: base_url.trim_end_matches('/').to_owned(),
            auth_token,
        })
    }

    pub async fn acquire(
        &self,
        lock_key: &str,
        owner: &str,
        lease_secs: u64,
        wait_secs: u64,
    ) -> JobControlResult<FiduciaLeaseGrant> {
        require_nonempty("lock_key", lock_key)?;
        require_nonempty("owner", owner)?;
        if lease_secs == 0 || lease_secs > 86_400 {
            return Err(invalid("lease_secs must be between 1 and 86400"));
        }
        if wait_secs > 300 {
            return Err(invalid("wait_secs must not exceed 300"));
        }
        let request = FiduciaAcquireRequest {
            lock_key,
            owner,
            lease_secs,
            wait_secs,
        };
        let response = self
            .authorize(
                self.http
                    .post(format!("{}/v1/locks/acquire", self.base_url)),
            )
            .json(&request)
            .send()
            .await?;
        let status = response.status();
        let body = response.bytes().await?;
        if !status.is_success() {
            return Err(failed_precondition(format!(
                "Fiducia acquire failed with {status}: {}",
                String::from_utf8_lossy(&body)
            )));
        }
        let decoded: FiduciaAcquireResponse = serde_json::from_slice(&body)?;
        if !decoded.acquired {
            return Err(failed_precondition("Fiducia lock was not acquired"));
        }
        let lease_id = decoded
            .lease_id
            .ok_or_else(|| failed_precondition("Fiducia omitted lease_id"))?;
        let fencing_token = decoded
            .fencing_token
            .ok_or_else(|| failed_precondition("Fiducia omitted fencing_token"))?;
        if fencing_token < 0 {
            return Err(failed_precondition(
                "Fiducia returned a negative fencing token",
            ));
        }
        Ok(FiduciaLeaseGrant {
            lease_id,
            lock_key: lock_key.to_owned(),
            owner: owner.to_owned(),
            fencing_token,
            lease_expiry_epoch_ms: decoded.lease_expiry_epoch_ms,
        })
    }

    pub async fn heartbeat(
        &self,
        grant: &mut FiduciaLeaseGrant,
        lease_secs: u64,
    ) -> JobControlResult<()> {
        let response = self
            .authorize(
                self.http
                    .post(format!("{}/v1/locks/heartbeat", self.base_url)),
            )
            .json(&FiduciaHeartbeatRequest {
                lease_id: &grant.lease_id,
                lease_secs,
            })
            .send()
            .await?;
        ensure_fiducia_success("heartbeat", response.status()).await?;
        Ok(())
    }

    pub async fn release(&self, grant: &FiduciaLeaseGrant) -> JobControlResult<()> {
        let response = self
            .authorize(
                self.http
                    .post(format!("{}/v1/locks/release", self.base_url)),
            )
            .json(&FiduciaReleaseRequest {
                lease_id: &grant.lease_id,
            })
            .send()
            .await?;
        ensure_fiducia_success("release", response.status()).await?;
        Ok(())
    }

    fn authorize(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.auth_token {
            Some(token) => builder.bearer_auth(token),
            None => builder,
        }
    }
}

async fn ensure_fiducia_success(operation: &str, status: StatusCode) -> JobControlResult<()> {
    if status.is_success() {
        return Ok(());
    }
    Err(failed_precondition(format!(
        "Fiducia {operation} failed with {status}"
    )))
}

pub struct ClaimedJobLease {
    store: JobStore,
    fiducia: FiduciaClient,
    grant: FiduciaLeaseGrant,
    job: JobSnapshot,
    lease_secs: u64,
    released: bool,
}

impl fmt::Debug for ClaimedJobLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClaimedJobLease")
            .field("job_id", &self.job.job_id)
            .field("tenant_id", &self.job.tenant_id)
            .field("owner", &self.grant.owner)
            .field("fencing_token", &self.grant.fencing_token)
            .field("lease_secs", &self.lease_secs)
            .field("released", &self.released)
            .finish()
    }
}

impl ClaimedJobLease {
    pub async fn acquire(
        store: JobStore,
        fiducia: FiduciaClient,
        tenant_id: &str,
        job_id: &str,
        owner: &str,
        lease_secs: u64,
        wait_secs: u64,
    ) -> JobControlResult<Self> {
        let lock_key = fiducia_job_lock_key(tenant_id, job_id)?;
        let grant = fiducia
            .acquire(&lock_key, owner, lease_secs, wait_secs)
            .await?;
        match store
            .claim_job(tenant_id, job_id, owner, grant.fencing_token, lease_secs)
            .await
        {
            Ok(job) => Ok(Self {
                store,
                fiducia,
                grant,
                job,
                lease_secs,
                released: false,
            }),
            Err(error) => {
                let _ = fiducia.release(&grant).await;
                Err(error)
            }
        }
    }

    pub fn job(&self) -> &JobSnapshot {
        &self.job
    }

    pub fn fencing_token(&self) -> i64 {
        self.grant.fencing_token
    }

    pub fn heartbeat_interval(&self) -> Duration {
        Duration::from_secs((self.lease_secs / 3).max(1))
    }

    pub async fn heartbeat(&mut self) -> JobControlResult<&JobSnapshot> {
        if self.released {
            return Err(failed_precondition("lease is already released"));
        }
        self.fiducia
            .heartbeat(&mut self.grant, self.lease_secs)
            .await?;
        self.job = self
            .store
            .renew_job_lease(
                &self.job.tenant_id,
                &self.job.job_id,
                &self.grant.owner,
                self.grant.fencing_token,
                self.lease_secs,
            )
            .await?;
        Ok(&self.job)
    }

    pub async fn checkpoint(
        &mut self,
        stage: &str,
        checkpoint: &JsonValue,
    ) -> JobControlResult<&JobSnapshot> {
        if self.released {
            return Err(failed_precondition("lease is already released"));
        }
        self.job = self
            .store
            .checkpoint(
                &self.job.tenant_id,
                &self.job.job_id,
                &self.grant.owner,
                self.grant.fencing_token,
                self.job.checkpoint_version,
                stage,
                checkpoint,
                self.lease_secs,
            )
            .await?;
        Ok(&self.job)
    }

    pub async fn complete(mut self, result_payload: &JsonValue) -> JobControlResult<JobSnapshot> {
        let completed = self
            .store
            .complete(
                &self.job.tenant_id,
                &self.job.job_id,
                &self.grant.owner,
                self.grant.fencing_token,
                self.job.checkpoint_version,
                result_payload,
                RESULT_SUBJECT,
            )
            .await?;
        self.fiducia.release(&self.grant).await?;
        self.released = true;
        Ok(completed)
    }

    pub async fn fail(
        mut self,
        error_code: &str,
        error_message: &str,
        retryable: bool,
    ) -> JobControlResult<JobSnapshot> {
        let failed = self
            .store
            .fail(
                &self.job.tenant_id,
                &self.job.job_id,
                &self.grant.owner,
                self.grant.fencing_token,
                self.job.checkpoint_version,
                error_code,
                error_message,
                retryable,
                REQUEST_SUBJECT,
                RESULT_SUBJECT,
            )
            .await?;
        self.fiducia.release(&self.grant).await?;
        self.released = true;
        Ok(failed)
    }

    pub async fn release(mut self) -> JobControlResult<()> {
        if !self.released {
            self.fiducia.release(&self.grant).await?;
            self.released = true;
        }
        Ok(())
    }
}

pub async fn run_lease_drill(
    store: JobStore,
    tenant_id: &str,
    job_id: &str,
    owner: &str,
    hold_for: Duration,
    complete: bool,
) -> JobControlResult<JobSnapshot> {
    let fiducia = FiduciaClient::from_env()?;
    let lease_secs = env_u64("FIDUCIA_LEASE_SECS", DEFAULT_JOB_LEASE_SECS)?;
    let wait_secs = env_u64("FIDUCIA_WAIT_SECS", DEFAULT_FIDUCIA_WAIT_SECS)?;
    let mut lease = ClaimedJobLease::acquire(
        store, fiducia, tenant_id, job_id, owner, lease_secs, wait_secs,
    )
    .await?;
    let deadline = Instant::now() + hold_for;
    let mut heartbeat = tokio::time::interval(lease.heartbeat_interval());
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut iteration = 0_u64;
    while Instant::now() < deadline {
        heartbeat.tick().await;
        iteration += 1;
        lease.heartbeat().await?;
        lease
            .checkpoint(
                "lease_drill",
                &json!({
                    "iteration": iteration,
                    "owner": owner,
                    "fencingToken": lease.fencing_token(),
                    "observedAtEpochMs": chrono::Utc::now().timestamp_millis()
                }),
            )
            .await?;
    }
    if complete {
        lease
            .complete(&json!({
                "kind": "lease_drill",
                "iterations": iteration,
                "ok": true
            }))
            .await
    } else {
        let snapshot = lease.job().clone();
        lease.release().await?;
        Ok(snapshot)
    }
}

fn validate_uuid(name: &str, value: &str) -> JobControlResult<()> {
    require_nonempty(name, value)?;
    Uuid::parse_str(value)
        .map(|_| ())
        .map_err(|error| invalid(format!("{name} must be a UUID: {error}")))
}

fn validate_claim_inputs(
    tenant_id: &str,
    job_id: &str,
    owner: &str,
    fencing_token: i64,
    lease_secs: u64,
) -> JobControlResult<()> {
    require_nonempty("tenant_id", tenant_id)?;
    validate_uuid("job_id", job_id)?;
    require_nonempty("owner", owner)?;
    if fencing_token < 0 {
        return Err(invalid("fencing_token must be non-negative"));
    }
    if lease_secs == 0 || lease_secs > 86_400 {
        return Err(invalid("lease_secs must be between 1 and 86400"));
    }
    Ok(())
}

fn validate_terminal_inputs(
    tenant_id: &str,
    job_id: &str,
    owner: &str,
    fencing_token: i64,
    expected_version: i64,
    subject: &str,
) -> JobControlResult<()> {
    validate_claim_inputs(
        tenant_id,
        job_id,
        owner,
        fencing_token,
        DEFAULT_JOB_LEASE_SECS,
    )?;
    if expected_version < 0 {
        return Err(invalid("expected_version must be non-negative"));
    }
    require_nonempty("subject", subject)?;
    Ok(())
}

fn u64_to_i64(name: &str, value: u64) -> JobControlResult<i64> {
    i64::try_from(value).map_err(|_| invalid(format!("{name} is too large")))
}

fn fiducia_job_lock_key(tenant_id: &str, job_id: &str) -> JobControlResult<String> {
    require_nonempty("tenant_id", tenant_id)?;
    validate_uuid("job_id", job_id)?;
    Ok(format!(
        "daedalus-fab/fabrication-job/{}/{}",
        tenant_id.trim(),
        job_id
    ))
}

fn retry_delay_secs(attempt_count: i32) -> u64 {
    let exponent = u32::try_from(attempt_count.clamp(0, 8)).unwrap_or(8);
    5_u64
        .saturating_mul(2_u64.saturating_pow(exponent))
        .min(900)
}

fn publish_retry_delay_secs(publish_attempts: i32) -> u64 {
    let exponent = u32::try_from(publish_attempts.clamp(0, 8)).unwrap_or(8);
    2_u64.saturating_pow(exponent).clamp(1, 300)
}

fn queue_message_id(job_id: &str, checkpoint_version: i64, reason: &str) -> String {
    format!("fabrication:{job_id}:checkpoint:{checkpoint_version}:{reason}")
}

fn result_message_id(job_id: &str, checkpoint_version: i64, state: &str) -> String {
    format!("fabrication:{job_id}:terminal:{checkpoint_version}:{state}")
}

fn queue_payload(job: &JobSnapshot, reason: &str) -> JsonValue {
    json!({
        "schemaVersion": 1,
        "eventType": "fabrication.job.wakeup.v1",
        "jobId": job.job_id,
        "tenantId": job.tenant_id,
        "requestId": job.request_id,
        "kind": job.kind,
        "state": job.state,
        "currentStage": job.current_stage,
        "checkpointVersion": job.checkpoint_version,
        "checkpoint": job.checkpoint,
        "attemptCount": job.attempt_count,
        "maxAttempts": job.max_attempts,
        "reason": reason
    })
}

fn terminal_payload(job: &JobSnapshot) -> JsonValue {
    json!({
        "schemaVersion": 1,
        "eventType": "fabrication.job.terminal.v1",
        "jobId": job.job_id,
        "tenantId": job.tenant_id,
        "requestId": job.request_id,
        "kind": job.kind,
        "state": job.state,
        "currentStage": job.current_stage,
        "checkpointVersion": job.checkpoint_version,
        "attemptCount": job.attempt_count,
        "result": job.result_payload,
        "error": {
            "code": job.last_error_code,
            "message": job.last_error_message
        }
    })
}

async fn advisory_xact_lock<C: ConnectionTrait>(
    connection: &C,
    tenant_id: &str,
    job_id: &str,
) -> JobControlResult<()> {
    if try_advisory_xact_lock(connection, tenant_id, job_id).await? {
        return Ok(());
    }
    Err(failed_precondition(
        "PostgreSQL transaction advisory lock is busy",
    ))
}

async fn try_advisory_xact_lock<C: ConnectionTrait>(
    connection: &C,
    tenant_id: &str,
    job_id: &str,
) -> JobControlResult<bool> {
    let row = connection
        .query_one(postgres(
            ADVISORY_XACT_LOCK_SQL,
            vec![tenant_id.to_owned().into(), job_id.to_owned().into()],
        ))
        .await?
        .ok_or_else(|| failed_precondition("advisory lock query returned no row"))?;
    Ok(row.try_get("", "acquired")?)
}

fn decode_job(row: &QueryResult) -> JobControlResult<JobSnapshot> {
    Ok(JobSnapshot {
        job_id: row.try_get("", "job_id")?,
        tenant_id: row.try_get("", "tenant_id")?,
        request_id: row.try_get("", "request_id")?,
        idempotency_key: row.try_get("", "idempotency_key")?,
        kind: row.try_get("", "kind")?,
        state: row.try_get("", "state")?,
        current_stage: row.try_get("", "current_stage")?,
        checkpoint_version: row.try_get("", "checkpoint_version")?,
        checkpoint: row.try_get("", "checkpoint")?,
        request_payload: row.try_get("", "request_payload")?,
        result_payload: row.try_get("", "result_payload")?,
        attempt_count: row.try_get("", "attempt_count")?,
        max_attempts: row.try_get("", "max_attempts")?,
        priority: row.try_get("", "priority")?,
        lease_owner: row.try_get("", "lease_owner")?,
        lease_expires_at: row.try_get("", "lease_expires_at")?,
        fiducia_fencing_token: row.try_get("", "fiducia_fencing_token")?,
        next_attempt_at: row.try_get("", "next_attempt_at")?,
        last_error_code: row.try_get("", "last_error_code")?,
        last_error_message: row.try_get("", "last_error_message")?,
        created_at: row.try_get("", "created_at")?,
        updated_at: row.try_get("", "updated_at")?,
    })
}

fn decode_outbox(row: &QueryResult) -> JobControlResult<OutboxEvent> {
    Ok(OutboxEvent {
        event_id: row.try_get("", "event_id")?,
        job_id: row.try_get("", "job_id")?,
        subject: row.try_get("", "subject")?,
        event_type: row.try_get("", "event_type")?,
        message_id: row.try_get("", "message_id")?,
        payload: row.try_get("", "payload")?,
        publish_attempts: row.try_get("", "publish_attempts")?,
    })
}

const JOB_SELECT_SQL: &str = r#"
SELECT
    job_id::text AS job_id,
    tenant_id,
    request_id,
    idempotency_key,
    kind,
    state,
    current_stage,
    checkpoint_version,
    checkpoint,
    request_payload,
    result_payload,
    attempt_count,
    max_attempts,
    priority,
    lease_owner,
    CASE WHEN lease_expires_at IS NULL THEN NULL
         ELSE to_char(lease_expires_at AT TIME ZONE 'utc', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"')
    END AS lease_expires_at,
    fiducia_fencing_token,
    to_char(next_attempt_at AT TIME ZONE 'utc', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS next_attempt_at,
    last_error_code,
    last_error_message,
    to_char(created_at AT TIME ZONE 'utc', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS created_at,
    to_char(updated_at AT TIME ZONE 'utc', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS updated_at
FROM daedalus.fabrication_job_executions
"#;

const ENQUEUE_JOB_SQL: &str = r#"
WITH inserted AS (
    INSERT INTO daedalus.fabrication_job_executions (
        job_id,
        tenant_id,
        request_id,
        idempotency_key,
        kind,
        state,
        current_stage,
        checkpoint_version,
        checkpoint,
        request_payload,
        attempt_count,
        max_attempts,
        priority,
        next_attempt_at
    )
    VALUES (
        $1::uuid,
        $2,
        $3,
        $4,
        $5,
        'queued',
        'accepted',
        0,
        '{}'::jsonb,
        $6::jsonb,
        0,
        $7,
        $8,
        clock_timestamp()
    )
    ON CONFLICT (tenant_id, idempotency_key) DO NOTHING
    RETURNING *
), selected AS (
    SELECT * FROM inserted
    UNION ALL
    SELECT existing.*
    FROM daedalus.fabrication_job_executions AS existing
    WHERE existing.tenant_id = $2
      AND existing.idempotency_key = $4
    LIMIT 1
)
SELECT
    job_id::text AS job_id,
    tenant_id,
    request_id,
    idempotency_key,
    kind,
    state,
    current_stage,
    checkpoint_version,
    checkpoint,
    request_payload,
    result_payload,
    attempt_count,
    max_attempts,
    priority,
    lease_owner,
    CASE WHEN lease_expires_at IS NULL THEN NULL
         ELSE to_char(lease_expires_at AT TIME ZONE 'utc', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"')
    END AS lease_expires_at,
    fiducia_fencing_token,
    to_char(next_attempt_at AT TIME ZONE 'utc', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS next_attempt_at,
    last_error_code,
    last_error_message,
    to_char(created_at AT TIME ZONE 'utc', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS created_at,
    to_char(updated_at AT TIME ZONE 'utc', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS updated_at
FROM selected
"#;

const ADVISORY_XACT_LOCK_SQL: &str = r#"
SELECT pg_try_advisory_xact_lock(hashtext($1), hashtext($2)) AS acquired
"#;

const CLAIM_JOB_SQL: &str = r#"
UPDATE daedalus.fabrication_job_executions
SET state = 'running',
    attempt_count = attempt_count + 1,
    lease_owner = $3,
    lease_expires_at = clock_timestamp() + make_interval(secs => $5::double precision),
    fiducia_fencing_token = $4,
    started_at = COALESCE(started_at, clock_timestamp()),
    last_error_code = NULL,
    last_error_message = NULL,
    updated_at = clock_timestamp()
WHERE tenant_id = $1
  AND job_id = $2::uuid
  AND attempt_count < max_attempts
  AND next_attempt_at <= clock_timestamp()
  AND (
      state IN ('queued', 'retry_wait')
      OR (
          state = 'running'
          AND lease_expires_at IS NOT NULL
          AND lease_expires_at <= clock_timestamp()
      )
  )
  AND (
      fiducia_fencing_token IS NULL
      OR fiducia_fencing_token <= $4
  )
RETURNING
    job_id::text AS job_id,
    tenant_id,
    request_id,
    idempotency_key,
    kind,
    state,
    current_stage,
    checkpoint_version,
    checkpoint,
    request_payload,
    result_payload,
    attempt_count,
    max_attempts,
    priority,
    lease_owner,
    to_char(lease_expires_at AT TIME ZONE 'utc', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS lease_expires_at,
    fiducia_fencing_token,
    to_char(next_attempt_at AT TIME ZONE 'utc', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS next_attempt_at,
    last_error_code,
    last_error_message,
    to_char(created_at AT TIME ZONE 'utc', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS created_at,
    to_char(updated_at AT TIME ZONE 'utc', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS updated_at
"#;

const RENEW_JOB_LEASE_SQL: &str = r#"
UPDATE daedalus.fabrication_job_executions
SET lease_expires_at = clock_timestamp() + make_interval(secs => $5::double precision),
    updated_at = clock_timestamp()
WHERE tenant_id = $1
  AND job_id = $2::uuid
  AND state = 'running'
  AND lease_owner = $3
  AND fiducia_fencing_token = $4
RETURNING
    job_id::text AS job_id,
    tenant_id,
    request_id,
    idempotency_key,
    kind,
    state,
    current_stage,
    checkpoint_version,
    checkpoint,
    request_payload,
    result_payload,
    attempt_count,
    max_attempts,
    priority,
    lease_owner,
    to_char(lease_expires_at AT TIME ZONE 'utc', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS lease_expires_at,
    fiducia_fencing_token,
    to_char(next_attempt_at AT TIME ZONE 'utc', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS next_attempt_at,
    last_error_code,
    last_error_message,
    to_char(created_at AT TIME ZONE 'utc', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS created_at,
    to_char(updated_at AT TIME ZONE 'utc', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS updated_at
"#;

const CHECKPOINT_SQL: &str = r#"
UPDATE daedalus.fabrication_job_executions
SET current_stage = $6,
    checkpoint_version = checkpoint_version + 1,
    checkpoint = $7::jsonb,
    lease_expires_at = clock_timestamp() + make_interval(secs => $8::double precision),
    updated_at = clock_timestamp()
WHERE tenant_id = $1
  AND job_id = $2::uuid
  AND state = 'running'
  AND lease_owner = $3
  AND fiducia_fencing_token = $4
  AND checkpoint_version = $5
RETURNING
    job_id::text AS job_id,
    tenant_id,
    request_id,
    idempotency_key,
    kind,
    state,
    current_stage,
    checkpoint_version,
    checkpoint,
    request_payload,
    result_payload,
    attempt_count,
    max_attempts,
    priority,
    lease_owner,
    to_char(lease_expires_at AT TIME ZONE 'utc', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS lease_expires_at,
    fiducia_fencing_token,
    to_char(next_attempt_at AT TIME ZONE 'utc', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS next_attempt_at,
    last_error_code,
    last_error_message,
    to_char(created_at AT TIME ZONE 'utc', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS created_at,
    to_char(updated_at AT TIME ZONE 'utc', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS updated_at
"#;

const COMPLETE_SQL: &str = r#"
UPDATE daedalus.fabrication_job_executions
SET state = 'succeeded',
    current_stage = 'completed',
    checkpoint_version = checkpoint_version + 1,
    result_payload = $6::jsonb,
    lease_owner = NULL,
    lease_expires_at = NULL,
    completed_at = clock_timestamp(),
    updated_at = clock_timestamp()
WHERE tenant_id = $1
  AND job_id = $2::uuid
  AND state = 'running'
  AND lease_owner = $3
  AND fiducia_fencing_token = $4
  AND checkpoint_version = $5
RETURNING
    job_id::text AS job_id,
    tenant_id,
    request_id,
    idempotency_key,
    kind,
    state,
    current_stage,
    checkpoint_version,
    checkpoint,
    request_payload,
    result_payload,
    attempt_count,
    max_attempts,
    priority,
    lease_owner,
    NULL::text AS lease_expires_at,
    fiducia_fencing_token,
    to_char(next_attempt_at AT TIME ZONE 'utc', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS next_attempt_at,
    last_error_code,
    last_error_message,
    to_char(created_at AT TIME ZONE 'utc', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS created_at,
    to_char(updated_at AT TIME ZONE 'utc', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS updated_at
"#;

const FAIL_SQL: &str = r#"
UPDATE daedalus.fabrication_job_executions
SET state = CASE
        WHEN $8
         AND attempt_count < max_attempts
        THEN 'retry_wait'
        ELSE 'failed'
    END,
    current_stage = CASE
        WHEN $8
         AND attempt_count < max_attempts
        THEN current_stage
        ELSE 'failed'
    END,
    checkpoint_version = checkpoint_version + 1,
    lease_owner = NULL,
    lease_expires_at = NULL,
    next_attempt_at = CASE
        WHEN $8
         AND attempt_count < max_attempts
        THEN clock_timestamp()
             + make_interval(
                 secs => LEAST(900, 5 * power(2, LEAST(attempt_count, 8)))::double precision
               )
        ELSE next_attempt_at
    END,
    last_error_code = left($6, 200),
    last_error_message = left($7, 4000),
    completed_at = CASE
        WHEN $8
         AND attempt_count < max_attempts
        THEN NULL
        ELSE clock_timestamp()
    END,
    updated_at = clock_timestamp()
WHERE tenant_id = $1
  AND job_id = $2::uuid
  AND state = 'running'
  AND lease_owner = $3
  AND fiducia_fencing_token = $4
  AND checkpoint_version = $5
RETURNING
    job_id::text AS job_id,
    tenant_id,
    request_id,
    idempotency_key,
    kind,
    state,
    current_stage,
    checkpoint_version,
    checkpoint,
    request_payload,
    result_payload,
    attempt_count,
    max_attempts,
    priority,
    lease_owner,
    NULL::text AS lease_expires_at,
    fiducia_fencing_token,
    to_char(next_attempt_at AT TIME ZONE 'utc', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS next_attempt_at,
    last_error_code,
    last_error_message,
    to_char(created_at AT TIME ZONE 'utc', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS created_at,
    to_char(updated_at AT TIME ZONE 'utc', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS updated_at
"#;

const INSERT_OUTBOX_SQL: &str = r#"
INSERT INTO daedalus.fabrication_job_outbox (
    event_id,
    job_id,
    subject,
    event_type,
    message_id,
    payload,
    available_at
)
VALUES (
    $1::uuid,
    $2::uuid,
    $3,
    $4,
    $5,
    $6::jsonb,
    clock_timestamp() + make_interval(secs => $7::double precision)
)
ON CONFLICT (message_id) DO NOTHING
"#;

const CLAIM_OUTBOX_SQL: &str = r#"
WITH candidates AS (
    SELECT event_id
    FROM daedalus.fabrication_job_outbox
    WHERE published_at IS NULL
      AND available_at <= clock_timestamp()
      AND (
          claim_expires_at IS NULL
          OR claim_expires_at <= clock_timestamp()
      )
    ORDER BY available_at, created_at, event_id
    FOR UPDATE SKIP LOCKED
    LIMIT $2
), claimed AS (
    UPDATE daedalus.fabrication_job_outbox AS outbox
    SET claim_owner = $1,
        claim_expires_at = clock_timestamp()
            + make_interval(secs => $3::double precision),
        publish_attempts = outbox.publish_attempts + 1,
        updated_at = clock_timestamp()
    FROM candidates
    WHERE outbox.event_id = candidates.event_id
    RETURNING outbox.*
)
SELECT
    event_id::text AS event_id,
    job_id::text AS job_id,
    subject,
    event_type,
    message_id,
    payload,
    publish_attempts
FROM claimed
ORDER BY available_at, created_at, event_id
"#;

const REAP_EXPIRED_SQL: &str = r#"
UPDATE daedalus.fabrication_job_executions
SET state = CASE
        WHEN attempt_count < max_attempts THEN 'retry_wait'
        ELSE 'failed'
    END,
    current_stage = CASE
        WHEN attempt_count < max_attempts THEN current_stage
        ELSE 'failed'
    END,
    checkpoint_version = checkpoint_version + 1,
    lease_owner = NULL,
    lease_expires_at = NULL,
    next_attempt_at = CASE
        WHEN attempt_count < max_attempts
        THEN clock_timestamp()
             + make_interval(
                 secs => LEAST(900, 5 * power(2, LEAST(attempt_count, 8)))::double precision
               )
        ELSE next_attempt_at
    END,
    last_error_code = 'lease_expired',
    last_error_message = 'worker lease expired; recovery resumed from the last committed checkpoint',
    completed_at = CASE
        WHEN attempt_count < max_attempts THEN NULL
        ELSE clock_timestamp()
    END,
    updated_at = clock_timestamp()
WHERE tenant_id = $1
  AND job_id = $2::uuid
  AND state = 'running'
  AND lease_expires_at IS NOT NULL
  AND lease_expires_at <= clock_timestamp()
RETURNING
    job_id::text AS job_id,
    tenant_id,
    request_id,
    idempotency_key,
    kind,
    state,
    current_stage,
    checkpoint_version,
    checkpoint,
    request_payload,
    result_payload,
    attempt_count,
    max_attempts,
    priority,
    lease_owner,
    NULL::text AS lease_expires_at,
    fiducia_fencing_token,
    to_char(next_attempt_at AT TIME ZONE 'utc', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS next_attempt_at,
    last_error_code,
    last_error_message,
    to_char(created_at AT TIME ZONE 'utc', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS created_at,
    to_char(updated_at AT TIME ZONE 'utc', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS updated_at
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn postgres_locks_are_transaction_scoped() {
        assert!(ADVISORY_XACT_LOCK_SQL.contains("pg_try_advisory_xact_lock"));
        assert!(!ADVISORY_XACT_LOCK_SQL.contains("pg_advisory_lock("));
        assert!(!ADVISORY_XACT_LOCK_SQL.contains("pg_try_advisory_lock("));
    }

    #[test]
    fn job_claims_are_fenced_and_do_not_hold_a_transaction_for_work() {
        assert!(CLAIM_JOB_SQL.contains("fiducia_fencing_token <= $4"));
        assert!(CLAIM_JOB_SQL.contains("lease_expires_at <= clock_timestamp()"));
        assert!(CLAIM_JOB_SQL.contains("attempt_count < max_attempts"));
        assert!(!CLAIM_JOB_SQL.contains("pg_sleep"));
    }

    #[test]
    fn checkpoints_use_owner_token_and_compare_and_swap_version() {
        assert!(CHECKPOINT_SQL.contains("lease_owner = $3"));
        assert!(CHECKPOINT_SQL.contains("fiducia_fencing_token = $4"));
        assert!(CHECKPOINT_SQL.contains("checkpoint_version = $5"));
        assert!(CHECKPOINT_SQL.contains("checkpoint_version = checkpoint_version + 1"));
    }

    #[test]
    fn outbox_claims_are_concurrent_and_recoverable() {
        assert!(CLAIM_OUTBOX_SQL.contains("FOR UPDATE SKIP LOCKED"));
        assert!(CLAIM_OUTBOX_SQL.contains("claim_expires_at <= clock_timestamp()"));
        assert!(CLAIM_OUTBOX_SQL.contains("published_at IS NULL"));
        assert!(INSERT_OUTBOX_SQL.contains("ON CONFLICT (message_id) DO NOTHING"));
    }

    #[test]
    fn retry_backoff_is_positive_and_bounded() {
        assert_eq!(retry_delay_secs(0), 5);
        assert!(retry_delay_secs(8) <= 900);
        assert_eq!(retry_delay_secs(100), 900);
        assert_eq!(publish_retry_delay_secs(0), 1);
        assert_eq!(publish_retry_delay_secs(100), 256);
    }

    #[test]
    fn lock_key_is_tenant_and_job_scoped() {
        let job_id = Uuid::new_v4().to_string();
        let key = fiducia_job_lock_key("tenant-a", &job_id).unwrap();
        assert_eq!(
            key,
            format!("daedalus-fab/fabrication-job/tenant-a/{job_id}")
        );
    }

    #[test]
    fn outbox_message_ids_are_deterministic() {
        let job_id = Uuid::new_v4().to_string();
        assert_eq!(
            queue_message_id(&job_id, 7, "retry"),
            format!("fabrication:{job_id}:checkpoint:7:retry")
        );
        assert_eq!(
            result_message_id(&job_id, 8, "failed"),
            format!("fabrication:{job_id}:terminal:8:failed")
        );
    }

    #[test]
    fn request_subject_stays_inside_shared_fabrication_stream() {
        let request = EnqueueRequest {
            tenant_id: "tenant-a".into(),
            request_id: "request-a".into(),
            idempotency_key: "idem-a".into(),
            kind: "mesh_conversion".into(),
            request_payload: json!({}),
            max_attempts: 5,
            priority: 0,
            subject: REQUEST_SUBJECT.into(),
        };
        request.validate().unwrap();

        let mut invalid_request = request;
        invalid_request.subject = "other.subject".into();
        assert!(invalid_request.validate().is_err());
    }

    #[test]
    fn reaper_only_targets_expired_running_jobs() {
        assert!(REAP_EXPIRED_SQL.contains("state = 'running'"));
        assert!(REAP_EXPIRED_SQL.contains("lease_expires_at <= clock_timestamp()"));
        assert!(REAP_EXPIRED_SQL.contains("last committed checkpoint"));
    }
}
