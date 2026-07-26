//! Durable-state seam for the two stores that decide how many replicas this
//! service may run.
//!
//! Everything the service remembers *between* requests lives behind one of the
//! two traits in this module:
//!
//! * [`JobStore`] — fabrication jobs and their artifacts, which back `/jobs`,
//!   `/jobs/{id}`, `/jobs/{id}/artifacts/{artifact_id}` and
//!   `/jobs/{id}/release-bundle`.
//! * [`LearningStore`] — run outcomes, whose aggregate
//!   ([`LearningPolicySnapshot`]) is fed into `plan_fabrication_with_policy`
//!   and therefore changes what a plan request answers.
//!
//! Both had exactly one implementation before this module existed: an
//! in-process container behind an `RwLock`. That is why `replicas` was pinned to
//! 1. JetStream splits deliveries across replicas, so two pods accumulate
//! *disjoint halves* of both streams: `/jobs/{id}` 404s on the pod that did not
//! produce the job, and the learning aggregates diverge permanently so the same
//! plan request returns different plans depending on which pod answers. Neither
//! is a mutual-exclusion problem — a distributed lock cannot make one pod's
//! memory visible to another — so the fix is shared storage, which is what
//! [`PostgresJobStore`] and [`PostgresLearningStore`] are.
//!
//! # Why one module and not two
//!
//! `src/coordination.rs` is the house pattern this follows: a trait, a local
//! default, a real distributed implementation, and one `build_*` selector that
//! logs which one is live. These two stores are one seam by that measure —
//! "service state that must be shared before a second replica is correct" —
//! and they share their error type ([`StoreError`]), their millisecond-to-
//! `timestamptz` conversion, their retention sweep, and their single
//! [`Persistence`]-driven selection point. Splitting them across two files
//! would either duplicate all four or need a third file to hold them.
//!
//! # Retention is a policy, not a cap
//!
//! In memory, `MAX_STORED_JOBS` (128) and `MAX_LEARNING_OUTCOMES` (512) are
//! **hard caps**: the 129th job evicts the 1st inside the same `insert`, so the
//! store is never larger than the cap for even an instant.
//!
//! In Postgres they are **retention targets**, and that difference is real and
//! intentional:
//!
//! * The row limit is enforced by a bounded `DELETE` that runs every
//!   [`RETENTION_SWEEP_EVERY`] writes, not on every write, so the table
//!   transiently holds more than the target.
//! * Several writers share the table. Two pods inserting concurrently can push
//!   the count past the target between sweeps.
//! * The sweep deletes at most [`RETENTION_DELETE_BATCH`] rows per pass so that
//!   a table which somehow grew large is trimmed over several writes rather
//!   than in one unbounded statement holding locks.
//!
//! So: reads are always bounded (every query carries a `LIMIT`), and the table
//! converges to the target, but "there are never more than 128 rows in
//! `fab_jobs`" is not a property this code provides and must not be relied on.
//!
//! The sweep runs **on write**, deliberately, and therefore needs no lease. It
//! is an idempotent, bounded, keep-newest-N `DELETE`: two replicas running it at
//! the same instant produce the same end state as one, and the second deletes
//! nothing. A *scheduled* sweep would be a different matter — a timer firing on
//! every replica is precisely the double-fire that `src/coordination.rs` exists
//! to prevent — which is the other reason this one is not on a timer.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, RwLock,
};

use async_trait::async_trait;
use dd_pg_defs_sea_orm::{fab_jobs, fab_learning_outcomes};
use sea_orm::{
    sea_query::OnConflict, ActiveValue::Set, ConnectionTrait, DatabaseConnection, DbBackend,
    EntityTrait, QueryOrder, QuerySelect, Statement,
};
use serde_json::Value;

use crate::{
    job_release_bundle_response, persistence::Persistence, FabricationArtifact,
    FabricationJobDetail, FabricationJobStore, LearningMemory, LearningOutcomeRecord,
    LearningPolicySnapshot, StoredFabricationJob, MAX_LEARNING_OUTCOMES, MAX_STORED_JOBS,
    SERVICE_NAME,
};

/// Writes between retention sweeps. A sweep is one bounded `DELETE`; running it
/// on every insert would double the write cost of a store that is written far
/// more often than it overflows.
const RETENTION_SWEEP_EVERY: u64 = 32;
/// Upper bound on rows removed by a single sweep, so an oversized table is
/// trimmed across several writes instead of in one long-running statement.
const RETENTION_DELETE_BATCH: u64 = 512;

/// A store operation that did not complete.
///
/// Deliberately coarse and deliberately **not** silently swallowed: a read that
/// fails must surface as a 5xx, not as an empty job list, because an empty list
/// is indistinguishable from "this pod has never seen that job" — which is the
/// exact failure this whole module exists to remove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StoreError {
    /// The database refused, timed out, or was unreachable.
    Backend(String),
    /// A stored payload could not be decoded back into a domain record. Means
    /// the row was written by an incompatible build, not that it is missing.
    Payload(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Backend(detail) => write!(formatter, "store backend failed: {detail}"),
            Self::Payload(detail) => write!(formatter, "stored payload was unreadable: {detail}"),
        }
    }
}

impl std::error::Error for StoreError {}

fn backend<E: std::fmt::Display>(error: E) -> StoreError {
    StoreError::Backend(error.to_string())
}

/// What an insert did to the store.
///
/// `displaced` is true when a row of the same id already existed — an upsert
/// that *updated* rather than *inserted*. Job ids are deterministic in
/// `(kind, request_id)` alone, so this is the expected shape of a NATS
/// redelivery replaying the same request — at any later time, not only within
/// the same millisecond, which is the point of having dropped the timestamp
/// from the id. Two genuinely different requests cannot land here by accident:
/// a caller that supplies no request id is given a distinct `{prefix}-{uuid}`
/// one. It is still data loss if two genuinely different jobs collided (two
/// callers reusing one request id). The store cannot tell those apart, so it
/// reports the event and the caller counts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct InsertOutcome {
    pub(crate) displaced: bool,
    /// Artifacts carried by the row that was replaced, for the warning log.
    pub(crate) displaced_artifacts: usize,
}

fn to_timestamp(unix_ms: u128) -> sea_orm::prelude::DateTimeWithTimeZone {
    let clamped = i64::try_from(unix_ms).unwrap_or(i64::MAX);
    chrono::DateTime::from_timestamp_millis(clamped)
        .unwrap_or_else(chrono::Utc::now)
        .fixed_offset()
}

/// Longest value the schema accepts in the short text columns
/// (`fab_jobs.job_id`, `fab_jobs.request_id`, `fab_learning_outcomes.outcome_id`),
/// in **octets** — the schema's `octet_length(...) between 1 and 200` checks.
const MAX_SHORT_TEXT_BYTES: usize = 200;
/// `fab_jobs.summary`'s ceiling, also in octets.
const MAX_SUMMARY_BYTES: usize = 20_000;

/// Clamp a string to `limit` octets on a character boundary.
///
/// The domain's own limits are counted in **characters** (`safe_job_id`
/// truncates to 180 chars, `summary_text` to 240) while the schema's `check`
/// constraints count **octets**, so a multi-byte request id can satisfy the
/// former and violate the latter. A violated check aborts the insert, which
/// would lose the job entirely; truncating a diagnostic string loses far less.
fn clamp_bytes(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_string();
    }
    let mut end = limit;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

/// Normalise an outcome so it can be stored *and* read back.
///
/// Only one field needs it, and it needs it for two independent reasons:
///
/// * `fab_learning_outcomes` rejects NaN and ±Infinity by check constraint,
///   because `reward` feeds a mean that steers planning and one NaN poisons
///   every aggregate it lands in.
/// * `serde_json` has no representation for a non-finite float, so it encodes
///   one as `null` — which then fails to decode back into `f64` and turns the
///   whole `payload` into an unreadable row. That failure mode is invisible on
///   the write path and only surfaces on the next read.
///
/// Coercing to a neutral 0.0 keeps the outcome: its success flag, methods and
/// observations still carry signal, where dropping it would discard the whole
/// observation. Applied by both backends so the in-memory aggregate cannot
/// disagree with the stored one.
fn sanitize_outcome(mut outcome: LearningOutcomeRecord) -> LearningOutcomeRecord {
    if !outcome.reward.is_finite() {
        tracing::warn!(
            outcome.id = %outcome.outcome_id,
            "learning outcome carried a non-finite reward; storing 0.0 so the row is \
             accepted, the payload stays decodable, and the mean it feeds stays defined"
        );
        outcome.reward = 0.0;
    }
    outcome
}

/// Replace every non-finite JSON number (NaN, +Inf, -Inf) anywhere in `value`
/// with `0.0`, in place, recursing through objects and arrays.
///
/// # Why this exists
///
/// A [`StoredFabricationJob`] / [`LearningOutcomeRecord`] serialises to a
/// `payload` JSONB column, and `serde_json` has **no representation for a
/// non-finite `f64`**: it encodes `NaN`/`±Infinity` as the JSON literal `null`.
/// A `null` then fails to decode back into the struct's `f64` field, so the row
/// becomes permanently unreadable — a failure that is *invisible on the write
/// path* and only surfaces on the next read (exactly the trap
/// [`sanitize_outcome`] guards `reward` against, generalised to every one of the
/// ~113 `f64`/`Option<f64>`/`Vec<f64>`/map-of-`f64` fields reachable from the
/// serialized payload).
///
/// # Why `0.0` and not "drop the key"
///
/// Dropping the offending key would leave the payload *missing a field that the
/// strongly-typed struct requires*, so `serde_json::from_value` would fail to
/// decode — reintroducing the very unreadable-row failure this prevents. A
/// neutral `0.0` keeps the payload structurally intact and decodable, matching
/// the sentinel [`sanitize_outcome`] already uses for `reward`.
///
/// # Why here and not at input validation
///
/// The geometry engine rejects non-finite floats at request-validation time, so
/// a non-finite value reaching this point is a **computed** one (an overflow or
/// `0.0/0.0` deep in planning), not a supplied one. This is defence for that
/// computed case, applied once at the single persistence chokepoint rather than
/// on all ~113 fields individually.
#[cfg(test)]
fn sanitize_finite(value: &mut Value) {
    match value {
        Value::Number(number) => {
            if number.as_f64().is_some_and(|float| !float.is_finite()) {
                *value = Value::from(0.0_f64);
            }
        }
        Value::Array(items) => {
            for item in items {
                sanitize_finite(item);
            }
        }
        Value::Object(map) => {
            for (_, entry) in map.iter_mut() {
                sanitize_finite(entry);
            }
        }
        // Bools, strings and null are always finite / representable.
        _ => {}
    }
}

/// Keep the newest `retain` rows of `table`, deleting at most
/// [`RETENTION_DELETE_BATCH`] of the rest.
///
/// Written as one statement per call with a hard `LIMIT` so the work is bounded
/// no matter how large the table got. `table` is a compile-time constant in
/// this module and never caller-supplied.
async fn sweep_retention(
    db: &DatabaseConnection,
    table: &'static str,
    key: &'static str,
    retain: u64,
) -> Result<u64, StoreError> {
    let sql = format!(
        r#"DELETE FROM "daedalus"."{table}" WHERE "{key}" IN (
             SELECT "{key}" FROM "daedalus"."{table}"
             ORDER BY "created_at" DESC, "{key}" DESC
             OFFSET $1 LIMIT $2
           )"#
    );
    let result = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            &sql,
            [
                i64::try_from(retain).unwrap_or(i64::MAX).into(),
                i64::try_from(RETENTION_DELETE_BATCH)
                    .unwrap_or(i64::MAX)
                    .into(),
            ],
        ))
        .await
        .map_err(backend)?;
    Ok(result.rows_affected())
}

/// Decide whether this write should also sweep. Sweeping every
/// [`RETENTION_SWEEP_EVERY`] writes keeps the amortized cost of retention at
/// roughly one extra statement per 32 inserts.
fn should_sweep(writes: &AtomicU64) -> bool {
    writes.fetch_add(1, Ordering::Relaxed) % RETENTION_SWEEP_EVERY == 0
}

// ---------------------------------------------------------------------------
// Jobs
// ---------------------------------------------------------------------------

/// Fabrication jobs and their artifacts.
///
/// Only `insert`, `get` and `recent` are backend-specific. Every other read is
/// a projection of those, expressed once here as a default method so the
/// in-memory and Postgres stores cannot drift in what `/jobs/{id}` means.
#[async_trait]
pub(crate) trait JobStore: Send + Sync {
    async fn insert(&self, job: StoredFabricationJob) -> Result<InsertOutcome, StoreError>;
    async fn get(&self, job_id: &str) -> Result<Option<StoredFabricationJob>, StoreError>;
    /// The newest `limit` jobs, newest first.
    async fn recent(&self, limit: usize) -> Result<Vec<StoredFabricationJob>, StoreError>;
    /// Short name for boot logs and diagnostics.
    fn mode(&self) -> &'static str;
    /// Whether every replica sees the same rows.
    fn is_shared(&self) -> bool;

    async fn detail(&self, job_id: &str) -> Result<Option<FabricationJobDetail>, StoreError> {
        Ok(self.get(job_id).await?.as_ref().map(job_detail))
    }

    async fn artifact(
        &self,
        job_id: &str,
        artifact_id: &str,
    ) -> Result<Option<FabricationArtifact>, StoreError> {
        Ok(self
            .get(job_id)
            .await?
            .and_then(|job| job.artifacts.get(artifact_id).cloned()))
    }

    async fn release_bundle(&self, job_id: &str) -> Result<Option<Value>, StoreError> {
        Ok(self
            .get(job_id)
            .await?
            .as_ref()
            .map(job_release_bundle_response))
    }

    /// `(job count, artifact count)` over the retained window.
    async fn counts(&self) -> Result<(usize, usize), StoreError> {
        let jobs = self.recent(MAX_STORED_JOBS).await?;
        let artifacts = jobs.iter().map(|job| job.artifacts.len()).sum();
        Ok((jobs.len(), artifacts))
    }
}

fn job_detail(job: &StoredFabricationJob) -> FabricationJobDetail {
    let release_bundle = job_release_bundle_response(job);
    FabricationJobDetail {
        record: job.record.clone(),
        plan: job.plan.clone(),
        analysis: job.analysis.clone(),
        learning: job.learning.clone(),
        release_gate_summary: release_bundle
            .get("releaseGateSummary")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({})),
        release_bundle_route: format!("/fabrication/jobs/{}/release-bundle", job.record.job_id),
        artifacts: job
            .artifacts
            .values()
            .map(FabricationArtifact::summary)
            .collect(),
    }
}

pub(crate) fn release_gate_summary(job: &StoredFabricationJob) -> Value {
    let release_bundle = job_release_bundle_response(job);
    serde_json::json!({
        "jobId": job.record.job_id,
        "requestId": job.record.request_id,
        "kind": job.record.kind,
        "severity": job.record.severity,
        "ok": job.record.ok,
        "releaseGateSummary": release_bundle
            .get("releaseGateSummary")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({})),
        "releaseBundleRoute": format!("/fabrication/jobs/{}/release-bundle", job.record.job_id)
    })
}

/// In-process jobs, with the pre-existing FIFO cap.
///
/// This is the store used when persistence is disabled, and it is what every
/// test and every database-less local run exercises. It wraps the same
/// [`FabricationJobStore`] container the service always used, so the eviction
/// order, the displacement reporting and the `MAX_STORED_JOBS` hard cap are
/// unchanged by construction rather than by reimplementation.
///
/// **It does not see other processes.** Running more than one replica on this
/// implementation gives you a `/jobs/{id}` that 404s roughly half the time.
pub(crate) struct InMemoryJobStore {
    inner: RwLock<FabricationJobStore>,
}

impl InMemoryJobStore {
    pub(crate) fn new(max_jobs: usize) -> Self {
        Self {
            inner: RwLock::new(FabricationJobStore::new(max_jobs)),
        }
    }
}

impl Default for InMemoryJobStore {
    fn default() -> Self {
        Self::new(MAX_STORED_JOBS)
    }
}

/// Take a lock, recovering from poisoning instead of refusing forever.
///
/// The previous call sites matched on `RwLock::write()` and, on `Err`, logged
/// and returned. Poisoning is sticky: one panic anywhere inside a critical
/// section meant *every subsequent write for the life of the process* failed,
/// while `/readyz` stayed green because it never touches these locks. The data
/// behind the lock is a `VecDeque` plus a `BTreeMap`; a panic mid-insert can
/// leave it stale but not unsound, and a store that keeps accepting writes is
/// strictly better than a pod that silently stops recording jobs.
fn recover<T>(result: Result<T, std::sync::PoisonError<T>>) -> T {
    result.unwrap_or_else(|poisoned| {
        tracing::error!(
            "{SERVICE_NAME} in-memory store lock was poisoned by an earlier panic; \
             continuing with the recovered contents rather than refusing every \
             subsequent write for the life of the process"
        );
        poisoned.into_inner()
    })
}

#[async_trait]
impl JobStore for InMemoryJobStore {
    async fn insert(&self, job: StoredFabricationJob) -> Result<InsertOutcome, StoreError> {
        let displaced = recover(self.inner.write()).insert(job);
        Ok(match displaced {
            Some(previous) => InsertOutcome {
                displaced: true,
                displaced_artifacts: previous.artifacts.len(),
            },
            None => InsertOutcome::default(),
        })
    }

    async fn get(&self, job_id: &str) -> Result<Option<StoredFabricationJob>, StoreError> {
        Ok(recover(self.inner.read()).get(job_id))
    }

    async fn recent(&self, limit: usize) -> Result<Vec<StoredFabricationJob>, StoreError> {
        Ok(recover(self.inner.read()).recent(limit))
    }

    fn mode(&self) -> &'static str {
        "memory"
    }

    fn is_shared(&self) -> bool {
        false
    }
}

/// Jobs in `daedalus.fab_jobs`, readable by every replica.
///
/// `payload` holds the whole serialized [`StoredFabricationJob`]; the other
/// columns are the query, filter and ordering fields. The schema is generated
/// and owned by pg-defs — this code never issues DDL.
pub(crate) struct PostgresJobStore {
    db: Arc<DatabaseConnection>,
    retain: u64,
    writes: AtomicU64,
}

impl PostgresJobStore {
    pub(crate) fn new(db: Arc<DatabaseConnection>, retain: usize) -> Self {
        Self {
            db,
            retain: retain.max(1) as u64,
            writes: AtomicU64::new(0),
        }
    }

    fn decode(model: fab_jobs::Model) -> Result<StoredFabricationJob, StoreError> {
        serde_json::from_value(model.payload).map_err(|error| {
            StoreError::Payload(format!(
                "fab_jobs row {} could not be decoded: {error}",
                model.job_id
            ))
        })
    }
}

#[async_trait]
impl JobStore for PostgresJobStore {
    async fn insert(&self, job: StoredFabricationJob) -> Result<InsertOutcome, StoreError> {
        let payload = serde_json::to_value(&job)
            .map_err(|error| StoreError::Payload(format!("job could not be encoded: {error}")))?;
        let record = &job.record;

        // Read the row we are about to replace, purely so the displacement
        // warning can name how many artifacts it carried. This is NOT wrapped
        // in a transaction with the upsert below, on purpose: the upsert is a
        // single atomic statement, so the *data* never races. Only the
        // diagnostic count can be stale, and a row lock held across a
        // multi-megabyte payload write is too high a price for exactness in a
        // log line.
        let previous =
            fab_jobs::Entity::find_by_id(clamp_bytes(&record.job_id, MAX_SHORT_TEXT_BYTES))
                .one(&*self.db)
                .await
                .map_err(backend)?;

        let model = fab_jobs::ActiveModel {
            job_id: Set(clamp_bytes(&record.job_id, MAX_SHORT_TEXT_BYTES)),
            request_id: Set(clamp_bytes(&record.request_id, MAX_SHORT_TEXT_BYTES)),
            kind: Set(record.kind.clone()),
            status: Set(record.status.clone()),
            ok: Set(record.ok),
            severity: Set(record.severity.clone()),
            summary: Set(clamp_bytes(&record.summary, MAX_SUMMARY_BYTES)),
            artifact_count: Set(i32::try_from(record.artifact_count).unwrap_or(i32::MAX)),
            payload: Set(payload),
            created_at: Set(to_timestamp(record.created_at_ms)),
            updated_at: Set(to_timestamp(record.updated_at_ms)),
        };

        // Upsert, not insert. A redelivered NATS message regenerates the same
        // deterministic job id, and an insert would fail the whole delivery on
        // a duplicate key for what is the *same* logical job.
        fab_jobs::Entity::insert(model)
            .on_conflict(
                OnConflict::column(fab_jobs::Column::JobId)
                    .update_columns([
                        fab_jobs::Column::RequestId,
                        fab_jobs::Column::Kind,
                        fab_jobs::Column::Status,
                        fab_jobs::Column::Ok,
                        fab_jobs::Column::Severity,
                        fab_jobs::Column::Summary,
                        fab_jobs::Column::ArtifactCount,
                        fab_jobs::Column::Payload,
                        fab_jobs::Column::CreatedAt,
                        fab_jobs::Column::UpdatedAt,
                    ])
                    .to_owned(),
            )
            .exec(&*self.db)
            .await
            .map_err(backend)?;

        if should_sweep(&self.writes) {
            match sweep_retention(&*self.db, "fab_jobs", "job_id", self.retain).await {
                Ok(removed) if removed > 0 => tracing::debug!(
                    store.table = "fab_jobs",
                    store.retained = self.retain,
                    store.removed = removed,
                    "job retention sweep trimmed the oldest rows"
                ),
                Ok(_) => {}
                // Retention failing is not a reason to fail the write that
                // already succeeded; the next sweep tries again.
                Err(error) => tracing::warn!(
                    store.table = "fab_jobs",
                    "job retention sweep failed: {error}"
                ),
            }
        }

        Ok(match previous {
            Some(previous) => InsertOutcome {
                displaced: true,
                displaced_artifacts: usize::try_from(previous.artifact_count).unwrap_or(0),
            },
            None => InsertOutcome::default(),
        })
    }

    async fn get(&self, job_id: &str) -> Result<Option<StoredFabricationJob>, StoreError> {
        fab_jobs::Entity::find_by_id(clamp_bytes(job_id, MAX_SHORT_TEXT_BYTES))
            .one(&*self.db)
            .await
            .map_err(backend)?
            .map(Self::decode)
            .transpose()
    }

    async fn recent(&self, limit: usize) -> Result<Vec<StoredFabricationJob>, StoreError> {
        // Every read is bounded: the LIMIT is not an optimization, it is what
        // keeps a table that has grown past its retention target from being
        // loaded into one response.
        let limit = limit.clamp(1, MAX_STORED_JOBS) as u64;
        fab_jobs::Entity::find()
            .order_by_desc(fab_jobs::Column::CreatedAt)
            .order_by_desc(fab_jobs::Column::JobId)
            .limit(limit)
            .all(&*self.db)
            .await
            .map_err(backend)?
            .into_iter()
            .map(Self::decode)
            .collect()
    }

    fn mode(&self) -> &'static str {
        "postgres"
    }

    fn is_shared(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// Learning outcomes
// ---------------------------------------------------------------------------

/// Run outcomes, and the policy aggregate computed from them.
///
/// [`JobStore`]'s note about default methods applies here too: `snapshot` is a
/// pure function of the retained window, defined once, so an aggregate can
/// never mean two different things depending on which backend is live.
#[async_trait]
pub(crate) trait LearningStore: Send + Sync {
    /// Returns whether an outcome of the same id already existed.
    async fn insert(&self, outcome: LearningOutcomeRecord) -> Result<InsertOutcome, StoreError>;
    /// The newest `limit` outcomes, **oldest first** — the order
    /// [`LearningMemory`] iterates in, so aggregates and the "most recent 32"
    /// examples match the in-memory store exactly.
    async fn recent(&self, limit: usize) -> Result<Vec<LearningOutcomeRecord>, StoreError>;
    fn mode(&self) -> &'static str;
    fn is_shared(&self) -> bool;

    async fn window(&self) -> Result<LearningMemory, StoreError> {
        Ok(LearningMemory::from_outcomes(
            self.recent(MAX_LEARNING_OUTCOMES).await?,
            MAX_LEARNING_OUTCOMES,
        ))
    }

    async fn snapshot(&self) -> Result<LearningPolicySnapshot, StoreError> {
        Ok(self.window().await?.snapshot())
    }
}

/// In-process outcomes, with the pre-existing FIFO cap.
///
/// One behavioural change over the raw container: an outcome whose id is
/// already present **replaces it in place** rather than being appended a second
/// time. See [`PostgresLearningStore::insert`] for why that is a fix and not a
/// regression.
pub(crate) struct InMemoryLearningStore {
    inner: RwLock<LearningMemory>,
}

impl InMemoryLearningStore {
    pub(crate) fn new(max_outcomes: usize) -> Self {
        Self {
            inner: RwLock::new(LearningMemory::new(max_outcomes)),
        }
    }
}

impl Default for InMemoryLearningStore {
    fn default() -> Self {
        Self::new(MAX_LEARNING_OUTCOMES)
    }
}

#[async_trait]
impl LearningStore for InMemoryLearningStore {
    async fn insert(&self, outcome: LearningOutcomeRecord) -> Result<InsertOutcome, StoreError> {
        let replaced = recover(self.inner.write()).upsert(sanitize_outcome(outcome));
        Ok(InsertOutcome {
            displaced: replaced,
            displaced_artifacts: 0,
        })
    }

    async fn recent(&self, limit: usize) -> Result<Vec<LearningOutcomeRecord>, StoreError> {
        Ok(recover(self.inner.read()).recent(limit))
    }

    fn mode(&self) -> &'static str {
        "memory"
    }

    fn is_shared(&self) -> bool {
        false
    }
}

/// Outcomes in `daedalus.fab_learning_outcomes`, aggregated identically by
/// every replica because every replica reads the same rows.
pub(crate) struct PostgresLearningStore {
    db: Arc<DatabaseConnection>,
    retain: u64,
    writes: AtomicU64,
}

impl PostgresLearningStore {
    pub(crate) fn new(db: Arc<DatabaseConnection>, retain: usize) -> Self {
        Self {
            db,
            retain: retain.max(1) as u64,
            writes: AtomicU64::new(0),
        }
    }

    fn decode(model: fab_learning_outcomes::Model) -> Result<LearningOutcomeRecord, StoreError> {
        serde_json::from_value(model.payload).map_err(|error| {
            StoreError::Payload(format!(
                "fab_learning_outcomes row {} could not be decoded: {error}",
                model.outcome_id
            ))
        })
    }
}

#[async_trait]
impl LearningStore for PostgresLearningStore {
    /// Upsert on `outcome_id`.
    ///
    /// The in-memory store appended unconditionally, so a JetStream redelivery
    /// (`max_deliver: 5`) could contribute the *same* outcome to the aggregate
    /// up to five times — inflating sample counts, success counts and reward
    /// sums, which then bias `plan_fabrication_with_policy`. One row per
    /// `outcome_id` makes a redelivery arithmetically free.
    ///
    /// `created_at` is deliberately left alone on conflict: the outcome's
    /// position in the retention window is when it was first observed, not when
    /// it was last redelivered.
    async fn insert(&self, outcome: LearningOutcomeRecord) -> Result<InsertOutcome, StoreError> {
        let outcome = sanitize_outcome(outcome);
        let payload = serde_json::to_value(&outcome).map_err(|error| {
            StoreError::Payload(format!("learning outcome could not be encoded: {error}"))
        })?;
        let existing = fab_learning_outcomes::Entity::find_by_id(clamp_bytes(
            &outcome.outcome_id,
            MAX_SHORT_TEXT_BYTES,
        ))
        .one(&*self.db)
        .await
        .map_err(backend)?;

        let model = fab_learning_outcomes::ActiveModel {
            outcome_id: Set(clamp_bytes(&outcome.outcome_id, MAX_SHORT_TEXT_BYTES)),
            request_id: Set(clamp_bytes(&outcome.request_id, MAX_SHORT_TEXT_BYTES)),
            job_id: Set(outcome.job_id.clone()),
            objective: Set(outcome.objective.clone()),
            machine_kind: Set(outcome.machine_kind.clone()),
            assembly_strategy: Set(outcome.assembly_strategy.clone()),
            success: Set(outcome.success),
            reward: Set(outcome.reward),
            payload: Set(payload),
            created_at: Set(to_timestamp(outcome.created_at_ms)),
        };
        fab_learning_outcomes::Entity::insert(model)
            .on_conflict(
                OnConflict::column(fab_learning_outcomes::Column::OutcomeId)
                    .update_columns([
                        fab_learning_outcomes::Column::RequestId,
                        fab_learning_outcomes::Column::JobId,
                        fab_learning_outcomes::Column::Objective,
                        fab_learning_outcomes::Column::MachineKind,
                        fab_learning_outcomes::Column::AssemblyStrategy,
                        fab_learning_outcomes::Column::Success,
                        fab_learning_outcomes::Column::Reward,
                        fab_learning_outcomes::Column::Payload,
                    ])
                    .to_owned(),
            )
            .exec(&*self.db)
            .await
            .map_err(backend)?;

        if should_sweep(&self.writes) {
            match sweep_retention(
                &*self.db,
                "fab_learning_outcomes",
                "outcome_id",
                self.retain,
            )
            .await
            {
                Ok(removed) if removed > 0 => tracing::debug!(
                    store.table = "fab_learning_outcomes",
                    store.retained = self.retain,
                    store.removed = removed,
                    "learning retention sweep trimmed the oldest rows"
                ),
                Ok(_) => {}
                Err(error) => tracing::warn!(
                    store.table = "fab_learning_outcomes",
                    "learning retention sweep failed: {error}"
                ),
            }
        }

        Ok(InsertOutcome {
            displaced: existing.is_some(),
            displaced_artifacts: 0,
        })
    }

    async fn recent(&self, limit: usize) -> Result<Vec<LearningOutcomeRecord>, StoreError> {
        let limit = limit.clamp(1, MAX_LEARNING_OUTCOMES) as u64;
        let mut outcomes = fab_learning_outcomes::Entity::find()
            .order_by_desc(fab_learning_outcomes::Column::CreatedAt)
            .order_by_desc(fab_learning_outcomes::Column::OutcomeId)
            .limit(limit)
            .all(&*self.db)
            .await
            .map_err(backend)?
            .into_iter()
            .map(Self::decode)
            .collect::<Result<Vec<_>, _>>()?;
        // Newest-first off the index, oldest-first for the caller.
        outcomes.reverse();
        Ok(outcomes)
    }

    fn mode(&self) -> &'static str {
        "postgres"
    }

    fn is_shared(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// Selection
// ---------------------------------------------------------------------------

/// Choose the job and learning stores from [`Persistence`] and say so once, at
/// boot, in the pod log.
///
/// Mirrors `build_coordination`: the log line names the mode, whether the store
/// is shared across replicas, and — when it is not — what that costs, so a
/// misconfigured deployment is visible in `kubectl logs` rather than only in a
/// 404 six hours later.
pub(crate) fn build_stores(
    persistence: &Persistence,
) -> (Arc<dyn JobStore>, Arc<dyn LearningStore>) {
    match persistence {
        Persistence::SeaOrm(connection) => {
            let jobs = PostgresJobStore::new(Arc::clone(connection), MAX_STORED_JOBS);
            let learning =
                PostgresLearningStore::new(Arc::clone(connection), MAX_LEARNING_OUTCOMES);
            tracing::info!(
                store.jobs.mode = jobs.mode(),
                store.learning.mode = learning.mode(),
                store.shared = jobs.is_shared() && learning.is_shared(),
                store.jobs.table = "daedalus.fab_jobs",
                store.learning.table = "daedalus.fab_learning_outcomes",
                store.jobs.retained = MAX_STORED_JOBS,
                store.learning.retained = MAX_LEARNING_OUTCOMES,
                "{SERVICE_NAME} job and learning state is in Postgres and shared by every \
                 replica. The row limits are retention targets swept on write, not hard caps"
            );
            (Arc::new(jobs), Arc::new(learning))
        }
        Persistence::Disabled => {
            let jobs = InMemoryJobStore::default();
            let learning = InMemoryLearningStore::default();
            tracing::warn!(
                store.jobs.mode = jobs.mode(),
                store.learning.mode = learning.mode(),
                store.shared = jobs.is_shared() || learning.is_shared(),
                store.jobs.retained = MAX_STORED_JOBS,
                store.learning.retained = MAX_LEARNING_OUTCOMES,
                "{SERVICE_NAME} job and learning state is IN-PROCESS: no database is \
                 configured. Jobs and artifacts are visible only on the pod that produced \
                 them and learning aggregates are per-pod. Correct for a database-less local \
                 run and for tests; running more than one replica in this mode returns 404s \
                 for jobs the other pod holds and diverging plans for identical requests"
            );
            (Arc::new(jobs), Arc::new(learning))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FabricationJobRecord;
    use sea_orm::{DatabaseBackend, MockDatabase, MockExecResult};
    use std::collections::BTreeMap;

    /// Pins the premise `sanitize_finite` was written for — and shows that the
    /// premise does not survive serialization, which is why that function
    /// cannot do its job where it currently sits.
    ///
    /// `serde_json` has no representation for a non-finite `f64`, so NaN and
    /// ±Infinity become the JSON literal `null` *during serialization*. By the
    /// time a payload is a [`Value`], the damage is already done and there is
    /// no non-finite number left to find: `Number::from_f64` refuses to build
    /// one. A `Value`-walking guard is therefore a no-op on real data — the
    /// unreadable-row failure has to be prevented before or at serialization,
    /// or detected as an unexpected `null` in a required float field.
    #[test]
    fn non_finite_floats_become_null_before_a_value_can_be_inspected() {
        assert_eq!(serde_json::to_value(f64::NAN).unwrap(), Value::Null);
        assert_eq!(serde_json::to_value(f64::INFINITY).unwrap(), Value::Null);
        assert_eq!(
            serde_json::to_value(f64::NEG_INFINITY).unwrap(),
            Value::Null
        );

        // A `Value` cannot even hold a non-finite number.
        assert!(serde_json::Number::from_f64(f64::NAN).is_none());

        #[derive(serde::Serialize)]
        struct Payload {
            a: f64,
            b: Option<f64>,
            c: Vec<f64>,
        }
        let encoded = serde_json::to_value(Payload {
            a: f64::INFINITY,
            b: Some(f64::NEG_INFINITY),
            c: vec![1.0, f64::NAN],
        })
        .unwrap();
        assert_eq!(encoded["a"], Value::Null);
        assert_eq!(encoded["b"], Value::Null);
        assert_eq!(encoded["c"][1], Value::Null);

        // Consequently the guard finds nothing to sanitize.
        let mut walked = encoded.clone();
        sanitize_finite(&mut walked);
        assert_eq!(
            walked, encoded,
            "sanitize_finite is a no-op on serialized data"
        );
    }

    fn job(job_id: &str, request_id: &str, created_at_ms: u128) -> StoredFabricationJob {
        StoredFabricationJob {
            record: FabricationJobRecord {
                job_id: job_id.to_string(),
                request_id: request_id.to_string(),
                kind: "fabrication-plan".to_string(),
                status: "planned".to_string(),
                ok: true,
                severity: "ok".to_string(),
                summary: "test job".to_string(),
                artifact_count: 0,
                artifact_ids: Vec::new(),
                created_at_ms,
                updated_at_ms: created_at_ms,
            },
            plan: None,
            analysis: None,
            learning: None,
            artifacts: BTreeMap::new(),
        }
    }

    fn outcome(outcome_id: &str, success: bool, reward: f64) -> LearningOutcomeRecord {
        LearningOutcomeRecord {
            outcome_id: outcome_id.to_string(),
            request_id: format!("req-{outcome_id}"),
            job_id: None,
            objective: Some("bracket".to_string()),
            material: None,
            manufacturing_methods: vec!["milling".to_string()],
            machine_kind: Some("cnc-mill".to_string()),
            operation_sequence: vec!["milling".to_string()],
            assembly_strategy: Some("bolted".to_string()),
            success,
            reward,
            observations: Vec::new(),
            notes: Vec::new(),
            created_at_ms: 1_700_000_000_000,
        }
    }

    fn job_row(job: &StoredFabricationJob) -> fab_jobs::Model {
        fab_jobs::Model {
            job_id: job.record.job_id.clone(),
            request_id: job.record.request_id.clone(),
            kind: job.record.kind.clone(),
            status: job.record.status.clone(),
            ok: job.record.ok,
            severity: job.record.severity.clone(),
            summary: job.record.summary.clone(),
            artifact_count: job.record.artifact_count as i32,
            payload: serde_json::to_value(job).expect("job encodes"),
            created_at: to_timestamp(job.record.created_at_ms),
            updated_at: to_timestamp(job.record.updated_at_ms),
        }
    }

    fn outcome_row(outcome: &LearningOutcomeRecord) -> fab_learning_outcomes::Model {
        fab_learning_outcomes::Model {
            outcome_id: outcome.outcome_id.clone(),
            request_id: outcome.request_id.clone(),
            job_id: outcome.job_id.clone(),
            objective: outcome.objective.clone(),
            machine_kind: outcome.machine_kind.clone(),
            assembly_strategy: outcome.assembly_strategy.clone(),
            success: outcome.success,
            reward: outcome.reward,
            payload: serde_json::to_value(outcome).expect("outcome encodes"),
            created_at: to_timestamp(outcome.created_at_ms),
        }
    }

    // -- selection ---------------------------------------------------------

    #[test]
    fn stores_fall_back_to_memory_when_persistence_is_disabled() {
        let (jobs, learning) = build_stores(&Persistence::Disabled);
        assert_eq!(jobs.mode(), "memory");
        assert_eq!(learning.mode(), "memory");
        // The honest half of the contract: nothing here is shared, and the
        // deployment must not run two replicas on it.
        assert!(!jobs.is_shared());
        assert!(!learning.is_shared());
    }

    // -- in-memory semantics ----------------------------------------------

    #[tokio::test]
    async fn the_in_memory_job_store_still_honours_the_fifo_cap() {
        let store = InMemoryJobStore::new(3);
        for index in 0..5 {
            store
                .insert(job(&format!("job-{index}"), "req", 1_000 + index as u128))
                .await
                .expect("insert");
        }
        let recent = store.recent(16).await.expect("recent");
        let ids = recent
            .iter()
            .map(|job| job.record.job_id.as_str())
            .collect::<Vec<_>>();
        // Newest first, and the two oldest were evicted by the hard cap.
        assert_eq!(ids, vec!["job-4", "job-3", "job-2"]);
        assert!(store.get("job-0").await.expect("get").is_none());
        assert_eq!(store.counts().await.expect("counts"), (3, 0));
    }

    #[tokio::test]
    async fn the_in_memory_job_store_reports_a_displaced_job() {
        let store = InMemoryJobStore::new(8);
        assert!(
            !store
                .insert(job("job-1", "req", 1))
                .await
                .unwrap()
                .displaced
        );
        let second = store.insert(job("job-1", "req", 1)).await.unwrap();
        assert!(
            second.displaced,
            "a redelivered job id must be reported, not silently overwritten"
        );
        assert_eq!(store.recent(8).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn the_in_memory_learning_store_honours_the_fifo_cap_and_dedupes() {
        let store = InMemoryLearningStore::new(3);
        for index in 0..5 {
            store
                .insert(outcome(&format!("outcome-{index}"), true, 1.0))
                .await
                .expect("insert");
        }
        let window = store.recent(64).await.expect("recent");
        let ids = window
            .iter()
            .map(|outcome| outcome.outcome_id.as_str())
            .collect::<Vec<_>>();
        // Oldest-first over the retained window, two evicted by the cap.
        assert_eq!(ids, vec!["outcome-2", "outcome-3", "outcome-4"]);

        let before = store.snapshot().await.expect("snapshot");
        let replaced = store
            .insert(outcome("outcome-4", true, 1.0))
            .await
            .expect("insert");
        assert!(replaced.displaced);
        let after = store.snapshot().await.expect("snapshot");
        assert_eq!(
            before.outcome_count, after.outcome_count,
            "re-observing one outcome must not grow the sample count"
        );
        assert_eq!(before.successes, after.successes);
    }

    #[tokio::test]
    async fn a_poisoned_in_memory_lock_keeps_accepting_writes() {
        let store = Arc::new(InMemoryJobStore::new(8));
        // Poison the lock the way a panic inside a critical section would.
        let poisoner = Arc::clone(&store);
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.inner.write().expect("write");
            panic!("panic inside the critical section");
        })
        .join();
        assert!(
            store.inner.is_poisoned(),
            "test fixture must poison the lock"
        );

        // Before this module the store refused every write from here on while
        // /readyz stayed green.
        store.insert(job("job-after", "req", 1)).await.expect(
            "a poisoned lock must not permanently wedge the store into refusing every write",
        );
        assert!(store.get("job-after").await.expect("get").is_some());
    }

    // -- postgres semantics ------------------------------------------------

    #[tokio::test]
    async fn a_redelivered_job_upserts_to_one_row_and_reports_the_displacement() {
        let first = job("job-1", "req-1", 1_700_000_000_000);
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            // insert #1: the pre-read finds nothing...
            .append_query_results([Vec::<fab_jobs::Model>::new()])
            // ...and the upsert returns the new row.
            .append_query_results([vec![job_row(&first)]])
            // insert #2: the pre-read finds the row this time...
            .append_query_results([vec![job_row(&first)]])
            // ...and the upsert updates it.
            .append_query_results([vec![job_row(&first)]])
            // the final read of the table
            .append_query_results([vec![job_row(&first)]])
            .into_connection();
        let store = PostgresJobStore::new(Arc::new(db), 128);

        let initial = store.insert(first.clone()).await.expect("first insert");
        assert!(!initial.displaced);
        let redelivered = store.insert(first.clone()).await.expect("redelivery");
        assert!(
            redelivered.displaced,
            "an upsert that updated an existing row is the same event the in-memory \
             store reported as a displacement"
        );

        let rows = store.recent(128).await.expect("recent");
        assert_eq!(rows.len(), 1, "one job id must be one row");
        assert_eq!(rows[0].record.job_id, "job-1");
    }

    #[tokio::test]
    async fn a_job_round_trips_through_the_payload_column() {
        let stored = job("job-round-trip", "req-9", 1_700_000_123_456);
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results([vec![job_row(&stored)]])
            .into_connection();
        let store = PostgresJobStore::new(Arc::new(db), 128);
        let loaded = store
            .get("job-round-trip")
            .await
            .expect("get")
            .expect("row present");
        assert_eq!(loaded.record.job_id, stored.record.job_id);
        assert_eq!(loaded.record.created_at_ms, stored.record.created_at_ms);
        assert_eq!(loaded.record.summary, stored.record.summary);
    }

    #[tokio::test]
    async fn a_redelivered_outcome_upserts_and_is_not_double_counted() {
        let observed = outcome("outcome-1", true, 0.75);
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_query_results([Vec::<fab_learning_outcomes::Model>::new()])
            .append_query_results([vec![outcome_row(&observed)]])
            .append_query_results([vec![outcome_row(&observed)]])
            .append_query_results([vec![outcome_row(&observed)]])
            .append_query_results([vec![outcome_row(&observed)]])
            .into_connection();
        let store = PostgresLearningStore::new(Arc::new(db), 512);

        assert!(!store.insert(observed.clone()).await.unwrap().displaced);
        assert!(
            store.insert(observed.clone()).await.unwrap().displaced,
            "the same outcome id must be recognised as already stored"
        );

        // The aggregate that feeds plan_fabrication_with_policy: one delivery,
        // one sample, whatever max_deliver did.
        let snapshot = store.snapshot().await.expect("snapshot");
        assert_eq!(snapshot.outcome_count, 1);
        assert_eq!(snapshot.successes, 1);
        assert!((snapshot.average_reward - 0.75).abs() < 1e-9);
    }

    #[tokio::test]
    async fn the_learning_window_is_oldest_first_over_the_newest_rows() {
        let mut older = outcome("outcome-a", true, 1.0);
        older.created_at_ms = 1_700_000_000_000;
        let mut newer = outcome("outcome-b", false, -1.0);
        newer.created_at_ms = 1_700_000_060_000;
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            // The query is created_at DESC, so the driver yields newest first.
            .append_query_results([vec![outcome_row(&newer), outcome_row(&older)]])
            .into_connection();
        let store = PostgresLearningStore::new(Arc::new(db), 512);
        let window = store.recent(512).await.expect("recent");
        assert_eq!(
            window
                .iter()
                .map(|outcome| outcome.outcome_id.as_str())
                .collect::<Vec<_>>(),
            vec!["outcome-a", "outcome-b"],
            "aggregation must see the same oldest-first order the in-memory deque has"
        );
    }

    #[tokio::test]
    async fn retention_keeps_the_newest_rows_and_is_bounded() {
        let db = MockDatabase::new(DatabaseBackend::Postgres)
            .append_exec_results([MockExecResult {
                last_insert_id: 0,
                rows_affected: 7,
            }])
            .into_connection();
        let removed = sweep_retention(&db, "fab_jobs", "job_id", 128)
            .await
            .expect("sweep");
        assert_eq!(removed, 7);

        let logged = db.into_transaction_log();
        let statement = format!("{:?}", logged.first().expect("one statement"));
        // Keep-newest-N: order by recency, skip the retained window, delete the
        // rest — and never more than one batch in a single pass.
        assert!(statement.contains("DELETE FROM"), "{statement}");
        assert!(statement.contains("fab_jobs"), "{statement}");
        assert!(statement.contains("created_at"), "{statement}");
        assert!(statement.contains("DESC"), "{statement}");
        assert!(statement.contains("OFFSET"), "{statement}");
        assert!(statement.contains("LIMIT"), "{statement}");
        // The retained window and the batch ceiling are bound parameters, not
        // interpolated text.
        assert!(statement.contains("BigInt(Some(128))"), "{statement}");
        assert!(
            statement.contains(&format!("BigInt(Some({RETENTION_DELETE_BATCH}))")),
            "{statement}"
        );
    }

    #[test]
    fn the_sweep_runs_periodically_rather_than_on_every_write() {
        let writes = AtomicU64::new(0);
        let sweeps = (0..(RETENTION_SWEEP_EVERY * 3))
            .filter(|_| should_sweep(&writes))
            .count();
        assert_eq!(
            sweeps, 3,
            "one bounded sweep per {RETENTION_SWEEP_EVERY} writes"
        );
    }

    #[test]
    fn column_limits_are_respected_so_a_check_constraint_cannot_drop_a_job() {
        // The schema counts octets; the domain counts characters.
        let wide = "\u{00e9}".repeat(150);
        assert!(wide.chars().count() < MAX_SHORT_TEXT_BYTES);
        assert!(wide.len() > MAX_SHORT_TEXT_BYTES);
        let clamped = clamp_bytes(&wide, MAX_SHORT_TEXT_BYTES);
        assert!(clamped.len() <= MAX_SHORT_TEXT_BYTES);
        // Never split a character in half.
        assert!(std::str::from_utf8(clamped.as_bytes()).is_ok());
        assert_eq!(clamp_bytes("short", MAX_SHORT_TEXT_BYTES), "short");
    }

    #[test]
    fn a_non_finite_reward_is_neutralised_rather_than_rejected() {
        // Two failures at once: the check constraint refuses NaN/Infinity, and
        // serde_json encodes them as `null`, which then will not decode back
        // into the f64 and makes the whole payload unreadable on the next read.
        for poison in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let sanitized = sanitize_outcome(outcome("outcome-poison", false, poison));
            assert_eq!(sanitized.reward, 0.0);
            let encoded = serde_json::to_value(&sanitized).expect("encodes");
            let decoded: LearningOutcomeRecord =
                serde_json::from_value(encoded).expect("a stored outcome must decode back");
            assert_eq!(decoded.reward, 0.0);
        }
        assert_eq!(
            sanitize_outcome(outcome("outcome-fine", true, -0.5)).reward,
            -0.5
        );
    }

    // -- opt-in integration ------------------------------------------------

    /// End-to-end against a real Postgres, because `MockDatabase` replays
    /// canned rows and therefore cannot tell you that the SQL parses, that the
    /// column types line up, or that a check constraint is satisfied.
    ///
    /// Ignored by default and skipped without a URL, so `cargo test` needs no
    /// database. Run it against a throwaway cluster with the two tables from
    /// `pg-defs/schema/schema.sql` applied:
    ///
    /// ```text
    /// FABRICATION_TEST_DATABASE_URL=postgres://postgres@127.0.0.1:5432/fabstoretest \
    ///   cargo test --lib stores::tests::against_a_real_postgres -- --ignored --nocapture
    /// ```
    #[tokio::test]
    #[ignore = "needs a throwaway Postgres with the daedalus fab tables applied"]
    async fn against_a_real_postgres_the_upsert_ordering_and_retention_hold() {
        let Ok(url) = std::env::var("FABRICATION_TEST_DATABASE_URL") else {
            eprintln!("FABRICATION_TEST_DATABASE_URL is unset; skipping");
            return;
        };
        let db = Arc::new(
            sea_orm::Database::connect(url)
                .await
                .expect("connect to the throwaway database"),
        );
        for table in ["fab_jobs", "fab_learning_outcomes"] {
            db.execute(Statement::from_string(
                DbBackend::Postgres,
                format!(r#"TRUNCATE TABLE "daedalus"."{table}""#),
            ))
            .await
            .expect("truncate");
        }

        // Upsert: the same deterministic id twice is one row.
        let jobs = PostgresJobStore::new(Arc::clone(&db), 3);
        let redelivered = job("job-1", "req-1", 1_700_000_000_000);
        assert!(!jobs.insert(redelivered.clone()).await.unwrap().displaced);
        assert!(jobs.insert(redelivered.clone()).await.unwrap().displaced);
        assert_eq!(jobs.recent(128).await.unwrap().len(), 1);
        assert!(jobs.get("job-1").await.unwrap().is_some());
        assert!(jobs.detail("job-1").await.unwrap().is_some());
        assert!(jobs.release_bundle("job-1").await.unwrap().is_some());

        // Ordering plus retention: write past the target and sweep.
        for index in 0..8 {
            jobs.insert(job(
                &format!("job-{index:02}"),
                "req-1",
                1_700_000_100_000 + index as u128 * 1_000,
            ))
            .await
            .expect("insert");
        }
        let removed = sweep_retention(&db, "fab_jobs", "job_id", 3)
            .await
            .expect("sweep");
        assert!(removed > 0, "the sweep must trim past the retention target");
        let kept = jobs.recent(128).await.expect("recent");
        assert_eq!(kept.len(), 3, "retention keeps exactly the newest N");
        assert_eq!(
            kept.iter()
                .map(|job| job.record.job_id.as_str())
                .collect::<Vec<_>>(),
            vec!["job-07", "job-06", "job-05"],
            "and it keeps the NEWEST N, newest first"
        );

        // A real plan job, not a fixture: the payload is a few hundred KB of
        // nested domain types and it must come back out of jsonb intact.
        let planned = crate::stored_plan_job(
            &crate::plan_fabrication(crate::FabricationPlanRequest {
                request_id: Some("integration-round-trip".to_string()),
                objective: "PETG enclosure with printed shell and machined datum insert"
                    .to_string(),
                material: None,
                stock: None,
                tolerance_mm: Some(0.12),
                quantity: Some(1),
                machines: None,
                constraints: None,
                parts: None,
                design_inputs: None,
                existing_instructions: None,
                learning: None,
            })
            .expect("plan"),
        );
        let planned_id = planned.record.job_id.clone();
        jobs.insert(planned.clone()).await.expect("insert plan job");
        let loaded = jobs
            .get(&planned_id)
            .await
            .expect("get")
            .expect("the plan job is in the table");
        assert_eq!(
            serde_json::to_value(&loaded).expect("encode"),
            serde_json::to_value(&planned).expect("encode"),
            "a real plan job must survive the jsonb payload byte for byte"
        );
        assert!(jobs.release_bundle(&planned_id).await.unwrap().is_some());

        // Learning: one outcome id is one sample no matter how often it is
        // redelivered, and a non-finite reward does not abort the insert.
        let learning = PostgresLearningStore::new(Arc::clone(&db), 512);
        let observed = outcome("outcome-1", true, 0.75);
        assert!(!learning.insert(observed.clone()).await.unwrap().displaced);
        assert!(learning.insert(observed.clone()).await.unwrap().displaced);
        let snapshot = learning.snapshot().await.expect("snapshot");
        assert_eq!(snapshot.outcome_count, 1);
        assert_eq!(snapshot.successes, 1);

        let mut poisoned = outcome("outcome-nan", false, f64::NAN);
        poisoned.created_at_ms = 1_700_000_200_000;
        learning
            .insert(poisoned)
            .await
            .expect("a non-finite reward must not abort the insert");
        let window = learning.recent(512).await.expect("recent");
        assert_eq!(
            window
                .iter()
                .map(|outcome| outcome.outcome_id.as_str())
                .collect::<Vec<_>>(),
            vec!["outcome-1", "outcome-nan"],
            "the window is oldest-first"
        );
    }

    #[test]
    fn milliseconds_convert_to_a_timestamptz_without_panicking() {
        let converted = to_timestamp(1_700_000_000_000);
        assert_eq!(converted.timestamp_millis(), 1_700_000_000_000);
        // An absurd value clamps instead of overflowing.
        let _ = to_timestamp(u128::MAX);
    }
}
